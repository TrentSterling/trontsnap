// Global hotkeys, Linux side. There is no portable in-process RegisterHotKey on
// Wayland, so TrontSnap doesn't try: the desktop's own keybinding system runs
// the one-shot CLI modes instead (`trontsnap full` / `region` / `ocr`), and the
// record toggle pokes the running app over the existing loopback command port
// (48761 by default, TRONTSNAP_PORT overrides — the same vocabulary
// trontsnap-hotkeys.exe uses on Windows).
//
// install() therefore (best-effort, GNOME only) writes four custom keybindings
// via gsettings, then returns a receiver that never fires — the real events
// arrive as processes/port writes, not channel messages. On other desktops the
// user binds the same commands in their DE's keyboard settings; SETUP.md lists
// them.

use crossbeam_channel::Receiver;

/// What a hotkey meant. Shared vocabulary with app.rs's dispatcher and the
/// loopback command channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Full,
    Region,
    Record,
    Ocr,
}

pub struct KeyboardHook;

const MEDIA_KEYS: &str = "org.gnome.settings-daemon.plugins.media-keys";
const KB_PATH: &str = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings";

fn gset(schema_path: &str, key: &str, value: &str) -> bool {
    std::process::Command::new("gsettings")
        .args(["set", schema_path, key, value])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn gget(schema: &str, key: &str) -> Option<String> {
    let out = std::process::Command::new("gsettings")
        .args(["get", schema, key])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Write one GNOME custom keybinding slot.
fn write_bind(slot: &str, name: &str, command: &str, binding: &str) {
    let path = format!("{KB_PATH}/{slot}/");
    let schema_path = format!("{MEDIA_KEYS}.custom-keybinding:{path}");
    gset(&schema_path, "name", &format!("'{name}'"));
    gset(&schema_path, "command", &format!("'{command}'"));
    gset(&schema_path, "binding", &format!("'{binding}'"));
}

/// Install the four TrontSnap keybindings into GNOME (idempotent — slots are
/// keyed by name, re-running just rewrites the same values). Quietly does
/// nothing on desktops without gsettings/GNOME.
fn install_gnome_binds() {
    // HARD-WON: harness/test instances must never touch a real session's
    // keybindings (dbus-run-session isolates gsettings, this is the backstop).
    if std::env::var_os("TRONTSNAP_TEST").is_some() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let exe = exe.display().to_string();
    let Some(current) = gget(MEDIA_KEYS, "custom-keybindings") else { return };

    let slots = [
        ("trontsnap-full", format!("{exe} full"), "Print"),
        ("trontsnap-region", format!("{exe} region"), "<Ctrl>Print"),
        // Record must reach the RUNNING app (the toggle state lives there), so it
        // goes over the loopback command port instead of a one-shot process.
        // DOUBLE quotes inside: the whole command gets wrapped in single quotes
        // for GVariant below, and nested single quotes made `gsettings set`
        // reject the value — silently, leaving the bind with no command at all.
        (
            "trontsnap-record",
            format!(
                "bash -c \"echo -n record > /dev/tcp/127.0.0.1/{}\"",
                crate::app::port()
            ),
            "<Ctrl><Shift>Print",
        ),
        ("trontsnap-ocr", format!("{exe} ocr"), "<Ctrl><Alt>Print"),
    ];

    // Merge our slot paths into the existing list without disturbing other apps'.
    let mut paths: Vec<String> = current
        .trim_start_matches("@as")
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(|s| s.trim().trim_matches('\'').to_string())
        .filter(|s| !s.is_empty())
        .collect();
    for (slot, _, _) in &slots {
        let p = format!("{KB_PATH}/{slot}/");
        if !paths.contains(&p) {
            paths.push(p);
        }
    }
    let list = format!(
        "[{}]",
        paths.iter().map(|p| format!("'{p}'")).collect::<Vec<_>>().join(", ")
    );
    if !gset(MEDIA_KEYS, "custom-keybindings", &list) {
        return; // not GNOME; nothing else will work either
    }
    for (slot, command, binding) in &slots {
        write_bind(slot, slot, command, binding);
    }
    eprintln!("trontsnap: GNOME keybindings installed (Print / Ctrl+Print / Ctrl+Shift+Print / Ctrl+Alt+Print)");
}

impl KeyboardHook {
    /// Set up desktop keybindings (GNOME, best-effort) and return a receiver.
    /// The receiver never yields — on Linux the binds run one-shot processes and
    /// poke the loopback port rather than message this process directly.
    pub fn install() -> anyhow::Result<(Self, Receiver<HotkeyEvent>)> {
        install_gnome_binds();
        let (_tx, rx) = crossbeam_channel::unbounded();
        // _tx drops here: rx.recv() errors immediately and the consumer thread
        // in app.rs exits — exactly the dormant behavior we want.
        Ok((Self, rx))
    }
}

/// Rebind notification from the Settings UI. The Linux binds are fixed Print-key
/// chords installed above (the VK/modifier model in settings.rs is Windows
/// vocabulary); re-assert them so at least a wiped slot heals.
pub fn reload() {
    install_gnome_binds();
}
