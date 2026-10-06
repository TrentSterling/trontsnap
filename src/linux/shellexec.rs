// Child-process launches, Linux side. No uiAccess/integrity drama here — a
// plain Command works — but the module keeps the same shape as the Windows one
// so shared call sites don't care.

use std::process::Command;

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

/// `params` is split on whitespace; callers with paths that may contain spaces
/// should build a Command and use spawn_detached directly (toast.rs does).
pub fn run(file: &str, params: &str) -> bool {
    let mut cmd = Command::new(file);
    if !params.trim().is_empty() {
        cmd.args(params.split_whitespace());
    }
    spawn_detached(cmd)
}
