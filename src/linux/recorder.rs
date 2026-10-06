// === Linux screen recorder ===
// One hotkey toggles a screen recording (full screen, or a picked region); stop
// delivers the file (clipboard file ref + shutter + toast), same contract as the
// Windows recorder.
//
// Backends, first that applies wins (TRONTSNAP_RECORDER=hook|kms|gnome|x11 forces one):
//
//   hook   ~/.config/trontsnap/record-start.sh <out> [+ record-stop.sh]. User override.
//   kms    capkit's polkit-whitelisted root helper: kmsgrab -> VCE hardware H.264.
//          ~0.1 core, every displayed frame, 30fps container. The "perfect performance" path.
//          Costs: ~3 s from the press to the first clip frame (pkexec + VAAPI init + the
//          helper trimming the first second), and the pointer is NOT in the picture
//          (kmsgrab reads the primary plane; the cursor is on the hardware cursor plane).
//   gnome  org.gnome.Shell.Screencast over D-Bus. Any GNOME Wayland box, no root, draws the
//          pointer, but the shell encodes VP8 on the CPU. Measured here: 2.9 fps under heavy
//          full-screen noise, ~14 fps under moderate motion, 1-2 cores pegged.
//   x11    ffmpeg x11grab + libx264 ultrafast: ~3 cores for a busy screen. Last resort.
//
// Audio ("Record audio" setting) = system audio, i.e. the default sink's MONITOR
// (what you hear, matching WASAPI loopback on Windows), encoded LIVE by a user-side
// sidecar (AAC .m4a, Opus .opus for webm) and stream-copied in after stop, so delivery
// time doesn't grow with clip length. HARD-WON: the mux is END-aligned (the two streams
// start at different latencies: pkexec + kmsgrab warmup, and capkit trims the first
// second), which only works if both streams END at the same real instant. The helper's
// stop takes ~0.8-1.3 s (pkexec's polkit round trip) during which kmsgrab keeps
// capturing, so the audio sidecar is cut when the stop helper RETURNS, not when the key
// is pressed; cutting it at the press left the sound ~1.4 s late against the picture.
//
// Cues: a kms clip starts ~2-3 s after the press, so "Recording" is announced when the
// helper's first ffmpeg stats line says frames are flowing (about when the clip begins:
// the helper trims the first second), not at the press. The post-press tail (the stop helper's latency) is cut from the finished
// clip so the stop cue/banner never ends up in the video.
//
// Collision rules with capkit's own Super+Alt+R recorder (same kmsgrab helper):
//   - never start a second kmsgrab recording: if one is live (capkit's), refuse
//     and say so, instead of stacking two root ffmpegs that capkit-stop would
//     both kill. Our OWN previous recording still winding down is waited out.
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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const KMS_HELPER: &str = "/usr/local/bin/capkit-kms";
const KMS_STOP: &str = "/usr/local/bin/capkit-stop";
const FPS: u32 = 30;
/// A "recording" smaller / shorter than this is a failed one, not a clip.
const MIN_CLIP_BYTES: u64 = 4096;
const MIN_CLIP_SECS: f64 = 0.4;
/// kmsgrab's exit trails its SIGINT by about this much (encoder flush): only used when
/// the stop came from OUTSIDE (capkit's hotkey) and the audio had to be cut at the exit.
const EXT_FLUSH_SECS: f64 = 0.3;
/// Post-press footage kept at the end of a kms clip (the stop helper's own latency is
/// cut off; this is the margin so jitter never clips the moment of the press itself, and
/// small enough that the stop banner/tick, which arrive just after the press, stay out).
const TAIL_SLACK_SECS: f64 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    Hook,
    Kms,
    Gnome,
    X11,
}

/// A screen rectangle in physical pixels, as the region picker produces it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Region {
    /// "x,y,w,h" as it travels over the command port. Rejects anything degenerate.
    pub fn parse(s: &str) -> Option<Region> {
        let mut it = s.split(',').map(|v| v.trim().parse::<i32>());
        let (x, y, w, h) = (it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?);
        if it.next().is_some() || x < 0 || y < 0 || w < 16 || h < 16 {
            return None;
        }
        Some(Region { x, y, w: w as u32, h: h as u32 })
    }

    pub fn to_arg(self) -> String {
        format!("{},{},{},{}", self.x, self.y, self.w, self.h)
    }

    /// H.264 4:2:0 wants even sizes.
    fn even(self) -> Region {
        Region { w: self.w & !1, h: self.h & !1, ..self }
    }
}

/// What the UI shows. `Recording` carries the time since the CLIP began (not the press).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Status {
    Idle,
    Starting,
    Recording(Duration),
    Saving,
}

/// How a stop press tells a live session to stop. The watcher owns the real handles.
#[derive(Clone)]
enum StopHandle {
    /// SIGINT this process group (x11 ffmpeg, or the hook script's group).
    Pgid(u32),
    /// User's record-stop.sh, then SIGINT the group as a backstop.
    HookScript(PathBuf, u32),
    /// pkexec capkit-stop (passwordless via the capkit polkit rule).
    Kms,
    /// Wake the D-Bus holder thread so it calls StopScreencast (it then cuts the audio).
    Gnome(crossbeam_channel::Sender<()>),
}

