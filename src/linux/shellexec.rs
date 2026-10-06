// Child-process launches, Linux side. No uiAccess/integrity drama here — a
// plain Command works. The Windows twin's string-params `run` has no Linux caller
// (a whitespace-split argv cannot carry a quoted path), so callers build a Command.

use std::path::Path;
use std::process::{Command, Stdio};

/// Spawn fire-and-forget WITH a reaper thread: a dropped Child is a zombie until
/// the parent waits, and the persistent app spawns one child per capture/toast/
/// shutter for its whole lifetime. Every detached launch goes through here.
pub fn spawn_detached(mut cmd: Command) -> bool {
    match cmd.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            true
        }
        Err(_) => false,
    }
}

/// Open a file in the user's default app. HARD-WON: `opener::open` spawns xdg-open and
/// drops the Child, so every Open left a zombie for the life of the app; this one is
/// reaped. Returns false if xdg-open could not be spawned (caller may try opener).
pub fn open(path: &Path) -> bool {
    // HARD-WON: on this box's radeon (Mullins) hardware video decode (VLC/mpv's default
    // VA-API, the UVD block) while kmsgrab encodes on the VCE hung the GPU ("UVD not
    // responding", then the power-state change deadlocked) and took the desktop with it.
    // Opening a clip from the gallery/toast mid-recording is exactly that overlap, so wait.
    if crate::index::is_video(path)
        && std::env::var_os("TRONTSNAP_OPEN_WHILE_RECORDING").is_none()
        && crate::recorder::gpu_encoding()
    {
        crate::notice::show_async(
            "Not opening the video yet",
            "A hardware recording is running; playing a video now can hang this GPU. Stop the recording first.",
            8,
        );
        return true;
    }
    let mut cmd = Command::new("xdg-open");
    cmd.arg(path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    spawn_detached(cmd)
}
