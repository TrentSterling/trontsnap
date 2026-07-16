// Persisted user settings. Backed by the same HKCU\Software\TrontSnap registry key
// autostart already uses (winreg is a dependency), so there's no config file to manage.
// Values are mirrored into process-global atomics so the capture threads read them
// without touching the registry on the hot path.

use std::sync::atomic::{AtomicBool, Ordering};

use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
use winreg::RegKey;

const APP_KEY: &str = r"Software\TrontSnap";
const CAPTURE_CURSOR_VALUE: &str = "CaptureCursor";
const RECORD_AUDIO_VALUE: &str = "RecordAudio";

// Default ON: include the mouse cursor in captures. ShareX shows it and Trent asked
// for it; the tray toggle turns it off.
static CAPTURE_CURSOR: AtomicBool = AtomicBool::new(true);
// Default ON: recordings include system audio (WASAPI loopback of what you hear).
static RECORD_AUDIO: AtomicBool = AtomicBool::new(true);

/// Load persisted settings into the atomics. Call once at process start — every mode
/// captures (the one-shot `full` / `region` launches too), so all of them need it.
pub fn load() {
    if let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(APP_KEY, KEY_READ) {
        // A missing value keeps the default (true); only an explicit stored value flips it.
        if let Ok(v) = key.get_value::<u32, _>(CAPTURE_CURSOR_VALUE) {
            CAPTURE_CURSOR.store(v != 0, Ordering::Relaxed);
        }
        if let Ok(v) = key.get_value::<u32, _>(RECORD_AUDIO_VALUE) {
            RECORD_AUDIO.store(v != 0, Ordering::Relaxed);
        }
    }
}

pub fn capture_cursor() -> bool {
    CAPTURE_CURSOR.load(Ordering::Relaxed)
}

pub fn set_capture_cursor(on: bool) {
    CAPTURE_CURSOR.store(on, Ordering::Relaxed);
    persist(CAPTURE_CURSOR_VALUE, on);
}

pub fn record_audio() -> bool {
    RECORD_AUDIO.load(Ordering::Relaxed)
}

pub fn set_record_audio(on: bool) {
    RECORD_AUDIO.store(on, Ordering::Relaxed);
    persist(RECORD_AUDIO_VALUE, on);
}

fn persist(value: &str, on: bool) {
    if let Ok((key, _)) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(APP_KEY) {
        let _ = key.set_value(value, &u32::from(on));
    }
}
