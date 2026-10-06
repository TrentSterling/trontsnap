// === Linux screen recorder ===
// One hotkey toggles a full-screen recording; stop delivers the file (clipboard
// file ref + shutter + toast), same contract as the Windows recorder.
//
// Backends, first that applies wins (TRONTSNAP_RECORDER=hook|kms|gnome|x11 forces one):
//
//   hook   ~/.config/trontsnap/record-start.sh <out> [+ record-stop.sh]. User override.
//   kms    capkit's polkit-whitelisted root helper: kmsgrab -> VCE hardware H.264.
//          ~0% CPU, solid 30fps on the Mullins. The "perfect performance" path.
//   gnome  org.gnome.Shell.Screencast over D-Bus. Any GNOME Wayland box, no root,
//          but the shell encodes on the CPU (VP8 webm here: ~9fps, 2+ cores pegged).
//   x11    ffmpeg x11grab + libx264 ultrafast.
//
// Audio ("Record audio" setting) = system audio, i.e. the default sink's MONITOR
// (what you hear, matching WASAPI loopback on Windows), recorded as a user-side
// sidecar wav and muxed after stop. HARD-WON: the mux is END-aligned. Both
// streams stop within ms of each other, but they start with different latencies
// (pkexec + kmsgrab warmup, and capkit trims the first ~1s of video), so
// aligning the starts drifts and aligning the ends doesn't.
//
// Collision rules with capkit's own Super+Alt+R recorder (same kmsgrab helper):
//   - never start a second kmsgrab recording: if one is live (capkit's), refuse
//     and say so, instead of stacking two root ffmpegs that capkit-stop would
//     both kill
//   - capkit-stop SIGINTs every kmsgrab, so Super+Alt+R during a TrontSnap
//     recording stops it. That's treated as a normal stop: the watcher sees the
//     helper exit, finalizes, delivers, and clears the session.
//   - capkit's rec_*.mp4 for OUR session is moved out of ~/Videos/captures into
//     the TrontSnap folder; nothing of capkit's is touched otherwise.
//
// Every session gets a watcher thread that owns the processes and does the
// finalize, so a stop from anywhere (our hotkey, capkit's hotkey, app quit)
// lands in one code path. Partial files live in ~/.cache/trontsnap/rec until
// they're complete, so the gallery's inotify never sees a half-written video.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const KMS_HELPER: &str = "/usr/local/bin/capkit-kms";
const KMS_STOP: &str = "/usr/local/bin/capkit-stop";
const FPS: u32 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    Hook,
    Kms,
    Gnome,
    X11,
}

/// How toggle() tells a live session to stop. The watcher owns the real handles.
enum StopHandle {
    /// SIGINT this process group (x11 ffmpeg, or the hook script's group).
    Pgid(u32),
    /// User's record-stop.sh, then SIGINT the group as a backstop.
    HookScript(PathBuf, u32),
    /// pkexec capkit-stop (passwordless via the capkit polkit rule).
    Kms,
    /// Wake the D-Bus holder thread so it calls StopScreencast.
    Gnome(crossbeam_channel::Sender<()>),
}

