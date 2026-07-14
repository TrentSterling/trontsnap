// Run-at-login via the HKCU Run key. The autostart entry launches us hidden
// (`--startup`) so login is quiet — the app just appears in the tray.

use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
use winreg::RegKey;

const RUN_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "TrontSnap";
const APP_KEY: &str = r"Software\TrontSnap";
const INIT_VALUE: &str = "AutostartInit";

fn command() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(format!("\"{}\" --startup", exe.display()))
}

pub fn is_enabled() -> bool {
    let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_PATH, KEY_READ) else {
        return false;
    };
    key.get_value::<String, _>(VALUE).is_ok()
}

pub fn enable() -> anyhow::Result<()> {
    let cmd = command().ok_or_else(|| anyhow::anyhow!("no exe path"))?;
    let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_PATH, KEY_WRITE)?;
    key.set_value(VALUE, &cmd)?;
    Ok(())
}

pub fn disable() -> anyhow::Result<()> {
    let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_PATH, KEY_WRITE)?;
    let _ = key.delete_value(VALUE);
    Ok(())
}

/// Turn autostart on exactly once — the first time TrontSnap ever runs. After
/// that we never touch it automatically, so the tray "Start at login" toggle
/// sticks across restarts (a plain enable-if-missing would silently re-enable it).
pub fn ensure_first_run() {
    let already = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(APP_KEY, KEY_READ)
        .and_then(|k| k.get_value::<u32, _>(INIT_VALUE))
        .is_ok();
    if already {
        return;
    }
    let _ = enable();
    if let Ok((key, _)) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(APP_KEY) {
        let _ = key.set_value(INIT_VALUE, &1u32);
    }
}

pub fn set(enabled: bool) {
    let _ = if enabled { enable() } else { disable() };
}
