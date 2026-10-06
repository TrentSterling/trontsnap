// Screen recording, Linux side: shell out to ffmpeg. X11 sessions get x11grab
// (full screen, 30fps, libx264 ultrafast + optional pulse audio); Wayland has no
// portable grab an app can just start, so a user-provided script hook fills the
// gap until portal/PipeWire wiring lands:
//
//   ~/.config/trontsnap/record-start.sh <out.mp4>   starts a recording
//   ~/.config/trontsnap/record-stop.sh              stops it (optional; default SIGINT)
//
// (On Trent's own box, capkit's kmsgrab->VAAPI one-liner is exactly what goes in
// that script.) Stop sends SIGINT so ffmpeg finalizes the moov atom — a plain
// kill() writes an unplayable file.
//
// The recorder child gets its own PROCESS GROUP and the stop signal goes to the
// whole group (`kill -INT -pgid`): when the start hook is a bash script, a
// signal to just the wrapper is NOT forwarded to the ffmpeg it spawned, and the
// wait would hang forever on a recorder that never got told to stop.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

static SESSION: Mutex<Option<(Child, PathBuf)>> = Mutex::new(None);

/// Lock SESSION, recovering from poisoning (a panicked stop must not turn every
/// future toggle into a silent no-op).
fn session() -> std::sync::MutexGuard<'static, Option<(Child, PathBuf)>> {
    SESSION.lock().unwrap_or_else(|p| p.into_inner())
}

/// Hotkey entry: first press starts a session, second press stops it and
/// delivers the MP4 (clipboard file reference + toast + shutter), same spirit
/// as the Windows recorder. Safe to call from any thread — stop runs on its own
/// thread so the caller (often the loopback listener) is never blocked on
/// ffmpeg's finalize.
pub fn toggle() {
    let mut slot = session();
    match slot.take() {
        Some((child, path)) => {
            std::thread::spawn(move || stop(child, path));
        }
        None => match start() {
            Ok(sess) => *slot = Some(sess),
            Err(e) => eprintln!("trontsnap: recording unavailable: {e:#}"),
        },
    }
}

/// Stop any live recording and wait for it to finalize. Called on app quit so
/// an exit never orphans an ffmpeg that would record until the disk fills.
pub fn shutdown() {
    let taken = session().take();
    if let Some((child, path)) = taken {
        stop(child, path);
    }
}

fn out_path() -> anyhow::Result<PathBuf> {
    let dir = crate::capture::trontsnap_dir()?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(format!(
        "TrontSnap_{}.mp4",
        chrono::Local::now().format("%Y-%m-%d_%H-%M-%S")
    )))
}

fn script_path(name: &str) -> Option<PathBuf> {
    let p = dirs::config_dir()?.join("trontsnap").join(name);
    p.exists().then_some(p)
}

fn start() -> anyhow::Result<(Child, PathBuf)> {
    use std::os::unix::process::CommandExt as _;

    let out = out_path()?;

    // User script hook wins everywhere (it's the only way on Wayland for now).
    if let Some(script) = script_path("record-start.sh") {
        let mut cmd = Command::new("bash");
        cmd.arg(script).arg(&out).process_group(0);
        let child = cmd.spawn()?;
        eprintln!("trontsnap: recording via record-start.sh -> {}", out.display());
        return Ok((child, out));
    }

    if std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland") {
        anyhow::bail!(
            "Wayland screen recording isn't wired up yet — drop a \
             ~/.config/trontsnap/record-start.sh that writes to $1, or use your \
             desktop's recorder"
        );
    }

    let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into());
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-f", "x11grab", "-framerate", "30", "-i"])
        .arg(&display);
    if crate::settings::record_audio() {
        cmd.args(["-f", "pulse", "-i", "default"]);
    }
    // ultrafast: screen content on a weak CPU; even dimensions for H.264.
    cmd.args([
        "-vf",
        "crop=trunc(iw/2)*2:trunc(ih/2)*2",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-crf",
        "28",
        "-pix_fmt",
        "yuv420p",
    ]);
    if crate::settings::record_audio() {
        cmd.args(["-c:a", "aac"]);
    }
    cmd.arg(&out).stdin(Stdio::null()).process_group(0);

    let child = cmd.spawn().map_err(|e| {
        anyhow::anyhow!("ffmpeg not found — install it for recording (`sudo apt install ffmpeg`): {e}")
    })?;
    eprintln!("trontsnap: recording {} -> {}", display, out.display());
    Ok((child, out))
}

fn stop(mut child: Child, path: PathBuf) {
    if let Some(script) = script_path("record-stop.sh") {
        let _ = Command::new("bash").arg(script).status();
    } else {
        // SIGINT to the whole process group (negative pid; the child is its own
        // group leader via process_group(0)) — reaches ffmpeg even when the
        // session was started through the bash script hook. SIGINT = ffmpeg's
        // clean shutdown; it flushes and writes the moov atom. std's
        // child.kill() is SIGKILL and would corrupt the file.
        let _ = Command::new("kill").args(["-INT", "--", &format!("-{}", child.id())]).status();
    }
    let _ = child.wait();

    if !path.exists() {
        eprintln!("trontsnap: recording produced no file");
        return;
    }
    let clipboard_ok = match crate::clipboard::set_file(&path) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("trontsnap: recording clipboard failed: {e:#}");
            false
        }
    };
    crate::sound::play_shutter();
    crate::toast::launch(&path, clipboard_ok);
    println!("recorded -> {}", path.display());
}