struct Session {
    id: u64,
    stop: StopHandle,
    /// Audio sidecar's pgid, signalled at the same instant as the video so the
    /// end-aligned mux lines up.
    audio_pgid: Option<u32>,
    watcher: JoinHandle<()>,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Lock SESSION, recovering from poisoning (a panicked stop must not turn every
/// future toggle into a silent no-op).
fn session() -> std::sync::MutexGuard<'static, Option<Session>> {
    SESSION.lock().unwrap_or_else(|p| p.into_inner())
}

/// Hotkey entry: first press starts, second press stops. Safe from any thread;
/// never blocks on ffmpeg finalize (the watcher does that).
pub fn toggle() {
    let taken = session().take();
    match taken {
        // A finished watcher means the session already ended on its own (capkit's
        // hotkey stopped it, or the helper died): the press means START.
        Some(sess) if !sess.watcher.is_finished() => trigger_stop(&sess),
        _ => match start() {
            Ok(sess) => {
                // A start can race a watcher that already finished (helper died at
                // birth): only park the session if its watcher is still running.
                if !sess.watcher.is_finished() {
                    *session() = Some(sess);
                }
            }
            Err(e) => {
                eprintln!("trontsnap: recording unavailable: {e:#}");
                notify("Recording didn't start", &format!("{e:#}"));
            }
        },
    }
}

/// Stop any live recording and wait for it to finalize. Called on app quit so
/// an exit never orphans a recorder that would run until the disk fills.
pub fn shutdown() {
    let taken = session().take();
    if let Some(sess) = taken {
        trigger_stop(&sess);
        let _ = sess.watcher.join();
    }
}

// === Stop ===

fn trigger_stop(sess: &Session) {
    // Audio first: it's the cheap, instant one, and ends must match.
    if let Some(pg) = sess.audio_pgid {
        sigint_group(pg);
    }
    match &sess.stop {
        StopHandle::Pgid(pg) => sigint_group(*pg),
        StopHandle::HookScript(script, pg) => {
            let _ = Command::new("bash").arg(script).status();
            sigint_group(*pg);
        }
        StopHandle::Kms => {
            let _ = Command::new("pkexec")
                .arg(KMS_STOP)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        StopHandle::Gnome(tx) => {
            let _ = tx.try_send(());
        }
    }
}

/// SIGINT a whole process group. SIGINT is ffmpeg's clean shutdown (flushes and
/// writes the moov atom); std's Child::kill is SIGKILL and leaves an unplayable
/// file. Group-wide because a signal to a bash wrapper isn't forwarded to the
/// ffmpeg it spawned.
fn sigint_group(pgid: u32) {
    let _ = Command::new("kill")
        .args(["-INT", "--", &format!("-{pgid}")])
        .stderr(Stdio::null())
        .status();
}

// === Start ===

fn pick_backend() -> anyhow::Result<Backend> {
    if let Ok(forced) = std::env::var("TRONTSNAP_RECORDER") {
        return match forced.as_str() {
            "hook" => Ok(Backend::Hook),
            "kms" => Ok(Backend::Kms),
            "gnome" => Ok(Backend::Gnome),
            "x11" => Ok(Backend::X11),
            other => anyhow::bail!("TRONTSNAP_RECORDER={other} isn't a backend (hook|kms|gnome|x11)"),
        };
    }
    if script_path("record-start.sh").is_some() {
        return Ok(Backend::Hook);
    }
    if is_exec(KMS_HELPER) && is_exec(KMS_STOP) {
        return Ok(Backend::Kms);
    }
    let wayland = std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland")
        || std::env::var_os("WAYLAND_DISPLAY").is_some();
    if wayland {
        if gnome_screencast_supported() {
            return Ok(Backend::Gnome);
        }
        anyhow::bail!(
            "no Wayland recorder here: this isn't GNOME, so drop a \
             ~/.config/trontsnap/record-start.sh that records to $1"
        );
    }
    Ok(Backend::X11)
}

fn start() -> anyhow::Result<Session> {
    let backend = pick_backend()?;
    let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
    let stage = stage_dir()?;
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

    // Kms collision guard runs BEFORE any audio starts, so a refusal leaves nothing behind.
    if backend == Backend::Kms && kmsgrab_running() {
        anyhow::bail!(
            "another kmsgrab recording is already running (capkit?). \
             Stop it with Super+Alt+R first; TrontSnap won't stack a second one"
        );
    }

    // Hook scripts own their audio; everyone else gets the monitor sidecar.
    let audio = if backend != Backend::Hook && crate::settings::record_audio() {
        start_audio(&stage.join(format!("TrontSnap_{stamp}.audio.wav")))
    } else {
        None
    };
    let audio_pgid = audio.as_ref().map(|(c, _)| c.id());

    let started = match backend {
        Backend::Hook => start_hook(&stage, &stamp, id, audio),
        Backend::Kms => start_kms(&stamp, id, audio),
        Backend::Gnome => start_gnome(&stage, &stamp, id, audio),
        Backend::X11 => start_x11(&stage, &stamp, id, audio),
    };
    // start_* own the audio child from here; each one reaps it on failure.
    let (stop, watcher) = started?;
    let video_pgid = match &stop {
        StopHandle::Pgid(pg) | StopHandle::HookScript(_, pg) => Some(*pg),
        _ => None,
    };
    write_marker(backend, video_pgid, audio_pgid);
    eprintln!("trontsnap: recording via {backend:?}");
    notify(
        "Recording",
        "Ctrl+Shift+PrtSc to stop. Saves to Pictures/TrontSnap",
    );
    Ok(Session { id, stop, audio_pgid, watcher })
}

type Audio = Option<(Child, PathBuf)>;

/// System-audio sidecar: the default sink's monitor via pulse (PipeWire's pulse
/// shim answers @DEFAULT_MONITOR@ too). Best-effort: no audio beats no recording.
fn start_audio(wav: &Path) -> Audio {
    use std::os::unix::process::CommandExt as _;
    let child = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "pulse", "-i", "@DEFAULT_MONITOR@"])
        .args(["-ac", "2", "-ar", "48000", "-c:a", "pcm_s16le"])
        .arg(wav)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| eprintln!("trontsnap: audio sidecar unavailable: {e}"))
        .ok()?;
    Some((child, wav.to_path_buf()))
}

