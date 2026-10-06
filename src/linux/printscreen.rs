// Frees the PrintScreen key from GNOME's own screenshot UI binding — the Linux
// twin of the PrintScreenKeyForSnippingEnabled registry flag. GNOME binds bare
// Print to org.gnome.shell.keybindings show-screenshot-ui; clearing that is
// what lets the TrontSnap custom keybinding (see linux/keyhook.rs) win the key.
// Everything is best-effort via `gsettings`; on non-GNOME desktops these are
// quiet no-ops and the user binds keys in their own DE settings instead.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

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

// === Cached state ===
//
// HARD-WON: the Settings tab asks is_free() every frame, and each ask was a synchronous
// `gsettings get` (10-20 ms on this box's 4 weak cores): 36 spawns over 20 pointer moves and a
// Settings page at half frame rate. The answer now lives in an atomic; only a stale read
// (and the first one) touches gsettings, and the stale refresh runs on its own thread.
const UNREAD: u8 = 0;
const FREE: u8 = 1;
const OWNED: u8 = 2;
static STATE: AtomicU8 = AtomicU8::new(UNREAD);
static LAST_READ_MS: AtomicU64 = AtomicU64::new(0);
static REFRESHING: AtomicBool = AtomicBool::new(false);
/// Re-read at most this often while someone is looking (the user can change it in GNOME Settings).
const STALE_MS: u64 = 5000;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn is_cleared(v: &str) -> bool {
    v == "[]" || v == "@as []"
}

fn store(state: u8) {
    STATE.store(state, Ordering::Relaxed);
    LAST_READ_MS.store(now_ms(), Ordering::Relaxed);
}

fn read_state() -> u8 {
    match gsettings_get(SCHEMA, KEY) {
        Some(v) if is_cleared(&v) => FREE,
        Some(_) => OWNED,
        None => FREE, // no GNOME schema: nothing to free, nothing we can change
    }
}

/// Clear GNOME's bare-Print binding if it's set. Idempotent, best-effort.
/// Returns true if it actually changed something (caller shows a one-time note).
pub fn free_printscreen_key() -> bool {
    match gsettings_get(SCHEMA, KEY) {
        Some(v) if !is_cleared(&v) => {
            let changed = gsettings_set(SCHEMA, KEY, "[]");
            store(if changed { FREE } else { OWNED });
            changed
        }
        Some(_) => {
            store(FREE);
            false
        }
        None => {
            store(FREE);
            false
        }
    }
}

/// Whether bare Print is currently free of the desktop's own screenshot UI.
/// Unknown desktop (no gsettings / no GNOME schema) reads as "free" — we can't
/// do anything about it either way. Cheap enough to call every frame.
pub fn is_free() -> bool {
    let st = STATE.load(Ordering::Relaxed);
    if st == UNREAD {
        let st = read_state();
        store(st);
        return st == FREE;
    }
    if now_ms().saturating_sub(LAST_READ_MS.load(Ordering::Relaxed)) > STALE_MS
        && !REFRESHING.swap(true, Ordering::SeqCst)
    {
        std::thread::spawn(|| {
            store(read_state());
            REFRESHING.store(false, Ordering::SeqCst);
        });
    }
    st == FREE
}

/// Explicitly set the binding: `free = true` clears GNOME's Print binding;
/// `free = false` restores the stock `['Print']` (desktop owns it again).
pub fn set_free(free: bool) {
    let value = if free { "[]" } else { "['Print']" };
    if gsettings_set(SCHEMA, KEY, value) {
        store(if free { FREE } else { OWNED });
    } else {
        eprintln!("trontsnap: could not change the desktop Print binding (no gsettings?)");
    }
}
