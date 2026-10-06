// `trontsnap record [region]`: the command the GNOME record keybindings run.
//
// The recorder's toggle state lives in the running app, so the CLI's job is to hand the
// app one command over its loopback port. HARD-WON: the binding used to be a bare
// `bash -c "echo -n record > /dev/tcp/..."`, which with no app running (an XWayland
// restart takes the app down with it) failed in 5 ms, silently: the hotkey did nothing
// and nothing said why. Now a refused connection starts the app and delivers the
// command anyway. That works because app::run() binds the listener BEFORE eframe starts,
// so the kernel queues the bytes until the command thread exists.
//
// `record region` runs the freeze-frame picker here, on this process's main thread
// (winit allows one event loop per process, and the app's is the gallery's), then sends
// `record x,y,w,h`. A cold start is by definition a START: nothing was recording.

use std::io::{Read as _, Write as _};
use std::net::{Ipv4Addr, TcpStream};
use std::time::{Duration, Instant};

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let payload = match args.first().map(String::as_str) {
        None => "record".to_string(),
        Some("region") => {
            // The picker captures with the user's settings (cursor, ...).
            crate::settings::load();
            // A press while a recording runs must STOP it, not open a picker over it
            // (HARD-WON: the picker was recorded into the clip, and the stop only came after
            // the user dragged a rectangle). Only the app knows, so ask it first.
            if app_says_stop() {
                return send("record");
            }
            match crate::region_win32::pick_rect() {
                Some(r) => format!("record {}", r.to_arg()),
                None => return Ok(()), // Esc: nothing to do
            }
        }
        Some(other) => anyhow::bail!("unknown record mode '{other}' (use: record | record region)"),
    };
    send(&payload)
}

/// Spawn `trontsnap record region` as its own process (the picker needs a main thread).
pub fn spawn_region() {
    let Ok(exe) = std::env::current_exe() else { return };
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["record", "region"]);
    if !crate::shellexec::spawn_detached(cmd) {
        eprintln!("trontsnap: could not spawn the record-region picker");
    }
}

fn connect() -> Option<TcpStream> {
    TcpStream::connect_timeout(
        &(Ipv4Addr::LOCALHOST, crate::app::port()).into(),
        Duration::from_millis(500),
    )
    .ok()
}

/// Ask the running app whether a press now would STOP a recording ("stop") or start one ("pick").
/// No app, no answer, or an app too old to know the question: not a stop (the picker opens).
fn app_says_stop() -> bool {
    let Some(mut s) = connect() else { return false };
    if s.write_all(b"recstate").is_err() {
        return false;
    }
    let _ = s.set_read_timeout(Some(Duration::from_millis(800)));
    let mut buf = [0u8; 16];
    matches!(s.read(&mut buf), Ok(n) if &buf[..n] == b"stop")
}

fn send(payload: &str) -> anyhow::Result<()> {
    if let Some(mut s) = connect() {
        s.write_all(payload.as_bytes())?;
        return Ok(());
    }
    // No app: start it (detached, its own session so it outlives this process and the
    // keybinding's shell), tell the user why nothing happens for a few seconds, retry.
    let starting = notify("TrontSnap is starting", "Recording begins a few seconds after it is up.");
    spawn_app()?;
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(25) {
        if let Some(mut s) = connect() {
            s.write_all(payload.as_bytes())?;
            // Gone the moment the app has the command: a banner left up would be in the clip's
            // first frames (the app's own "Starting recording" banner takes over).
            crate::notice::close(starting);
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    crate::notice::close(starting);
    notify("Recording didn't start", "TrontSnap did not come up. See ~/.cache/trontsnap/app.log");
    anyhow::bail!("the app did not start listening on port {}", crate::app::port())
}

fn spawn_app() -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt as _;
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--startup").stdin(std::process::Stdio::null()).process_group(0);
    // Same place the lab's relaunch recipe logs to, so a crash on a cold start is findable.
    let log = dirs::cache_dir().map(|d| d.join("trontsnap")).and_then(|d| {
        std::fs::create_dir_all(&d).ok()?;
        std::fs::OpenOptions::new().create(true).append(true).open(d.join("app.log")).ok()
    });
    match log {
        Some(f) => {
            cmd.stdout(f.try_clone()?).stderr(f);
        }
        None => {
            cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        }
    }
    if !crate::shellexec::spawn_detached(cmd) {
        anyhow::bail!("could not start the app");
    }
    Ok(())
}

fn notify(summary: &str, body: &str) -> u32 {
    crate::notice::show(summary, body, 8)
}
