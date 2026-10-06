// Global hotkeys, Linux side. There is no portable in-process RegisterHotKey on
// Wayland, so TrontSnap doesn't try: the desktop's own keybinding system runs
// the one-shot CLI modes instead (`trontsnap full` / `region` / `ocr`), and the
// record toggle pokes the running app over the existing loopback command port
// (48761 by default, TRONTSNAP_PORT overrides — the same vocabulary
// trontsnap-hotkeys.exe uses on Windows).
//
// install() therefore (best-effort, GNOME only) writes five custom keybindings
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

// === Accelerators ===
//
// The Settings > Hotkeys UI is shared with Windows and stores Windows vocabulary
// (MOD_ALT=1 / CONTROL=2 / SHIFT=4 / WIN=8 bits + a virtual-key code), so the GNOME
// accelerator is derived from that pair instead of a second, Linux-only setting.

/// Windows VK -> X keysym name, for exactly the keys the Settings combo offers.
fn keysym(vk: u32) -> Option<String> {
    Some(
        match vk {
            0x2C => "Print",
            0x13 => "Pause",
            0x91 => "Scroll_Lock",
            0x2D => "Insert",
            0x24 => "Home",
            0x23 => "End",
            0x21 => "Page_Up",
            0x22 => "Page_Down",
            0x70..=0x7B => return Some(format!("F{}", vk - 0x6F)),
            _ => return None,
        }
        .to_string(),
    )
}

/// GNOME accelerator ("<Ctrl><Shift>Print") for a stored (modifier bits, VK) pair. An
/// unknown VK falls back to Print, the same display fallback the Settings combo uses.
pub fn accel(mods: u32, vk: u32) -> String {
    let mut s = String::new();
    for (bit, name) in [(8, "<Super>"), (2, "<Ctrl>"), (4, "<Shift>"), (1, "<Alt>")] {
        if mods & bit != 0 {
            s.push_str(name);
        }
    }
    s.push_str(&keysym(vk).unwrap_or_else(|| "Print".into()));
    s
}

/// The record-region slot has no Settings row of its own: it is the Record chord plus Alt.
/// None when Record already uses Alt (there is no free modifier left to add; the Capture menu
/// still has the item).
pub fn record_region_accel(rec_mods: u32, vk: u32) -> Option<String> {
    (rec_mods & 1 == 0).then(|| accel(rec_mods | 1, vk))
}

/// Write one GNOME custom keybinding slot.
fn write_bind(slot: &str, name: &str, command: &str, binding: &str) {
    let path = format!("{KB_PATH}/{slot}/");
    let schema_path = format!("{MEDIA_KEYS}.custom-keybinding:{path}");
    gset(&schema_path, "name", &format!("'{name}'"));
    gset(&schema_path, "command", &format!("'{command}'"));
    gset(&schema_path, "binding", &format!("'{binding}'"));
}

/// Install the five TrontSnap keybindings into GNOME (idempotent — slots are
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
    let port_env = match crate::app::port() {
        p if p == crate::app::DEFAULT_PORT => String::new(),
        p => format!("env TRONTSNAP_PORT={p} "),
    };

    use crate::settings::{hotkey, HotkeyAction};
    let (full, region, record, ocr) = (
        hotkey(HotkeyAction::Full),
        hotkey(HotkeyAction::Region),
        hotkey(HotkeyAction::Record),
        hotkey(HotkeyAction::Ocr),
    );
    let mut full_accel = accel(full.0, full.1);
    // Opted out of "Use the PrintScreen key" = the desktop owns bare Print again; a second
    // binding on the same key would just fight it.
    if full_accel == "Print" && crate::settings::print_key_opt_out() {
        full_accel.clear();
    }
    let slots = [
        ("trontsnap-full", format!("{exe} full"), full_accel),
        ("trontsnap-region", format!("{exe} region"), accel(region.0, region.1)),
        // Record must reach the RUNNING app (the toggle state lives there): `record`
        // hands it one command over the loopback port, and STARTS the app when it is
        // not running (the old bare /dev/tcp echo failed silently in that case).
        // HARD-WON: a keybinding command doesn't inherit our environment, so a
        // non-default port (the harness) has to be spelled out in the command.
        ("trontsnap-record", format!("{port_env}{exe} record"), accel(record.0, record.1)),
        (
            "trontsnap-record-region",
            format!("{port_env}{exe} record region"),
            record_region_accel(record.0, record.1).unwrap_or_default(),
        ),
        ("trontsnap-ocr", format!("{exe} ocr"), accel(ocr.0, ocr.1)),
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
    let shown: Vec<&str> = slots.iter().map(|s| s.2.as_str()).collect();
    eprintln!("trontsnap: GNOME keybindings installed ({})", shown.join(" / "));
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

/// Rebind notification from the Settings UI: rewrite the GNOME slots from the stored hotkeys.
/// HARD-WON: install_gnome_binds is ~22 `gsettings` spawns, which froze the UI for ~1.5 s per
/// checkbox click when it ran inline; it runs on a worker, and edits inside the debounce window
/// (a ctrl+shift+alt sequence is three clicks) collapse into one install of the latest values.
pub fn reload() {
    static PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if PENDING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(600));
        PENDING.store(false, std::sync::atomic::Ordering::SeqCst);
        install_gnome_binds();
    });
}

#[cfg(test)]
mod tests {
    use super::{accel, record_region_accel};

    #[test]
    fn defaults_keep_the_original_chords() {
        // DEFAULT_*_MODS from settings.rs, all on VK_SNAPSHOT.
        assert_eq!(accel(0, 0x2C), "Print");
        assert_eq!(accel(2, 0x2C), "<Ctrl>Print");
        assert_eq!(accel(2 | 4, 0x2C), "<Ctrl><Shift>Print");
        assert_eq!(accel(2 | 1, 0x2C), "<Ctrl><Alt>Print");
        assert_eq!(record_region_accel(2 | 4, 0x2C).as_deref(), Some("<Ctrl><Shift><Alt>Print"));
    }

    #[test]
    fn other_keys_and_super() {
        assert_eq!(accel(8, 0x7B), "<Super>F12");
        assert_eq!(accel(0, 0x70), "F1");
        assert_eq!(accel(4, 0x91), "<Shift>Scroll_Lock");
        assert_eq!(accel(2, 0x22), "<Ctrl>Page_Down");
        assert_eq!(accel(0, 0xFF), "Print", "unknown VK shows (and binds) as Print");
    }

    #[test]
    fn record_region_needs_a_free_alt() {
        assert_eq!(record_region_accel(2 | 1, 0x2C), None);
    }
}