fn start_hook(stage: &Path, stamp: &str, id: u64, audio: Audio) -> anyhow::Result<(StopHandle, JoinHandle<()>)> {
    use std::os::unix::process::CommandExt as _;
    let raw = stage.join(format!("TrontSnap_{stamp}.mp4"));
    let spawned = script_path("record-start.sh")
        .ok_or_else(|| anyhow::anyhow!("~/.config/trontsnap/record-start.sh is missing"))
        .and_then(|script| {
            Command::new("bash")
                .arg(&script)
                .arg(&raw)
                .stdin(Stdio::null())
                .process_group(0)
                .spawn()
                .map_err(anyhow::Error::from)
        });
    let child = match spawned {
        Ok(c) => c,
        Err(e) => {
            reap_audio(audio);
            return Err(e);
        }
    };
    let pg = child.id();
    let stop = match script_path("record-stop.sh") {
        Some(s) => StopHandle::HookScript(s, pg),
        None => StopHandle::Pgid(pg),
    };
    let w = std::thread::spawn(move || {
        let mut child = child;
        let _ = child.wait();
        finish(id, Some(raw), audio);
    });
    Ok((stop, w))
}

fn start_x11(stage: &Path, stamp: &str, id: u64, audio: Audio) -> anyhow::Result<(StopHandle, JoinHandle<()>)> {
    use std::os::unix::process::CommandExt as _;
    let raw = stage.join(format!("TrontSnap_{stamp}.mp4"));
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into());
    let cursor = if crate::settings::capture_cursor() { "1" } else { "0" };
    let spawned = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "x11grab", "-draw_mouse", cursor])
        .args(["-framerate", &FPS.to_string(), "-i", &display])
        // ultrafast: screen content on a weak CPU; even dimensions for H.264.
        .args(["-vf", "crop=trunc(iw/2)*2:trunc(ih/2)*2", "-c:v", "libx264", "-preset", "ultrafast"])
        .args(["-crf", "28", "-pix_fmt", "yuv420p"])
        .arg(&raw)
        .stdin(Stdio::null())
        .process_group(0)
        .spawn();
    let child = match spawned {
        Ok(c) => c,
        Err(e) => {
            reap_audio(audio);
            anyhow::bail!("ffmpeg not found; install it for recording (`sudo apt install ffmpeg`): {e}");
        }
    };
    let pg = child.id();
    let w = std::thread::spawn(move || {
        let mut child = child;
        let _ = child.wait();
        finish(id, Some(raw), audio);
    });
    Ok((StopHandle::Pgid(pg), w))
}

