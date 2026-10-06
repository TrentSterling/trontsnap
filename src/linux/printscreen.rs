// Frees the PrintScreen key from GNOME's own screenshot UI binding — the Linux
// twin of the PrintScreenKeyForSnippingEnabled registry flag. GNOME binds bare
// Print to org.gnome.shell.keybindings show-screenshot-ui; clearing that is
// what lets the TrontSnap custom keybinding (see linux/keyhook.rs) win the key.
// Everything is best-effort via `gsettings`; on non-GNOME desktops these are
// quiet no-ops and the user binds keys in their own DE settings instead.

const SCHEMA: &str = "org.gnome.shell.keybindings";
const KEY: &str = "show-screenshot-ui";

fn gsettings_get(schema: &str, key: &str) -> Option<String> {
    let out = std::process::Command::new("gsettings").args(["get", schema, key]).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn gsettings_set(schema: &str, key: &str, value: &str) -> bool {
    std::process::Command::new("gsettings")
        .args(["set", schema, key, value])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Clear GNOME's bare-Print binding if it's set. Idempotent, best-effort.
/// Returns true if it actually changed something (caller shows a one-time note).
pub fn free_printscreen_key() -> bool {
    match gsettings_get(SCHEMA, KEY) {
        Some(v) if v != "[]" && v != "@as []" => gsettings_set(SCHEMA, KEY, "[]"),
        _ => false,
    }
}

/// Whether bare Print is currently free of the desktop's own screenshot UI.
/// Unknown desktop (no gsettings / no GNOME schema) reads as "free" — we can't
/// do anything about it either way.
pub fn is_free() -> bool {
    match gsettings_get(SCHEMA, KEY) {
        Some(v) => v == "[]" || v == "@as []",
        None => true,
    }
}

/// Explicitly set the binding: `free = true` clears GNOME's Print binding;
/// `free = false` restores the stock `['Print']` (desktop owns it again).
pub fn set_free(free: bool) {
    let value = if free { "[]" } else { "['Print']" };
    if !gsettings_set(SCHEMA, KEY, value) {
        eprintln!("trontsnap: could not change the desktop Print binding (no gsettings?)");
    }
}