/// State shared by a session's stop path, its watcher and the UI.
struct Ctl {
    backend: Backend,
    region: Option<Region>,
    /// A stop press was accepted: any exit from here on is ours.
    stopping: AtomicBool,
    stop_at: Mutex<Option<Instant>>,
    /// The stop came before capture went live: nothing useful was recorded.
    cancel: AtomicBool,
    /// The kms helper process has exited (successfully or not).
    helper_done: AtomicBool,
    /// Audio sidecar's pgid (0 = none); SIGINTed exactly once via cut_audio().
    audio_pgid: AtomicU32,
    audio_cut: AtomicBool,
    /// Set when the frames that will be in the clip are flowing.
    live: Mutex<Option<Instant>>,
    /// Seconds (ms) of post-press footage to cut from the END of the finished clip.
    tail_ms: AtomicU64,
    /// Seconds (ms) the audio ran past the video's end (external stops only).
    overrun_ms: AtomicU64,
    /// When the stop helper returned (diagnostics: how long kmsgrab lingered after it).
    stop_returned: Mutex<Option<Instant>>,
}

impl Ctl {
    fn new(backend: Backend, region: Option<Region>) -> Arc<Ctl> {
        Arc::new(Ctl {
            backend,
            region,
            stopping: AtomicBool::new(false),
            stop_at: Mutex::new(None),
            cancel: AtomicBool::new(false),
            helper_done: AtomicBool::new(false),
            audio_pgid: AtomicU32::new(0),
            audio_cut: AtomicBool::new(false),
            live: Mutex::new(None),
            tail_ms: AtomicU64::new(0),
            overrun_ms: AtomicU64::new(0),
            stop_returned: Mutex::new(None),
        })
    }

    fn is_live(&self) -> bool {
        lk(&self.live).is_some()
    }

    /// True the first time only.
    fn mark_live(&self) -> bool {
        let mut l = lk(&self.live);
        if l.is_some() {
            return false;
        }
        *l = Some(Instant::now());
        drop(l);
        changed();
        true
    }

    /// Stop the audio sidecar's capture. Idempotent: a second SIGINT would make ffmpeg
    /// abort instead of finalizing its file.
    fn cut_audio(&self) {
        let pg = self.audio_pgid.load(Ordering::SeqCst);
        if pg != 0 && !self.audio_cut.swap(true, Ordering::SeqCst) {
            sigint_group(pg);
        }
    }
}

struct Session {
    id: u64,
    stop: StopHandle,
    watcher: JoinHandle<()>,
    ctl: Arc<Ctl>,
}

struct StopJob {
    stop: StopHandle,
    ctl: Arc<Ctl>,
    press: Instant,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
/// Serializes toggle(): the listener thread and the UI menu can both call it.
static TOGGLE: Mutex<()> = Mutex::new(());
/// Sessions between "stop accepted" and "delivered"; drives Status::Saving after the slot clears.
static SAVING: AtomicUsize = AtomicUsize::new(0);
/// Our own kms recording was told to stop and its kmsgrab hasn't exited yet.
static WINDING_DOWN: AtomicBool = AtomicBool::new(false);
static CURSOR_NOTE_SHOWN: AtomicBool = AtomicBool::new(false);
static FALLBACK_NOTE_SHOWN: AtomicBool = AtomicBool::new(false);

/// Lock, recovering from poisoning (a panicked stop must not turn every future toggle
/// into a silent no-op).
fn lk<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn session() -> MutexGuard<'static, Option<Session>> {
    lk(&SESSION)
}

/// Another kmsgrab recording (capkit's) is running; start() refuses with a banner.
#[derive(Debug)]
struct Refused;
impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("another kmsgrab recording is running")
    }
}
impl std::error::Error for Refused {}

// === Public surface ===

/// Hotkey entry: first press starts a full-screen recording, second press stops.
/// Safe from any thread; never blocks on the stop helper or the finalize.
pub fn toggle() {
    toggle_region(None);
}

/// Same, but a START records only `region` (ignored when the press is a stop).
pub fn toggle_region(region: Option<Region>) {
    let _serial = lk(&TOGGLE);
    enum Action {
        Stop(StopJob),
        Ignore,
        Start,
    }
    let action = {
        let mut slot = session();
        match slot.as_ref().map(|s| !s.watcher.is_finished()) {
            // A finished watcher means the session already ended on its own (capkit's
            // hotkey stopped it, or the helper died): the press means START.
            Some(false) => {
                *slot = None;
                Action::Start
            }
            Some(true) => {
                let s = slot.as_ref().expect("checked above");
                let mut at = lk(&s.ctl.stop_at);
                let since_stop = at.map(|t| t.elapsed());
                if !s.ctl.stopping.load(Ordering::SeqCst) {
                    s.ctl.stopping.store(true, Ordering::SeqCst);
                    *at = Some(Instant::now());
                    Action::Stop(StopJob { stop: s.stop.clone(), ctl: s.ctl.clone(), press: Instant::now() })
                } else if since_stop.map(|d| d < Duration::from_millis(400)).unwrap_or(false) {
                    // A double tap on the stop key, not a new command.
                    Action::Ignore
                } else {
                    // A press while the previous recording is still being stopped/saved
                    // means "record again": start() waits out the old kmsgrab (kms_guard)
                    // and the old session finishes on its own.
                    Action::Start
                }
            }
            None => Action::Start,
        }
    };
    match action {
        Action::Ignore => {}
        // The stop helper takes ~1 s (pkexec); never make the caller (the loopback
        // listener, the UI) wait for it.
        Action::Stop(job) => {
            changed();
            std::thread::spawn(move || run_stop(&job));
        }
        Action::Start => match start(region) {
            Ok(sess) => {
                // A start can race a watcher that already finished (helper died at
                // birth): only park the session if its watcher is still running.
                if !sess.watcher.is_finished() {
                    *session() = Some(sess);
                }
                changed();
            }
            Err(e) if e.is::<Refused>() => {
                eprintln!("trontsnap: refused: another kmsgrab recording is already running (capkit's?)");
                banner(
                    "Another recording is running",
                    "Press Super+Alt+R to stop it, then try again.",
                    8,
                );
            }
            Err(e) => {
                eprintln!("trontsnap: recording unavailable: {e:#}");
                banner("Recording didn't start", &format!("{e:#}"), 8);
            }
        },
    }
}

