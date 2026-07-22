// Persisted user settings. Backed by the same HKCU\Software\TrontSnap registry key
// autostart already uses (winreg is a dependency), so there's no config file to manage.
// Values are mirrored into process-global atomics so the capture threads read them
// without touching the registry on the hot path.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{LazyLock, RwLock};

use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
use winreg::RegKey;

const APP_KEY: &str = r"Software\TrontSnap";
const CAPTURE_CURSOR_VALUE: &str = "CaptureCursor";
const RECORD_AUDIO_VALUE: &str = "RecordAudio";
const SHOW_ABOUT_VALUE: &str = "ShowAboutOnLaunch";
const HAS_RUN_VALUE: &str = "HasRun";
const LOUPE_SIZE_VALUE: &str = "LoupeSize";
const HOTKEY_FULL_MODS_VALUE: &str = "HotkeyFullMods";
const HOTKEY_FULL_VK_VALUE: &str = "HotkeyFullVk";
const HOTKEY_REGION_MODS_VALUE: &str = "HotkeyRegionMods";
const HOTKEY_REGION_VK_VALUE: &str = "HotkeyRegionVk";
const HOTKEY_RECORD_MODS_VALUE: &str = "HotkeyRecordMods";
const HOTKEY_RECORD_VK_VALUE: &str = "HotkeyRecordVk";
const THEME_NAME_VALUE: &str = "ThemeName";
const THEME_SOURCE_VALUE: &str = "ThemeSource";

// On-screen size (px) of the region-picker magnifier loupe, scrollwheel-adjustable
// during a pick. 132 is the original fixed size; region_win32 clamps to its own
// LOUPE_MIN/LOUPE_MAX on use, so this only needs a defensive sanity bound.
static LOUPE_SIZE: AtomicI32 = AtomicI32::new(132);

// Default ON: include the mouse cursor in captures. ShareX shows it and Trent asked
// for it; the tray toggle turns it off.
static CAPTURE_CURSOR: AtomicBool = AtomicBool::new(true);
// Default ON: recordings include system audio (WASAPI loopback of what you hear).
static RECORD_AUDIO: AtomicBool = AtomicBool::new(true);
// Default OFF (opt-in): whether to open on the About tab on EVERY launch. The very first
// run always shows About regardless (see HAS_RUN); this only governs repeat launches, via
// the "Show this tab when TrontSnap starts" checkbox. Most people want Gallery on launch.
static SHOW_ABOUT: AtomicBool = AtomicBool::new(false);
// Whether TrontSnap has ever run on this machine. Unset on a fresh install -> the first
// launch opens on About (welcome + author credit), then this flips true forever.
static HAS_RUN: AtomicBool = AtomicBool::new(false);

/// Which of the three global hotkeys a bind belongs to. Used both to key
/// settings::hotkey()/set_hotkey() and by keyhook::register_all() to look up
/// each RegisterHotKey id's current (modifiers, vk) pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    Full,
    Region,
    Record,
}

// Persisted (modifiers, virtual-key) pairs for the three rebindable hotkeys. The
// modifier values are the raw HOT_KEY_MODIFIERS bits (MOD_ALT=0x1, MOD_CONTROL=0x2,
// MOD_SHIFT=0x4, MOD_WIN=0x8) WITHOUT MOD_NOREPEAT: keyhook::register_all() ORs
// that in itself at RegisterHotKey time. VK_SNAPSHOT (0x2C) is PrintScreen for all
// three defaults, matching the original fixed bindings.
static HOTKEY_FULL_MODS: AtomicU32 = AtomicU32::new(0x0000);
static HOTKEY_FULL_VK: AtomicU32 = AtomicU32::new(0x2C);
static HOTKEY_REGION_MODS: AtomicU32 = AtomicU32::new(0x0002);
static HOTKEY_REGION_VK: AtomicU32 = AtomicU32::new(0x2C);
static HOTKEY_RECORD_MODS: AtomicU32 = AtomicU32::new(0x0002 | 0x0004);
static HOTKEY_RECORD_VK: AtomicU32 = AtomicU32::new(0x2C);