fn start_kms(stamp: &str, id: u64, audio: Audio) -> anyhow::Result<(StopHandle, JoinHandle<()>)> {
    let _ = stamp; // capkit names its own output; we rename on delivery
    let spawned = Command::new("pkexec")
        .args([KMS_HELPER, &FPS.to_string(), "24"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let child = match spawned {
        Ok(c) => c,
        Err(e) => {
            reap_audio(audio);
            anyhow::bail!("pkexec {KMS_HELPER} failed: {e}");
        }
    };
    let w = std::thread::spawn(move || kms_watch(id, child, audio));
    Ok((StopHandle::Kms, w))
}

/// Owns the capkit helper. Polls for the kmsgrab ffmpeg so an EXTERNAL stop
/// (capkit's Super+Alt+R) also cuts the audio sidecar at the right instant, then
/// waits out capkit's warmup-trim remux and reads its SAVED:<path> line.
fn kms_watch(id: u64, mut child: Child, mut audio: Audio) {
    let mut seen = false;
    let mut audio_cut = false;
    let mut killed = false;
    let born = Instant::now();
    loop {
        if matches!(child.try_wait(), Ok(Some(_))) {
            break;
        }
        let live = kmsgrab_running();
        if live {
            seen = true;
        }
        // Grab ended (either stop path) or never came up: stop the audio now.
        let dead_at_birth = !seen && born.elapsed() > Duration::from_secs(15);
        if !audio_cut && ((seen && !live) || dead_at_birth) {
            if let Some((a, _)) = &audio {
                sigint_group(a.id());
            }
            audio_cut = true;
        }
        if dead_at_birth && !killed {
            // pkexec auth refused/hung: give up rather than wait forever. (Killable:
            // until pkexec execs the helper its real uid is still ours.)
            let _ = child.kill();
            killed = true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if !audio_cut {
        if let Some((a, _)) = &audio {
            sigint_group(a.id());
        }
    }
    let mut out = String::new();
    if let Some(mut so) = child.stdout.take() {
        use std::io::Read as _;
        let _ = so.read_to_string(&mut out);
    }
    let _ = child.wait();
    let raw = out
        .lines()
        .find_map(|l| l.strip_prefix("SAVED:"))
        .map(|p| PathBuf::from(p.trim()));
    if raw.is_none() {
        notify("Recording failed", "the kmsgrab helper didn't produce a file");
    }
    // Wait the sidecar out before muxing (it has its own SIGINT already).
    if let Some((a, _)) = audio.as_mut() {
        let _ = a.wait();
    }
    finish(id, raw, audio);
}

fn start_gnome(stage: &Path, stamp: &str, id: u64, audio: Audio) -> anyhow::Result<(StopHandle, JoinHandle<()>)> {
    // No extension: the shell appends the right one for whatever encoder its
    // pipeline picked (.webm for VP8, .mp4 when a hardware H.264 path exists).
    let template = stage.join(format!("TrontSnap_{stamp}")).display().to_string();
    let cursor = crate::settings::capture_cursor();
    let (stop_tx, stop_rx) = crossbeam_channel::bounded::<()>(1);
    let (ready_tx, ready_rx) = crossbeam_channel::bounded::<anyhow::Result<()>>(1);

    // The thread HOLDS the bus connection for the whole recording: the shell
    // stops a screencast the moment its caller's connection vanishes (a bare
    // `gdbus call` records exactly one frame). Bonus: if TrontSnap crashes, the
    // recording ends instead of running forever.
    let w = std::thread::spawn(move || {
        let mut audio = audio;
        let begun = (|| -> anyhow::Result<(zbus::blocking::Connection, String)> {
            let conn = zbus::blocking::Connection::session()?;
            let proxy = gnome_proxy(&conn)?;
            let mut opts = std::collections::HashMap::<&str, zbus::zvariant::Value>::new();
            opts.insert("framerate", zbus::zvariant::Value::from(FPS as i32));
            opts.insert("draw-cursor", zbus::zvariant::Value::from(cursor));
            let (ok, file): (bool, String) = proxy.call("Screencast", &(template.as_str(), opts))?;
            if !ok {
                anyhow::bail!("GNOME Shell refused to start a screencast (another one running?)");
            }
            Ok((conn, file))
        })();
        let (conn, file) = match begun {
            Ok(v) => {
                let _ = ready_tx.send(Ok(()));
                v
            }
            Err(e) => {
                reap_audio(audio.take());
                let _ = ready_tx.send(Err(e));
                return;
            }
        };
        // Sender dropped (app quit without shutdown) also counts as stop.
        let _ = stop_rx.recv();
        if let Some((a, _)) = &audio {
            sigint_group(a.id());
        }
        if let Ok(proxy) = gnome_proxy(&conn) {
            let _: zbus::Result<bool> = proxy.call("StopScreencast", &());
        }
        drop(conn);
        let raw = PathBuf::from(file);
        wait_settled(&raw);
        if let Some((a, _)) = audio.as_mut() {
            let _ = a.wait();
        }
        finish(id, Some(raw), audio);
    });

    match ready_rx.recv_timeout(Duration::from_secs(8)) {
        Ok(Ok(())) => Ok((StopHandle::Gnome(stop_tx), w)),
        Ok(Err(e)) => Err(e),
        Err(_) => {
            let _ = stop_tx.try_send(());
            anyhow::bail!("GNOME Shell screencast didn't answer within 8s")
        }
    }
}

fn gnome_proxy(conn: &zbus::blocking::Connection) -> zbus::Result<zbus::blocking::Proxy<'static>> {
    zbus::blocking::Proxy::new(
        conn,
        "org.gnome.Shell.Screencast",
        "/org/gnome/Shell/Screencast",
        "org.gnome.Shell.Screencast",
    )
}

/// Cheap probe: is the shell's screencast service on the bus and willing?
fn gnome_screencast_supported() -> bool {
    (|| -> zbus::Result<bool> {
        let conn = zbus::blocking::Connection::session()?;
        gnome_proxy(&conn)?.get_property::<bool>("ScreencastSupported")
    })()
    .unwrap_or(false)
}

/// The shell's StopScreencast can return before the muxer's last write lands.
/// Wait until the file size holds still for a beat (bounded).
fn wait_settled(p: &Path) {
    let mut last = u64::MAX;
    for _ in 0..40 {
        let now = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        if now == last && now > 0 {
            return;
        }
        last = now;
        std::thread::sleep(Duration::from_millis(150));
    }
}

fn reap_audio(audio: Audio) {
    if let Some((mut a, wav)) = audio {
        sigint_group(a.id());
        let _ = a.wait();
        let _ = std::fs::remove_file(wav);
    }
}

// === Finalize ===

/// Common tail for every backend: mux the audio sidecar in (end-aligned), move
/// the result into the TrontSnap folder, deliver, clear the session.
fn finish(id: u64, raw: Option<PathBuf>, audio: Audio) {
    // Clear our slot first: if the stop came from outside (capkit hotkey, helper
    // died), the next hotkey press must START, not try to stop a ghost.
    {
        let mut slot = session();
        if slot.as_ref().map(|s| s.id) == Some(id) {
            *slot = None;
        }
    }
    if let Some(m) = marker_path() {
        let _ = std::fs::remove_file(m);
    }
    let wav = audio.map(|(mut a, wav)| {
        let _ = a.wait();
        wav
    });
    let Some(raw) = raw.filter(|p| p.exists()) else {
        eprintln!("trontsnap: recording produced no file");
        if let Some(w) = wav {
            let _ = std::fs::remove_file(w);
        }
        return;
    };
    let out = match deliver_path(&raw) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("trontsnap: no save dir: {e:#}");
            return;
        }
    };

    let muxed = wav
        .as_deref()
        .filter(|w| std::fs::metadata(w).map(|m| m.len() > 4096).unwrap_or(false))
        .map(|w| mux_end_aligned(&raw, w, &out))
        .unwrap_or(false);
    if muxed {
        let _ = std::fs::remove_file(&raw);
    } else if let Err(e) = move_file(&raw, &out) {
        eprintln!("trontsnap: couldn't move recording into place: {e}");
        return;
    }
    if let Some(w) = wav {
        let _ = std::fs::remove_file(w);
    }

    let clipboard_ok = match crate::clipboard::set_file(&out) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("trontsnap: recording clipboard failed: {e:#}");
            false
        }
    };
    crate::sound::play_shutter();
    crate::toast::launch(&out, clipboard_ok);
    println!("recorded -> {}", out.display());
}