/// Stop any live recording and wait for it to finalize. Called on app quit so
/// an exit never orphans a recorder that would run until the disk fills.
pub fn shutdown() {
    let taken = session().take();
    if let Some(sess) = taken {
        sess.ctl.stopping.store(true, Ordering::SeqCst);
        run_stop(&StopJob { stop: sess.stop.clone(), ctl: sess.ctl.clone(), press: Instant::now() });
        let _ = sess.watcher.join();
    }
    // A previous session may still be muxing (restart-then-quit): don't cut it off.
    let t0 = Instant::now();
    while SAVING.load(Ordering::SeqCst) > 0 && t0.elapsed() < Duration::from_secs(30) {
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub fn status() -> Status {
    if let Some(s) = session().as_ref() {
        if s.ctl.stopping.load(Ordering::SeqCst) {
            return Status::Saving;
        }
        return match *lk(&s.ctl.live) {
            Some(t) => Status::Recording(t.elapsed()),
            None => Status::Starting,
        };
    }
    if SAVING.load(Ordering::SeqCst) > 0 {
        Status::Saving
    } else {
        Status::Idle
    }
}

static ON_CHANGE: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// The UI registers a repaint here: egui draws only on events, and a recording starts,
/// goes live and finishes on worker threads, so without this the title-bar indicator
/// would sit stale until the user wiggled the mouse.
pub fn on_change(f: impl Fn() + Send + Sync + 'static) {
    let _ = ON_CHANGE.set(Box::new(f));
}

fn changed() {
    if let Some(f) = ON_CHANGE.get() {
        f();
    }
}

/// Is the VCE encoding a screen capture right now (ours or capkit's)? Playing a video
/// back with hardware decode meanwhile is what hung this machine's radeon once.
pub fn gpu_encoding() -> bool {
    kmsgrab_running()
}

// === Stop ===

fn run_stop(job: &StopJob) {
    let live = job.ctl.is_live();
    if !live {
        job.ctl.cancel.store(true, Ordering::SeqCst);
        banner("Cancelling recording", "Stopped before capture began.", 20);
    } else {
        // gnome/x11 stop at the press, so this lands AFTER the last frame; kms cuts its tail.
        let mut body = String::from("Finalizing the clip...");
        if matches!(job.ctl.backend, Backend::Gnome | Backend::X11) && !FALLBACK_NOTE_SHOWN.swap(true, Ordering::SeqCst) {
            body.push_str("\nFallback recorder: expect a low frame rate on a busy screen.");
        }
        banner("Saving recording", &body, 60);
    }
    crate::sound::play_tick(true);
    match &job.stop {
        StopHandle::Pgid(pg) => {
            sigint_group(*pg);
            job.ctl.cut_audio();
        }
        StopHandle::HookScript(script, pg) => {
            let _ = Command::new("bash").arg(script).status();
            sigint_group(*pg);
        }
        StopHandle::Kms => {
            WINDING_DOWN.store(true, Ordering::SeqCst);
            // Pressed before kmsgrab even exists (pkexec still in flight): the helper
            // would start it AFTER a capkit-stop that found nothing, leaving an orphan
            // recording. Wait for it to come up, then stop it.
            let t0 = Instant::now();
            while !kmsgrab_running()
                && !job.ctl.helper_done.load(Ordering::SeqCst)
                && t0.elapsed() < Duration::from_secs(25)
            {
                std::thread::sleep(Duration::from_millis(100));
            }
            if !job.ctl.helper_done.load(Ordering::SeqCst) {
                let _ = kms_cmd(KMS_STOP, "TRONTSNAP_KMS_STOP")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            *lk(&job.ctl.stop_returned) = Some(Instant::now());
            if live {
                // pkexec's round trip happens AFTER the press but BEFORE kmsgrab hears
                // the signal: that much post-press footage is in the clip. Cut it.
                let tail = (job.press.elapsed().as_secs_f64() - TAIL_SLACK_SECS).max(0.0);
                job.ctl.tail_ms.store((tail * 1000.0) as u64, Ordering::SeqCst);
            }
            // pkill has been delivered by now, so the audio ends within ~0.1 s of the video.
            job.ctl.cut_audio();
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

/// The real helpers run through pkexec. HARD-WON: a harness instance (TRONTSNAP_TEST) may
/// swap in fake scripts via TRONTSNAP_KMS_HELPER / TRONTSNAP_KMS_STOP and runs them
/// directly, so the kms code path is testable without root, the polkit rule or the GPU.
/// Outside TEST mode the variables are ignored.
fn kms_override(var: &str) -> Option<PathBuf> {
    std::env::var_os("TRONTSNAP_TEST")?;
    std::env::var_os(var).map(PathBuf::from)
}

fn kms_cmd(real: &str, var: &str) -> Command {
    match kms_override(var) {
        Some(p) => Command::new(p),
        None => {
            let mut c = Command::new("pkexec");
            c.arg(real);
            c
        }
    }
}

fn kms_available() -> bool {
    match (kms_override("TRONTSNAP_KMS_HELPER"), kms_override("TRONTSNAP_KMS_STOP")) {
        (Some(h), Some(s)) => is_exec(&h.display().to_string()) && is_exec(&s.display().to_string()),
        _ => is_exec(KMS_HELPER) && is_exec(KMS_STOP),
    }
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
    if kms_available() {
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

/// Collision guard for the kmsgrab helper. Runs BEFORE any audio starts, so a refusal
/// leaves nothing behind.
fn kms_guard() -> anyhow::Result<()> {
    if !kmsgrab_running() {
        return Ok(());
    }
    // Our OWN previous recording is still winding down (kmsgrab lingers ~1-1.5 s after
    // the stop): a quick restart waits it out instead of being refused with a message
    // that blames capkit.
    if WINDING_DOWN.load(Ordering::SeqCst) {
        for _ in 0..80 {
            std::thread::sleep(Duration::from_millis(50));
            if !kmsgrab_running() {
                return Ok(());
            }
        }
    }
    Err(Refused.into())
}

fn start(region: Option<Region>) -> anyhow::Result<Session> {
    let backend = pick_backend()?;
    let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
    let stage = stage_dir()?;
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    if backend == Backend::Kms {
        kms_guard()?;
    }
    let ctl = Ctl::new(backend, region);

    // Hook scripts own their audio; everyone else gets the monitor sidecar.
    let audio = if backend != Backend::Hook && crate::settings::record_audio() {
        start_audio(&stage, &stamp, backend == Backend::Gnome)
    } else {
        None
    };
    if let Some((c, _)) = &audio {
        ctl.audio_pgid.store(c.id(), Ordering::SeqCst);
    }

    let started = match backend {
        Backend::Hook => start_hook(&stage, &stamp, id, &ctl, audio),
        Backend::Kms => start_kms(id, &ctl, audio),
        Backend::Gnome => start_gnome(&stage, &stamp, id, &ctl, audio),
        Backend::X11 => start_x11(&stage, &stamp, id, &ctl, audio),
    };
    // start_* own the audio child from here; each one reaps it on failure.
    let (stop, watcher) = started?;
    let video_pgid = match &stop {
        StopHandle::Pgid(pg) | StopHandle::HookScript(_, pg) => Some(*pg),
        _ => None,
    };
    write_marker(backend, id, video_pgid, ctl.audio_pgid.load(Ordering::SeqCst));
    eprintln!("trontsnap: recording via {backend:?}{}", region.map(|r| format!(" region {}", r.to_arg())).unwrap_or_default());

    if backend == Backend::Kms {
        // The clip begins ~2-3 s from now. HARD-WON: no banner once it has begun: GNOME pins
        // banners top-centre INSIDE the picture (one lingered in every frame of a clip). So
        // this one is visible only during the dead time and closes the instant frames flow
        // (kms_watch), where a soft tick marks "now".
        crate::sound::play_tick(false);
        let mut body = String::from("Capture begins in about 3 seconds.");
        if crate::settings::capture_cursor() && !CURSOR_NOTE_SHOWN.swap(true, Ordering::SeqCst) {
            body.push_str("\nThe mouse pointer isn't captured by this recorder.");
        }
        banner("Starting recording", &body, 20);
    } else {
        // gnome/x11/hook start within a fraction of a second: nothing to announce, and
        // anything shown now would be in the clip. The title bar shows REC.
        ctl.mark_live();
    }
    Ok(Session { id, stop, watcher, ctl })
}

/// Capture is live (kms): the "Starting" banner goes away and a tick marks the instant.
fn announce_live() {
    banner_close();
    crate::sound::play_tick(false);
}

type Audio = Option<(Child, PathBuf)>;

/// System-audio sidecar: the default sink's monitor via pulse (PipeWire's pulse
/// shim answers @DEFAULT_MONITOR@ too), encoded live so the post-stop mux is a stream
/// copy. Best-effort: no audio beats no recording.
fn start_audio(stage: &Path, stamp: &str, webm: bool) -> Audio {
    use std::os::unix::process::CommandExt as _;
    let (ext, codec, fmt): (&str, [&str; 4], &str) = if webm {
        ("opus", ["-c:a", "libopus", "-b:a", "128k"], "opus")
    } else {
        ("m4a", ["-c:a", "aac", "-b:a", "160k"], "mp4")
    };
    let path = stage.join(format!("TrontSnap_{stamp}.audio.{ext}"));
    let child = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "pulse", "-i", "@DEFAULT_MONITOR@"])
        .args(["-ac", "2", "-ar", "48000"])
        .args(codec)
        .args(["-f", fmt])
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| eprintln!("trontsnap: audio sidecar unavailable: {e}"))
        .ok()?;
    Some((child, path))
}

type Started = anyhow::Result<(StopHandle, JoinHandle<()>)>;

fn start_hook(stage: &Path, stamp: &str, id: u64, ctl: &Arc<Ctl>, audio: Audio) -> Started {
    use std::os::unix::process::CommandExt as _;
    let raw = stage.join(format!("TrontSnap_{stamp}.mp4"));
    let region = ctl.region;
    let spawned = script_path("record-start.sh")
        .ok_or_else(|| anyhow::anyhow!("~/.config/trontsnap/record-start.sh is missing"))
        .and_then(|script| {
            let mut cmd = Command::new("bash");
            cmd.arg(&script).arg(&raw).stdin(Stdio::null()).process_group(0);
            if let Some(r) = region {
                cmd.env("TRONTSNAP_REGION", r.to_arg());
            }
            cmd.spawn().map_err(anyhow::Error::from)
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
    let ctl = ctl.clone();
    let w = std::thread::spawn(move || {
        let mut child = child;
        let _ = child.wait();
        finish(id, Some(raw), audio, ctl);
    });
    Ok((stop, w))
}

fn start_x11(stage: &Path, stamp: &str, id: u64, ctl: &Arc<Ctl>, audio: Audio) -> Started {
    use std::os::unix::process::CommandExt as _;
    let raw = stage.join(format!("TrontSnap_{stamp}.mp4"));
    let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into());
    let cursor = if crate::settings::capture_cursor() { "1" } else { "0" };
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-f", "x11grab", "-draw_mouse", cursor])
        .args(["-framerate", &FPS.to_string()]);
    // x11grab takes the region as a size + an offset glued to the display name.
    let input = match ctl.region.map(Region::even) {
        Some(r) => {
            cmd.args(["-video_size", &format!("{}x{}", r.w, r.h)]);
            format!("{display}+{},{}", r.x, r.y)
        }
        None => display,
    };
    let spawned = cmd
        .args(["-i", &input])
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
    let ctl = ctl.clone();
    let w = std::thread::spawn(move || {
        let mut child = child;
        let _ = child.wait();
        finish(id, Some(raw), audio, ctl);
    });
    Ok((StopHandle::Pgid(pg), w))
}

fn start_kms(id: u64, ctl: &Arc<Ctl>, audio: Audio) -> Started {
    let spawned = kms_cmd(KMS_HELPER, "TRONTSNAP_KMS_HELPER")
        .args([&FPS.to_string(), "24"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // The helper's ffmpeg runs with -stats: its first progress line is the
        // "frames are flowing" event kms_watch waits for.
        .stderr(Stdio::piped())
        .spawn();
    let child = match spawned {
        Ok(c) => c,
        Err(e) => {
            reap_audio(audio);
            anyhow::bail!("pkexec {KMS_HELPER} failed: {e}");
        }
    };
    let ctl = ctl.clone();
    let w = std::thread::spawn(move || kms_watch(id, child, ctl, audio));
    Ok((StopHandle::Kms, w))
}

/// Drain the helper's stderr (the pipe must never fill up and stall ffmpeg) and signal
/// once when an ffmpeg `-stats` line reports at least one frame encoded.
fn watch_stats(mut se: std::process::ChildStderr, live: crossbeam_channel::Sender<()>) {
    use std::io::Read as _;
    let mut buf = [0u8; 4096];
    let mut sent = false;
    loop {
        let n = match se.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        if sent {
            continue;
        }
        let text = String::from_utf8_lossy(&buf[..n]);
        for part in text.split("frame=").skip(1) {
            let digits: String = part.trim_start().chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.parse::<u64>().unwrap_or(0) > 0 {
                sent = true;
                let _ = live.try_send(());
                break;
            }
        }
    }
}

/// Owns the capkit helper. Announces the recording when frames really flow, notices an
/// EXTERNAL stop (capkit's Super+Alt+R) so the audio is cut at the right instant, then
/// waits out capkit's warmup-trim remux and reads its SAVED:<path> line.
fn kms_watch(id: u64, mut child: Child, ctl: Arc<Ctl>, audio: Audio) {
    let stats = child.stderr.take().map(|se| {
        let (tx, rx) = crossbeam_channel::bounded::<()>(1);
        std::thread::spawn(move || watch_stats(se, tx));
        rx
    });
    let born = Instant::now();
    let mut pid: Option<u32> = None;
    let mut seen_at: Option<Instant> = None;
    let mut frames_at: Option<Instant> = None;
    let mut gone = false;
    let mut killed = false;
    let mut last_scan = Instant::now() - Duration::from_secs(1);
    loop {
        if matches!(child.try_wait(), Ok(Some(_))) {
            break;
        }
        // HARD-WON: finding kmsgrab walks every /proc/*/cmdline (~8000 syscalls/s at
        // the old 50 ms cadence, more CPU than the encoder it supervised). Scan only
        // until it is found, then stat the one pid.
        match pid {
            None if last_scan.elapsed() >= Duration::from_millis(100) => {
                last_scan = Instant::now();
                if let Some(&p) = kmsgrab_pids().first() {
                    pid = Some(p);
                    seen_at = Some(Instant::now());
                }
            }
            Some(p) if !gone && !Path::new(&format!("/proc/{p}")).exists() => {
                gone = true;
                WINDING_DOWN.store(false, Ordering::SeqCst);
                if !ctl.stopping.load(Ordering::SeqCst) {
                    // Stopped from outside: kmsgrab's exit trails the signal by its
                    // flush, so the audio overruns the video by about that much.
                    ctl.overrun_ms.store((EXT_FLUSH_SECS * 1000.0) as u64, Ordering::SeqCst);
                    ctl.cut_audio();
                } else if let Some(t) = *lk(&ctl.stop_returned) {
                    eprintln!("trontsnap: kmsgrab exited {:.2}s after the stop helper returned", t.elapsed().as_secs_f64());
                }
            }
            _ => {}
        }
        if frames_at.is_none() {
            let flowing = stats.as_ref().map(|rx| rx.try_recv().is_ok()).unwrap_or(false);
            // No stats line (a helper variant without -stats) must not hide the cue forever.
            let assumed = seen_at.map(|t| t.elapsed() > Duration::from_secs(4)).unwrap_or(false);
            if flowing || assumed {
                frames_at = Some(Instant::now());
            }
        }
        // HARD-WON: announce on the stats line itself. The clip begins 1 s (the helper's trim)
        // after the first raw frame, and on this box the first positive `frame=` line trails
        // that frame by about as much (measured: clip began at press+2.14 s, the line arrived
        // at ~2.3 s; holding it another 0.5 s made the cue 0.7 s LATE). An early cue costs
        // nothing, a late one makes the user start before it.
        if frames_at.is_some() && !ctl.is_live() && !ctl.stopping.load(Ordering::SeqCst) && ctl.mark_live() {
            eprintln!(
                "trontsnap: kms frames flowing {:.2}s after the press ({:.2}s after kmsgrab appeared)",
                born.elapsed().as_secs_f64(),
                seen_at.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0)
            );
            announce_live();
        }
        if pid.is_none() && !killed && born.elapsed() > Duration::from_secs(15) {
            // pkexec auth refused/hung: give up rather than wait forever. (Killable:
            // until pkexec execs the helper its real uid is still ours.)
            let _ = child.kill();
            killed = true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    ctl.helper_done.store(true, Ordering::SeqCst);
    WINDING_DOWN.store(false, Ordering::SeqCst);
    ctl.cut_audio();
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
    if raw.is_none() && !ctl.cancel.load(Ordering::SeqCst) {
        banner("Recording failed", "The kmsgrab helper didn't produce a file.", 8);
    }
    finish(id, raw, audio, ctl);
}

fn start_gnome(stage: &Path, stamp: &str, id: u64, ctl: &Arc<Ctl>, audio: Audio) -> Started {
    // No extension: the shell appends the right one for whatever encoder its
    // pipeline picked (.webm for VP8, .mp4 when a hardware H.264 path exists).
    let template = stage.join(format!("TrontSnap_{stamp}")).display().to_string();
    let cursor = crate::settings::capture_cursor();
    let region = ctl.region;
    let (stop_tx, stop_rx) = crossbeam_channel::bounded::<()>(1);
    let (ready_tx, ready_rx) = crossbeam_channel::bounded::<anyhow::Result<()>>(1);
    let ctl = ctl.clone();

    // The thread HOLDS the bus connection for the whole recording: the shell
    // stops a screencast the moment its caller's connection vanishes (a bare
    // `gdbus call` records exactly one frame). Bonus: if TrontSnap crashes, the
    // recording ends instead of running forever.
    let w = std::thread::spawn(move || {
        let mut audio = audio;
        let begun = (|| -> anyhow::Result<(zbus::blocking::Connection, String)> {
            let conn = zbus::blocking::Connection::session()?;
            let proxy = gnome_proxy(&conn)?;
            let mut opts = HashMap::<&str, zbus::zvariant::Value>::new();
            opts.insert("framerate", zbus::zvariant::Value::from(FPS as i32));
            opts.insert("draw-cursor", zbus::zvariant::Value::from(cursor));
            let (ok, file): (bool, String) = match region.map(Region::even) {
                Some(r) => proxy.call(
                    "ScreencastArea",
                    &(r.x, r.y, r.w as i32, r.h as i32, template.as_str(), opts),
                )?,
                None => proxy.call("Screencast", &(template.as_str(), opts))?,
            };
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
        // HARD-WON: audio FIRST, the opposite of kms. The shell's video ends when the call
        // ARRIVES (measured: last frame = the press) but StopScreencast only returns ~0.4 s
        // later, so cutting the audio after the call left it 0.4 s long and every sound
        // 0.4 s early (rec_gnome_test.py, lag -0.24 s instead of ~+0.14 s).
        ctl.cut_audio();
        if let Ok(proxy) = gnome_proxy(&conn) {
            let _: zbus::Result<bool> = proxy.call("StopScreencast", &());
        }
        drop(conn);
        let raw = PathBuf::from(file);
        wait_settled(&raw);
        finish(id, Some(raw), audio, ctl);
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
    if let Some((mut a, path)) = audio {
        sigint_group(a.id());
        let _ = a.wait();
        let _ = std::fs::remove_file(path);
    }
}

// === Finalize ===

struct SavingGuard;
impl SavingGuard {
    fn new() -> Self {
        SAVING.fetch_add(1, Ordering::SeqCst);
        changed();
        SavingGuard
    }
}
impl Drop for SavingGuard {
    fn drop(&mut self) {
        SAVING.fetch_sub(1, Ordering::SeqCst);
        changed();
    }
}

/// A clip worth keeping: not empty, not a stub, something ffprobe can read.
fn clip_usable(p: &Path) -> bool {
    std::fs::metadata(p).map(|m| m.len() >= MIN_CLIP_BYTES).unwrap_or(false)
        && probe_duration(p).map(|d| d >= MIN_CLIP_SECS).unwrap_or(false)
}

/// Common tail for every backend: trim/mux/crop as needed, move the result into the
/// TrontSnap folder, deliver, clear the session.
fn finish(id: u64, raw: Option<PathBuf>, audio: Audio, ctl: Arc<Ctl>) {
    // Counted BEFORE the slot clears so the UI never flickers through Idle.
    let _saving = SavingGuard::new();
    // Clear our slot first: if the stop came from outside (capkit hotkey, helper
    // died), the next hotkey press must START, not try to stop a ghost.
    {
        let mut slot = session();
        if slot.as_ref().map(|s| s.id) == Some(id) {
            *slot = None;
        }
    }
    clear_marker(id);
    // Idempotent: normally the stop path already cut it. If the video recorder died on
    // its own nobody did, and waiting for a live sidecar would hang forever.
    ctl.cut_audio();
    let sidecar = audio.map(|(mut a, path)| {
        let _ = a.wait();
        path
    });

    // HARD-WON: capkit-kms `mv`s whatever ffmpeg left, even a 0-byte file when the stop
    // beat the encoder's first frame (record pressed twice quickly), and prints SAVED:
    // for it. That used to be delivered as a recording: shutter, clipboard, toast, a
    // blank tile. Anything unreadable or stub-sized is a FAILED recording.
    let had_raw = raw.as_ref().map(|p| p.exists()).unwrap_or(false);
    let usable = raw.as_ref().filter(|p| clip_usable(p)).cloned();
    let Some(raw_ok) = usable else {
        eprintln!("trontsnap: recording produced no usable file");
        if ctl.cancel.load(Ordering::SeqCst) {
            banner("Recording cancelled", "Stopped before capture began; it takes a few seconds to start.", 6);
        } else if had_raw {
            banner("Recording too short", "Nothing worth saving was captured.", 6);
        } else {
            banner_close();
        }
        // This session's own stub output (and its audio) are ours to clear.
        for p in raw.iter().chain(sidecar.iter()) {
            let _ = std::fs::remove_file(p);
        }
        return;
    };
    let raw = raw_ok;
    let out = match deliver_path(&raw) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("trontsnap: no save dir: {e:#}");
            return;
        }
    };

    let post = Post {
        tail: ctl.tail_ms.load(Ordering::SeqCst) as f64 / 1000.0,
        overrun: ctl.overrun_ms.load(Ordering::SeqCst) as f64 / 1000.0,
        // Only kms records the whole screen; the other backends crop at capture.
        region: if ctl.backend == Backend::Kms { ctl.region } else { None },
        webm: out.extension().and_then(|e| e.to_str()) == Some("webm"),
    };
    // HARD-WON: judge the sidecar by whether it PROBES, not by its size: live AAC of silence is
    // ~1 KB/s, so a 4 KB floor dropped the audio of every short recording nothing was playing in.
    let audio_file = sidecar
        .as_deref()
        .filter(|a| probe_duration(a).map(|d| d >= MIN_CLIP_SECS).unwrap_or(false));
    let needs_pass = audio_file.is_some() || post.tail > 0.0 || post.region.is_some();
    if needs_pass && post_process(&raw, audio_file, &out, &post) {
        let _ = std::fs::remove_file(&raw);
    } else if let Err(e) = move_file(&raw, &out) {
        eprintln!("trontsnap: couldn't move recording into place: {e}");
        return;
    }
    if let Some(w) = &sidecar {
        let _ = std::fs::remove_file(w);
    }

    let clipboard_ok = match crate::clipboard::set_file(&out) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("trontsnap: recording clipboard failed: {e:#}");
            false
        }
    };
    banner_close();
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

/// What the post-stop ffmpeg pass has to do beyond a plain move.
struct Post {
    /// Seconds cut from the END of the clip (the stop helper's post-press footage).
    tail: f64,
    /// Seconds the audio sidecar ran past the video's end (external stops).
    overrun: f64,
    /// Crop to this rectangle (re-encodes: kms records the full screen).
    region: Option<Region>,
    webm: bool,
}

/// Mux video + audio sidecar, align the two ENDS, cut the tail, crop. Stream-copies
/// whenever it can (the sidecar is already AAC/Opus), so it is quick at any length;
/// only a region crop re-encodes the video. Writes to a non-media temp name in the
/// stage dir (NOT the TrontSnap folder, where the desktop indexer sees it) and renames,
/// so the gallery never sees a partial file.
fn post_process(video: &Path, audio: Option<&Path>, out: &Path, p: &Post) -> bool {
    let Some(vd) = probe_duration(video) else { return false };
    let keep = if p.tail > 0.0 && vd - p.tail >= 0.5 { vd - p.tail } else { vd };
    let Ok(stage) = stage_dir() else { return false };
    let name = out.file_name().and_then(|n| n.to_str()).unwrap_or("out");
    let tmp = stage.join(format!("{name}.muxing"));

    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-i"]).arg(video);
    let mut copy_audio = true;
    let mut delay_ms = 0u64;
    if let Some(a) = audio {
        let Some(ad) = probe_duration(a) else { return false };
        // Audio seconds to drop from the front so the ends line up.
        let head = ad - p.overrun - vd;
        if head >= 0.0 {
            cmd.args(["-ss", &format!("{head:.3}")]);
        } else {
            // The audio is SHORTER than the video (it started late or died early): keep
            // it at the end where it belongs and let the front be silent. HARD-WON: not
            // `-shortest`, which truncated the VIDEO to the audio's length.
            delay_ms = (-head * 1000.0) as u64;
            copy_audio = false;
        }
        cmd.arg("-i").arg(a);
        let ext = a.extension().and_then(|e| e.to_str()).unwrap_or("");
        copy_audio &= (p.webm && ext == "opus") || (!p.webm && ext == "m4a");
    }
    cmd.args(["-map", "0:v:0"]);
    if audio.is_some() {
        cmd.args(["-map", "1:a:0"]);
    }
    match p.region {
        Some(r) => {
            let (cw, ch) = probe_dims(video).unwrap_or((r.x as u32 + r.w, r.y as u32 + r.h));
            let r = Region { w: r.w.min(cw.saturating_sub(r.x as u32)), h: r.h.min(ch.saturating_sub(r.y as u32)), ..r }.even();
            cmd.args(["-vf", &format!("crop={}:{}:{}:{},format=yuv420p", r.w, r.h, r.x, r.y)])
                .args(["-c:v", "libx264", "-preset", "ultrafast", "-crf", "23"]);
        }
        None => {
            cmd.args(["-c:v", "copy"]);
        }
    }
    if audio.is_some() {
        if copy_audio {
            cmd.args(["-c:a", "copy"]);
        } else {
            if delay_ms > 0 {
                cmd.args(["-af", &format!("adelay={delay_ms}|{delay_ms}")]);
            }
            if p.webm {
                cmd.args(["-c:a", "libopus", "-b:a", "128k"]);
            } else {
                cmd.args(["-c:a", "aac", "-b:a", "160k"]);
            }
        }
    }
    cmd.args(["-t", &format!("{keep:.3}"), "-f", if p.webm { "webm" } else { "mp4" }]);
    if !p.webm {
        cmd.args(["-movflags", "+faststart"]);
    }
    let ok = cmd.arg(&tmp).stdin(Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
    if ok && move_file(&tmp, out).is_ok() {
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

fn probe_dims(p: &Path) -> Option<(u32, u32)> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "csv=p=0"])
        .arg(p)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let (w, h) = text.split_once(',')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// rename(), falling back to copy+remove across filesystems. The copy goes via a
/// `.part` name (the desktop indexer ignores it, and so does the gallery's watcher)
/// so nobody catches it half-written.
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    let tmp = to.with_extension("part");
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

fn write_marker(backend: Backend, id: u64, video_pgid: Option<u32>, audio_pgid: u32) {
    let Some(p) = marker_path() else { return };
    let body = format!(
        "backend={backend:?}\nstart={}\nsession={id}\nvideo_pgid={}\naudio_pgid={audio_pgid}\n",
        now_epoch(),
        video_pgid.unwrap_or(0),
    );
    let _ = std::fs::write(p, body);
}

/// Remove the marker only if it still names THIS session: a quick restart writes a
/// new one, and the old session's finish must not delete it.
fn clear_marker(id: u64) {
    let Some(m) = marker_path() else { return };
    let Ok(body) = std::fs::read_to_string(&m) else { return };
    if body.lines().any(|l| l == format!("session={id}")) {
        let _ = std::fs::remove_file(m);
    }
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
            let _ = kms_cmd(KMS_STOP, "TRONTSNAP_KMS_STOP").stdin(Stdio::null()).status();
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
            if !clip_usable(p) {
                // The crash beat the first frame: a stub is not a recovered recording.
                let _ = std::fs::remove_file(p);
                continue;
            }
            if let Ok(out) = deliver_path(p) {
                if move_file(p, &out).is_ok() {
                    salvaged.push(out);
                }
            }
        } else if name.contains(".audio.") || matches!(ext, "muxing" | "moving" | "part") {
            let _ = std::fs::remove_file(p);
        }
    }
    let _ = std::fs::remove_file(&marker);
    if !salvaged.is_empty() {
        banner(
            "Recovered a recording",
            "TrontSnap closed mid-recording last time; the clip is in Pictures/TrontSnap",
            8,
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

// === Banners ===
//
// One desktop banner that start/stop/saved REPLACE (notification id), closed explicitly
// when done. HARD-WON: notify-send's -t and transient hint are ignored by GNOME Shell: a
// "Recording" banner stayed up for 5+ minutes and one left over from a previous run was
// burned into every frame of a later clip. Calls go through one worker thread so they
// land in order and the caller never waits on the bus.

enum Bn {
    Show(String, String, u64),
    Close,
}

fn banner_tx() -> &'static crossbeam_channel::Sender<Bn> {
    static TX: OnceLock<crossbeam_channel::Sender<Bn>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = crossbeam_channel::unbounded();
        std::thread::spawn(move || banner_worker(rx));
        tx
    })
}

/// Show (or replace) the TrontSnap banner; it closes itself after `secs` (0 = stays).
fn banner(summary: &str, body: &str, secs: u64) {
    let _ = banner_tx().send(Bn::Show(summary.into(), body.into(), secs));
}

fn banner_close() {
    let _ = banner_tx().send(Bn::Close);
}

fn banner_worker(rx: crossbeam_channel::Receiver<Bn>) {
    use crossbeam_channel::RecvTimeoutError;
    let mut conn: Option<zbus::blocking::Connection> = None;
    let mut id = 0u32;
    let mut deadline: Option<Instant> = None;
    let proxy = |c: &zbus::blocking::Connection| {
        zbus::blocking::Proxy::new(
            c,
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
        )
        .ok()
    };
    loop {
        let msg = match deadline {
            Some(d) => rx.recv_timeout(d.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        if conn.is_none() {
            conn = zbus::blocking::Connection::session().ok();
        }
        match msg {
            Ok(Bn::Show(summary, body, secs)) => {
                let Some(c) = conn.as_ref() else { continue };
                let Some(p) = proxy(c) else { continue };
                let mut hints = HashMap::<&str, zbus::zvariant::Value>::new();
                hints.insert("transient", zbus::zvariant::Value::from(true));
                let r: zbus::Result<u32> = p.call(
                    "Notify",
                    &("TrontSnap", id, "", summary.as_str(), body.as_str(), Vec::<&str>::new(), hints, -1i32),
                );
                id = r.unwrap_or(0);
                deadline = (secs > 0 && id != 0).then(|| Instant::now() + Duration::from_secs(secs));
            }
            Ok(Bn::Close) | Err(RecvTimeoutError::Timeout) => {
                if id != 0 {
                    if let Some(p) = conn.as_ref().and_then(proxy) {
                        let _: zbus::Result<()> = p.call("CloseNotification", &(id,));
                    }
                }
                id = 0;
                deadline = None;
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}