// Persisted theme selection: a name ("Cyan" = the hardcoded default built-in,
// a premade palette name, "Custom" for a picked accent, or "Random <flavor>")
// plus the source hex list it was derived from (empty for "Cyan"). Mirrors
// theme::resolve()'s inputs exactly, so a restart reproduces the same theme.
static THEME_NAME: LazyLock<RwLock<String>> = LazyLock::new(|| RwLock::new("Cyan".to_string()));
static THEME_SOURCE: LazyLock<RwLock<Vec<String>>> = LazyLock::new(|| RwLock::new(Vec::new()));

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
        if let Ok(v) = key.get_value::<u32, _>(SHOW_ABOUT_VALUE) {
            SHOW_ABOUT.store(v != 0, Ordering::Relaxed);
        }
        if let Ok(v) = key.get_value::<u32, _>(HAS_RUN_VALUE) {
            HAS_RUN.store(v != 0, Ordering::Relaxed);
        }
        if let Ok(v) = key.get_value::<u32, _>(LOUPE_SIZE_VALUE) {
            LOUPE_SIZE.store((v as i32).clamp(32, 2000), Ordering::Relaxed);
        }
        // Hotkey binds: a missing value keeps the compiled-in default for that field.
        if let Ok(v) = key.get_value::<u32, _>(HOTKEY_FULL_MODS_VALUE) {
            HOTKEY_FULL_MODS.store(v, Ordering::Relaxed);
        }
        if let Ok(v) = key.get_value::<u32, _>(HOTKEY_FULL_VK_VALUE) {
            HOTKEY_FULL_VK.store(v, Ordering::Relaxed);
        }
        if let Ok(v) = key.get_value::<u32, _>(HOTKEY_REGION_MODS_VALUE) {
            HOTKEY_REGION_MODS.store(v, Ordering::Relaxed);
        }
        if let Ok(v) = key.get_value::<u32, _>(HOTKEY_REGION_VK_VALUE) {
            HOTKEY_REGION_VK.store(v, Ordering::Relaxed);
        }
        if let Ok(v) = key.get_value::<u32, _>(HOTKEY_RECORD_MODS_VALUE) {
            HOTKEY_RECORD_MODS.store(v, Ordering::Relaxed);
        }
        if let Ok(v) = key.get_value::<u32, _>(HOTKEY_RECORD_VK_VALUE) {
            HOTKEY_RECORD_VK.store(v, Ordering::Relaxed);
        }
        if let Ok(v) = key.get_value::<String, _>(THEME_NAME_VALUE) {
            *THEME_NAME.write().unwrap() = v;
        }
        if let Ok(v) = key.get_value::<String, _>(THEME_SOURCE_VALUE) {
            let list: Vec<String> =
                if v.is_empty() { Vec::new() } else { v.split(',').map(|s| s.trim().to_string()).collect() };
            *THEME_SOURCE.write().unwrap() = list;
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

pub fn show_about_on_launch() -> bool {
    SHOW_ABOUT.load(Ordering::Relaxed)
}

pub fn set_show_about_on_launch(on: bool) {
    SHOW_ABOUT.store(on, Ordering::Relaxed);
    persist(SHOW_ABOUT_VALUE, on);
}

pub fn loupe_size() -> i32 {
    LOUPE_SIZE.load(Ordering::Relaxed)
}

pub fn set_loupe_size(px: i32) {
    let clamped = px.clamp(32, 2000);
    LOUPE_SIZE.store(clamped, Ordering::Relaxed);
    if let Ok((key, _)) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(APP_KEY) {
        let _ = key.set_value(LOUPE_SIZE_VALUE, &(clamped as u32));
    }
}

/// Current (modifiers, virtual-key) bind for one hotkey action.
pub fn hotkey(action: HotkeyAction) -> (u32, u32) {
    match action {
        HotkeyAction::Full => {
            (HOTKEY_FULL_MODS.load(Ordering::Relaxed), HOTKEY_FULL_VK.load(Ordering::Relaxed))
        }
        HotkeyAction::Region => {
            (HOTKEY_REGION_MODS.load(Ordering::Relaxed), HOTKEY_REGION_VK.load(Ordering::Relaxed))
        }
        HotkeyAction::Record => {
            (HOTKEY_RECORD_MODS.load(Ordering::Relaxed), HOTKEY_RECORD_VK.load(Ordering::Relaxed))
        }
    }
}

/// Rebind one hotkey action: updates the atomics (read by keyhook::register_all())
/// and persists both values to the registry. Does NOT re-register the OS-level
/// hotkey itself: call keyhook::reload() after this so the pump thread picks it up.
pub fn set_hotkey(action: HotkeyAction, mods: u32, vk: u32) {
    let (mods_atomic, vk_atomic, mods_value, vk_value) = match action {
        HotkeyAction::Full => {
            (&HOTKEY_FULL_MODS, &HOTKEY_FULL_VK, HOTKEY_FULL_MODS_VALUE, HOTKEY_FULL_VK_VALUE)
        }
        HotkeyAction::Region => (
            &HOTKEY_REGION_MODS,
            &HOTKEY_REGION_VK,
            HOTKEY_REGION_MODS_VALUE,
            HOTKEY_REGION_VK_VALUE,
        ),
        HotkeyAction::Record => (
            &HOTKEY_RECORD_MODS,
            &HOTKEY_RECORD_VK,
            HOTKEY_RECORD_MODS_VALUE,
            HOTKEY_RECORD_VK_VALUE,
        ),
    };
    mods_atomic.store(mods, Ordering::Relaxed);
    vk_atomic.store(vk, Ordering::Relaxed);
    persist_u32(mods_value, mods);
    persist_u32(vk_value, vk);
}

/// Restore all three hotkeys to their compiled-in defaults (PrintScreen /
/// Ctrl+PrintScreen / Ctrl+Shift+PrintScreen) and persist. Caller still needs to
/// call keyhook::reload() to make the OS-level rebind take effect.
pub fn reset_hotkeys() {
    set_hotkey(HotkeyAction::Full, 0x0000, 0x2C);
    set_hotkey(HotkeyAction::Region, 0x0002, 0x2C);
    set_hotkey(HotkeyAction::Record, 0x0002 | 0x0004, 0x2C);
}

fn persist_u32(value: &str, v: u32) {
    if let Ok((key, _)) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(APP_KEY) {
        let _ = key.set_value(value, &v);
    }
}

fn persist_str(value: &str, s: &str) {
    if let Ok((key, _)) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(APP_KEY) {
        let _ = key.set_value(value, &s.to_string());
    }
}

/// Current persisted theme name ("Cyan" by default).
pub fn theme_name() -> String {
    THEME_NAME.read().unwrap().clone()
}

/// Current persisted theme source hex list (empty for the "Cyan" built-in).
pub fn theme_source() -> Vec<String> {
    THEME_SOURCE.read().unwrap().clone()
}

/// Persist a theme selection: name + the color list it was derived from.
/// Mirrored into the in-memory statics AND `HKCU\Software\TrontSnap` (ThemeName
/// REG_SZ + ThemeSource REG_SZ, comma-joined lowercase hex).
pub fn set_theme(name: &str, source: &[String]) {
    *THEME_NAME.write().unwrap() = name.to_string();
    *THEME_SOURCE.write().unwrap() = source.to_vec();
    persist_str(THEME_NAME_VALUE, name);
    persist_str(THEME_SOURCE_VALUE, &source.join(","));
}

pub fn has_run_before() -> bool {
    HAS_RUN.load(Ordering::Relaxed)
}

/// Mark that TrontSnap has run at least once (so future launches skip the first-run About).
pub fn set_has_run() {
    HAS_RUN.store(true, Ordering::Relaxed);
    persist(HAS_RUN_VALUE, true);
}

fn persist(value: &str, on: bool) {
    if let Ok((key, _)) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(APP_KEY) {
        let _ = key.set_value(value, &u32::from(on));
    }
}