/// TrontSnap_<now>.<ext of raw> in the TrontSnap folder. Stamped at delivery,
/// matching the stills' "file time = when it landed" convention.
fn deliver_path(raw: &Path) -> anyhow::Result<PathBuf> {
    let dir = crate::capture::trontsnap_dir()?;
    std::fs::create_dir_all(&dir)?;
    let ext = raw.extension().and_then(|e| e.to_str()).unwrap_or("mp4");
    let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
    let mut p = dir.join(format!("TrontSnap_{stamp}.{ext}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("TrontSnap_{stamp}_{n}.{ext}"));
        n += 1;
    }
    Ok(p)
}

/// Mux video + wav, trimming the audio's head so both END together. Writes to a
/// non-media temp name and renames, so the gallery never sees a partial file.
fn mux_end_aligned(video: &Path, wav: &Path, out: &Path) -> bool {
    let (Some(vd), Some(ad)) = (probe_duration(video), probe_duration(wav)) else {
        return false;
    };
    let head = (ad - vd).max(0.0);
    let webm = out.extension().and_then(|e| e.to_str()) == Some("webm");
    let tmp = out.with_extension("muxing");
    let ok = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(video)
        .args(["-ss", &format!("{head:.3}"), "-i"])
        .arg(wav)
        .args(["-map", "0:v:0", "-map", "1:a:0", "-c:v", "copy"])
        .args(if webm { ["-c:a", "libopus", "-b:a", "128k"] } else { ["-c:a", "aac", "-b:a", "160k"] })
        .args(["-shortest", "-f", if webm { "webm" } else { "mp4" }])
        .args(if webm { vec![] } else { vec!["-movflags", "+faststart"] })
        .arg(&tmp)
        .stdin(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok && std::fs::rename(&tmp, out).is_ok() {
        return true;
    }
    let _ = std::fs::remove_file(&tmp);
    false
}

fn probe_duration(p: &Path) -> Option<f64> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"])
        .arg(p)
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// rename(), falling back to copy+remove across filesystems. The copy goes via a
/// temp name so the gallery's watcher never catches it half-written.
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    let tmp = to.with_extension("moving");
    std::fs::copy(from, &tmp)?;
    std::fs::rename(&tmp, to)?;
    std::fs::remove_file(from)
}

// === Crash recovery ===
//
// A live session leaves a marker in the stage dir; finish() deletes it. A marker
// at app start therefore means the last app died mid-recording, and its ffmpegs
// may still be running (process groups we made detach from us by design; the
// kmsgrab one is root). recover_orphans stops them and moves whatever video
// they produced into the TrontSnap folder, so a crash never means "recording
// until the disk fills" or a lost clip.

fn marker_path() -> Option<PathBuf> {
    stage_dir().ok().map(|d| d.join("session.marker"))
}

fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn write_marker(backend: Backend, video_pgid: Option<u32>, audio_pgid: Option<u32>) {
    let Some(p) = marker_path() else { return };
    let body = format!(
        "backend={backend:?}\nstart={}\nvideo_pgid={}\naudio_pgid={}\n",
        now_epoch(),
        video_pgid.unwrap_or(0),
        audio_pgid.unwrap_or(0)
    );
    let _ = std::fs::write(p, body);
}

/// Run once at app start (off the UI thread).
pub fn recover_orphans() {
    let Some(marker) = marker_path() else { return };
    let Ok(body) = std::fs::read_to_string(&marker) else { return };
    let field = |k: &str| {
        body.lines()
            .find_map(|l| l.strip_prefix(k).and_then(|v| v.strip_prefix('=')))
            .unwrap_or("")
            .to_string()
    };
    let start: u64 = field("start").parse().unwrap_or(0);
    let Ok(stage) = stage_dir() else { return };
    eprintln!("trontsnap: found a recording left running by a previous session; salvaging");

    // User-owned groups: only signal a pgid that still IS one of our ffmpegs
    // (its cmdline points into the stage dir) so a recycled pid is never hit.
    for k in ["video_pgid", "audio_pgid"] {
        if let Ok(pg) = field(k).parse::<u32>() {
            if pg > 0 && cmdline_mentions(pg, &stage) {
                sigint_group(pg);
            }
        }
    }

    // Root kmsgrab: only stop one that started with our dead session, not a
    // capkit recording Trent began afterwards.
    let mut kms_file = None;
    if field("backend") == "Kms" {
        if kmsgrab_pids().iter().any(|&pid| proc_start_epoch(pid).map(|t| t + 2 >= start && t <= start + 30).unwrap_or(false)) {
            let _ = Command::new("pkexec").arg(KMS_STOP).stdin(Stdio::null()).status();
            for _ in 0..100 {
                if !kmsgrab_running() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            // capkit's warmup-trim remux runs after ffmpeg exits.
            std::thread::sleep(Duration::from_secs(3));
        }
        kms_file = newest_capkit_since(start);
    }
    std::thread::sleep(Duration::from_millis(800)); // let SIGINTed ffmpegs finalize

    let mut salvaged = Vec::new();
    let staged = std::fs::read_dir(&stage)
        .map(|rd| rd.flatten().map(|e| e.path()).collect::<Vec<_>>())
        .unwrap_or_default();
    for p in staged.iter().chain(kms_file.iter()) {
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if matches!(ext, "mp4" | "webm") && !name.contains(".audio.") {
            wait_settled(p);
            if let Ok(out) = deliver_path(p) {
                if move_file(p, &out).is_ok() {
                    salvaged.push(out);
                }
            }
        } else if name.contains(".audio.") || matches!(ext, "muxing" | "moving") {
            let _ = std::fs::remove_file(p);
        }
    }
    let _ = std::fs::remove_file(&marker);
    if !salvaged.is_empty() {
        notify(
            "Recovered a recording",
            "TrontSnap closed mid-recording last time; the clip is in Pictures/TrontSnap",
        );
    }
}

fn cmdline_mentions(pid: u32, dir: &Path) -> bool {
    let needle = dir.display().to_string();
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .map(|c| String::from_utf8_lossy(&c).contains(&needle))
        .unwrap_or(false)
}

/// Wall-clock start of a pid: /proc/<pid>/stat field 22 (ticks since boot) +
/// btime. CLK_TCK is 100 on every Linux this will meet.
fn proc_start_epoch(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // comm can hold spaces/parens: fields count from after the LAST ')'.
    let rest = &stat[stat.rfind(')')? + 2..];
    let ticks: u64 = rest.split(' ').nth(19)?.parse().ok()?;
    let btime: u64 = std::fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()?;
    Some(btime + ticks / 100)
}

fn newest_capkit_since(start: u64) -> Option<PathBuf> {
    let dir = dirs::video_dir()?.join("captures");
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).map(|n| n.starts_with("rec_") && n.ends_with(".mp4")).unwrap_or(false)
        })
        .filter_map(|p| {
            let t = std::fs::metadata(&p).ok()?.modified().ok()?;
            let secs = t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
            (secs + 2 >= start).then_some((secs, p))
        })
        .max_by_key(|(t, _)| *t)
        .map(|(_, p)| p)
}

