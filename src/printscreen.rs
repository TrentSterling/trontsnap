// Frees the PrintScreen key from Windows' own "PrintScreen opens Snipping Tool"
// shell binding (Win10 1809+), the same per-user, no-admin trick ShareX relies
// on. Without this, RegisterHotKey's bare-PrintScreen registration silently
// loses to the OS binding.

use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

const SUBKEY: &str = r"Control Panel\Keyboard";
const VALUE: &str = "PrintScreenKeyForSnippingEnabled";

/// Set `HKCU\Control Panel\Keyboard\PrintScreenKeyForSnippingEnabled` to 0 if it
/// isn't already. Idempotent, best-effort: never panics, only logs on error.
/// Returns true if it actually changed a non-zero value to 0 (so the caller can
/// show a one-time "PrintScreen is now free for TrontSnap" note).
pub fn free_printscreen_key() -> bool {
    match try_free() {
        Ok(changed) => changed,
        Err(e) => {
            eprintln!("trontsnap: could not free PrintScreen key: {e:#}");
            false
        }
    }
}

fn try_free() -> std::io::Result<bool> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _disp) = hkcu.create_subkey(SUBKEY)?;

    let current: u32 = key.get_value(VALUE).unwrap_or(1);
    if current == 0 {
        return Ok(false);
    }

    key.set_value(VALUE, &0u32)?;
    Ok(true)
}

/// Whether PrintScreen is currently free of Windows' Snipping Tool binding, i.e.
/// `PrintScreenKeyForSnippingEnabled == 0`. Missing value reads as "free" (0 is the
/// default this app establishes), matching `try_free`'s own fallback.
pub fn is_free() -> bool {
    let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(SUBKEY) else {
        return true;
    };
    key.get_value::<u32, _>(VALUE).unwrap_or(0) == 0
}

/// Explicitly set the binding: `free = true` writes 0 (PrintScreen free for
/// TrontSnap/RegisterHotKey); `free = false` writes 1 (Windows owns it again, e.g.
/// opens Snipping Tool). Best-effort: never panics, only logs on error.
pub fn set_free(free: bool) {
    match RegKey::predef(HKEY_CURRENT_USER).create_subkey(SUBKEY) {
        Ok((key, _)) => {
            if let Err(e) = key.set_value(VALUE, &u32::from(!free)) {
                eprintln!("trontsnap: could not set PrintScreen key binding: {e:#}");
            }
        }
        Err(e) => eprintln!("trontsnap: could not open Keyboard key: {e:#}"),
    }
}
