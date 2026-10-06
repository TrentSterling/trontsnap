// === One-shot desktop notices that really go away ===
// HARD-WON: GNOME Shell ignores notify-send's -t and the transient hint. A banner stays until the
// user next touches the mouse or keyboard (an idle desktop kept an "OCR unavailable" one for
// minutes), every later banner queues behind it, and kmsgrab records it into clips. The one thing
// it honours is an explicit CloseNotification, so: Notify over D-Bus, and a detached
// `trontsnap notice-close ID SECS` closes it. (The recorder's own banners use a worker thread
// with replace/close instead, see recorder.rs: the app is long-lived, a one-shot CLI is not.)

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

fn proxy(c: &zbus::blocking::Connection) -> zbus::Result<zbus::blocking::Proxy<'static>> {
    zbus::blocking::Proxy::new(
        c,
        "org.freedesktop.Notifications",
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
    )
}

fn notify_dbus(summary: &str, body: &str) -> Option<u32> {
    let conn = zbus::blocking::Connection::session().ok()?;
    let p = proxy(&conn).ok()?;
    let mut hints = HashMap::<&str, zbus::zvariant::Value>::new();
    hints.insert("transient", zbus::zvariant::Value::from(true));
    let id: u32 = p
        .call("Notify", &("TrontSnap", 0u32, "", summary, body, Vec::<&str>::new(), hints, -1i32))
        .ok()?;
    (id != 0).then_some(id)
}

/// Show a notice and arrange for it to be closed after `secs`. Returns its id (0 when the
/// notification bus is unreachable and plain notify-send was used instead).
pub fn show(summary: &str, body: &str, secs: u64) -> u32 {
    if let Some(id) = notify_dbus(summary, body) {
        spawn_closer(id, secs);
        return id;
    }
    // No session bus / no daemon behind it: notify-send is all that is left (and in the test rigs
    // a logging stand-in).
    let mut cmd = Command::new("notify-send");
    cmd.args(["-a", "TrontSnap", "-t", &(secs * 1000).to_string(), "-h", "int:transient:1", summary, body])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let _ = crate::shellexec::spawn_detached(cmd);
    0
}

/// `show` from a thread that must not wait on the bus (the UI thread).
pub fn show_async(summary: &'static str, body: &'static str, secs: u64) {
    std::thread::spawn(move || {
        show(summary, body, secs);
    });
}

/// Close a notice now (its delayed closer then finds nothing to close, which is harmless).
pub fn close(id: u32) {
    if id == 0 {
        return;
    }
    if let Some(p) = zbus::blocking::Connection::session().ok().and_then(|c| proxy(&c).ok()) {
        let _: zbus::Result<()> = p.call("CloseNotification", &(id,));
    }
}

fn spawn_closer(id: u32, secs: u64) {
    use std::os::unix::process::CommandExt as _;
    let Ok(exe) = std::env::current_exe() else { return };
    // A rebuilt binary shows up as ".../trontsnap (deleted)" in a long-running app.
    let exe = PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
    let mut cmd = Command::new(exe);
    cmd.args(["notice-close", &id.to_string(), &secs.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    let _ = crate::shellexec::spawn_detached(cmd);
}

/// `trontsnap notice-close ID SECS`: the detached half of `show`.
pub fn run_closer() {
    let mut a = std::env::args().skip(2);
    let (Some(id), Some(secs)) = (a.next().and_then(|v| v.parse::<u32>().ok()), a.next().and_then(|v| v.parse::<u64>().ok()))
    else {
        return;
    };
    std::thread::sleep(Duration::from_secs(secs));
    close(id);
}