// === Helpers ===

/// Staging dir for in-progress recordings (outside the gallery's watch).
fn stage_dir() -> anyhow::Result<PathBuf> {
    let d = dirs::cache_dir()
        .ok_or_else(|| anyhow::anyhow!("no cache dir"))?
        .join("trontsnap")
        .join("rec");
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

fn script_path(name: &str) -> Option<PathBuf> {
    let p = dirs::config_dir()?.join("trontsnap").join(name);
    p.exists().then_some(p)
}

fn is_exec(p: &str) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(p).map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

/// Is any kmsgrab ffmpeg alive (ours or capkit's)?
fn kmsgrab_running() -> bool {
    !kmsgrab_pids().is_empty()
}

/// Pids of every kmsgrab ffmpeg. Scans /proc directly.
/// HARD-WON: not `pgrep -f`, which matches its own shell wrapper's cmdline.
fn kmsgrab_pids() -> Vec<u32> {
    let Ok(rd) = std::fs::read_dir("/proc") else { return Vec::new() };
    rd.flatten()
        .filter_map(|e| {
            let pid: u32 = e.file_name().to_str()?.parse().ok()?;
            let cmd = std::fs::read(e.path().join("cmdline")).ok()?;
            let mut args = cmd.split(|&b| b == 0);
            let a0 = args.next()?;
            let is_ffmpeg = a0 == b"ffmpeg" || a0.ends_with(b"/ffmpeg");
            (is_ffmpeg && args.any(|a| a == b"kmsgrab")).then_some(pid)
        })
        .collect()
}

/// Best-effort desktop notification (transient: doesn't pile up in the tray).
fn notify(summary: &str, body: &str) {
    let mut cmd = Command::new("notify-send");
    cmd.args(["-a", "TrontSnap", "-t", "2500", "-h", "int:transient:1", summary, body])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let _ = crate::shellexec::spawn_detached(cmd);
}
