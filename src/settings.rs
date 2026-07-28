// Persisted user settings. Backed by the same HKCU\Software\TrontSnap registry key
// autostart already uses (winreg is a dependency), so there's no config file to manage.
// Values are mirrored into process-global atomics so the capture threads read them
// without touching the registry on the hot path.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{LazyLock, RwLock};

use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
use winreg::RegKey;

use crate::color::{self, Rgb};

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
const HOTKEY_OCR_MODS_VALUE: &str = "HotkeyOcrMods";
const HOTKEY_OCR_VK_VALUE: &str = "HotkeyOcrVk";
const THEME_NAME_VALUE: &str = "ThemeName";
const THEME_SOURCE_VALUE: &str = "ThemeSource";
const GRADIENT_VALUE: &str = "Gradient";
const GRADIENT_ANGLE_VALUE: &str = "GradientAngle";
const GRADIENT_INTENSITY_VALUE: &str = "GradientIntensity";
const GRADIENT_FROST_VALUE: &str = "GradientFrost";
const GRADIENT_PEGS_VALUE: &str = "GradientPegs";
const GRADIENT_HARMONY_VALUE: &str = "GradientHarmony";
const GRADIENT_PRESET_VALUE: &str = "GradientPreset";
const GRADIENT_CUSTOM_VALUE: &str = "GradientCustom";
const GRADIENT_PRESET_SYNC_VALUE: &str = "GradientPresetSync";

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

/// Which of the four global hotkeys a bind belongs to. Used both to key
/// settings::hotkey()/set_hotkey() and by keyhook::register_all() to look up
/// each RegisterHotKey id's current (modifiers, vk) pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    Full,
    Region,
    Record,
    /// Region OCR: drag a region, its TEXT lands on the clipboard.
    Ocr,
}

// Persisted (modifiers, virtual-key) pairs for the three rebindable hotkeys. The
// modifier values are the raw HOT_KEY_MODIFIERS bits (MOD_ALT=0x1, MOD_CONTROL=0x2,
// MOD_SHIFT=0x4, MOD_WIN=0x8) WITHOUT MOD_NOREPEAT: keyhook::register_all() ORs
// that in itself at RegisterHotKey time. VK_SNAPSHOT (0x2C) is PrintScreen for all
// three defaults, matching the original fixed bindings.
/// Fullscreen is bare PrintScreen. Always, on every build.
///
/// v0.14.0 briefly defaulted this to Alt+PrtSc on the portable build, reasoning
/// that a modifier-less bind is not delivered to a Medium process while an
/// elevated window has focus, so a modified one would at least work everywhere.
/// That traded the common case away for the rare one, and the result was a
/// screenshot tool where pressing PrintScreen did nothing at all. PrtSc is what
/// the key is for and what muscle memory expects.
///
/// The elevated-window case is solved by trontsnap-hotkeys.exe (see hotkeyd/),
/// which owns the registration from a uiAccess process, NOT by degrading the
/// binding everyone actually uses.
pub const DEFAULT_FULL_MODS: u32 = 0x0000;
pub const DEFAULT_REGION_MODS: u32 = 0x0002;
pub const DEFAULT_RECORD_MODS: u32 = 0x0002 | 0x0004;
/// Ctrl+Alt: the next open PrintScreen chord (Ctrl and Ctrl+Shift are taken).
pub const DEFAULT_OCR_MODS: u32 = 0x0002 | 0x0001;
/// VK_SNAPSHOT. All four defaults sit on PrintScreen.
pub const DEFAULT_HOTKEY_VK: u32 = 0x2C;

static HOTKEY_FULL_MODS: AtomicU32 = AtomicU32::new(DEFAULT_FULL_MODS);
static HOTKEY_FULL_VK: AtomicU32 = AtomicU32::new(DEFAULT_HOTKEY_VK);
static HOTKEY_REGION_MODS: AtomicU32 = AtomicU32::new(DEFAULT_REGION_MODS);
static HOTKEY_REGION_VK: AtomicU32 = AtomicU32::new(DEFAULT_HOTKEY_VK);
static HOTKEY_RECORD_MODS: AtomicU32 = AtomicU32::new(DEFAULT_RECORD_MODS);
static HOTKEY_RECORD_VK: AtomicU32 = AtomicU32::new(DEFAULT_HOTKEY_VK);
static HOTKEY_OCR_MODS: AtomicU32 = AtomicU32::new(DEFAULT_OCR_MODS);
static HOTKEY_OCR_VK: AtomicU32 = AtomicU32::new(DEFAULT_HOTKEY_VK);

// Persisted theme selection: a name ("Cyan" = the hardcoded default built-in,
// a premade palette name, "Custom" for a picked accent, or "Random <flavor>")
// plus the source hex list it was derived from (empty for "Cyan"). Mirrors
// theme::resolve()'s inputs exactly, so a restart reproduces the same theme.
static THEME_NAME: LazyLock<RwLock<String>> = LazyLock::new(|| RwLock::new("Cyan".to_string()));
static THEME_SOURCE: LazyLock<RwLock<Vec<String>>> = LazyLock::new(|| RwLock::new(Vec::new()));

// Discord-style background wash (theme::paint_gradient). Default ON; the
// checkbox in Settings > Appearance flips it and forces an immediate visuals
// rebuild (see app.rs) since it also governs panel-fill translucency.
static GRADIENT: AtomicBool = AtomicBool::new(true);

// Gradient v2 knobs (Discord parity): direction dial, color intensity, pegs
// (1..=4), harmony rule index, preset shelf index, custom peg colors, and
// whether picking a preset also re-themes the app. Intensity/frost are stored
// as integer percent (0-100) so the registry stays plain DWORDs; preset is
// stored as a signed decimal string since winreg's DWORD helpers are u32-only.
// FROST is DARK-ONLY here (TrontSnap has no light mode) — a single value,
// unlike SpaceView's frost_dark/frost_light pair — but kept as its own
// get/set pair (mirroring theme::frost()/set_frost()) so a future light mode
// is a trivial extension, not a rewrite.
static GRADIENT_ANGLE: AtomicU32 = AtomicU32::new(135);
static GRADIENT_INTENSITY_PCT: AtomicU32 = AtomicU32::new(45);
static GRADIENT_FROST_PCT: AtomicU32 = AtomicU32::new(85);
static GRADIENT_PEGS: AtomicU32 = AtomicU32::new(3);
static GRADIENT_HARMONY: AtomicU32 = AtomicU32::new(0);
static GRADIENT_PRESET: LazyLock<RwLock<i16>> = LazyLock::new(|| RwLock::new(-1));
static GRADIENT_PRESET_SYNC: AtomicBool = AtomicBool::new(true);
static GRADIENT_CUSTOM: LazyLock<RwLock<[Rgb; 4]>> = LazyLock::new(|| {
    RwLock::new([[86, 204, 255], [153, 14, 165], [253, 79, 80], [37, 223, 196]])
});

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
        if let Ok(v) = key.get_value::<u32, _>(HOTKEY_OCR_MODS_VALUE) {
            HOTKEY_OCR_MODS.store(v, Ordering::Relaxed);
        }
        if let Ok(v) = key.get_value::<u32, _>(HOTKEY_OCR_VK_VALUE) {
            HOTKEY_OCR_VK.store(v, Ordering::Relaxed);
        }
        if let Ok(v) = key.get_value::<String, _>(THEME_NAME_VALUE) {
            *THEME_NAME.write().unwrap() = v;
        }
        if let Ok(v) = key.get_value::<String, _>(THEME_SOURCE_VALUE) {
            let list: Vec<String> =
                if v.is_empty() { Vec::new() } else { v.split(',').map(|s| s.trim().to_string()).collect() };
            *THEME_SOURCE.write().unwrap() = list;
        }
        if let Ok(v) = key.get_value::<u32, _>(GRADIENT_VALUE) {
            GRADIENT.store(v != 0, Ordering::Relaxed);
        }
        if let Ok(v) = key.get_value::<u32, _>(GRADIENT_ANGLE_VALUE) {
            GRADIENT_ANGLE.store(v % 360, Ordering::Relaxed);
        }
        if let Ok(v) = key.get_value::<u32, _>(GRADIENT_INTENSITY_VALUE) {
            GRADIENT_INTENSITY_PCT.store(v.min(100), Ordering::Relaxed);
        }
        if let Ok(v) = key.get_value::<u32, _>(GRADIENT_FROST_VALUE) {
            GRADIENT_FROST_PCT.store(v.min(100), Ordering::Relaxed);
        }
        if let Ok(v) = key.get_value::<u32, _>(GRADIENT_PEGS_VALUE) {
            GRADIENT_PEGS.store(v.clamp(1, 4), Ordering::Relaxed);
        }
        if let Ok(v) = key.get_value::<u32, _>(GRADIENT_HARMONY_VALUE) {
            GRADIENT_HARMONY.store(v, Ordering::Relaxed);
        }
        if let Ok(v) = key.get_value::<String, _>(GRADIENT_PRESET_VALUE) {
            if let Ok(p) = v.trim().parse::<i16>() {
                *GRADIENT_PRESET.write().unwrap() = p;
            }
        }
        if let Ok(v) = key.get_value::<String, _>(GRADIENT_CUSTOM_VALUE) {
            let mut pegs = *GRADIENT_CUSTOM.read().unwrap();
            for (i, hex) in v.split(',').take(4).enumerate() {
                if let Some(rgb) = color::hex_to_rgb(hex.trim()) {
                    pegs[i] = rgb;
                }
            }
            *GRADIENT_CUSTOM.write().unwrap() = pegs;
        }
        if let Ok(v) = key.get_value::<u32, _>(GRADIENT_PRESET_SYNC_VALUE) {
            GRADIENT_PRESET_SYNC.store(v != 0, Ordering::Relaxed);
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
        HotkeyAction::Ocr => {
            (HOTKEY_OCR_MODS.load(Ordering::Relaxed), HOTKEY_OCR_VK.load(Ordering::Relaxed))
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
        HotkeyAction::Ocr => {
            (&HOTKEY_OCR_MODS, &HOTKEY_OCR_VK, HOTKEY_OCR_MODS_VALUE, HOTKEY_OCR_VK_VALUE)
        }
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

/// Restore all four hotkeys to their compiled-in defaults and persist. Caller
/// still needs to call keyhook::reload() to make the OS-level rebind take
/// effect. Fullscreen resets to Alt+PrtSc on the portable build and bare PrtSc
/// on the installed one (see DEFAULT_FULL_MODS).
pub fn reset_hotkeys() {
    set_hotkey(HotkeyAction::Full, DEFAULT_FULL_MODS, DEFAULT_HOTKEY_VK);
    set_hotkey(HotkeyAction::Region, DEFAULT_REGION_MODS, DEFAULT_HOTKEY_VK);
    set_hotkey(HotkeyAction::Record, DEFAULT_RECORD_MODS, DEFAULT_HOTKEY_VK);
    set_hotkey(HotkeyAction::Ocr, DEFAULT_OCR_MODS, DEFAULT_HOTKEY_VK);
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

/// Discord-style background wash toggle (default ON). Read by
/// `theme::build_visuals` (panel-fill translucency) and `app.rs`'s per-frame
/// `theme::paint_gradient` call.
pub fn gradient() -> bool {
    GRADIENT.load(Ordering::Relaxed)
}

pub fn set_gradient(on: bool) {
    GRADIENT.store(on, Ordering::Relaxed);
    persist(GRADIENT_VALUE, on);
}

/// Gradient v2 direction dial, in degrees (0..360).
pub fn gradient_angle() -> f32 {
    GRADIENT_ANGLE.load(Ordering::Relaxed) as f32
}
pub fn set_gradient_angle(deg: f32) {
    let v = deg.rem_euclid(360.0).round() as u32;
    GRADIENT_ANGLE.store(v, Ordering::Relaxed);
    persist_u32(GRADIENT_ANGLE_VALUE, v);
}

/// Gradient v2 color intensity (0..1); persisted as an integer percent.
pub fn gradient_intensity() -> f32 {
    GRADIENT_INTENSITY_PCT.load(Ordering::Relaxed) as f32 / 100.0
}
pub fn set_gradient_intensity(v: f32) {
    let pct = (v.clamp(0.0, 1.0) * 100.0).round() as u32;
    GRADIENT_INTENSITY_PCT.store(pct, Ordering::Relaxed);
    persist_u32(GRADIENT_INTENSITY_VALUE, pct);
}

/// Panel opacity over the wash (0..1), dark-only (see the FROST comment near
/// the statics above). Mirrors `theme::frost()`/`set_frost()`.
pub fn gradient_frost() -> f32 {
    GRADIENT_FROST_PCT.load(Ordering::Relaxed) as f32 / 100.0
}
pub fn set_gradient_frost(v: f32) {
    let pct = (v.clamp(0.0, 1.0) * 100.0).round() as u32;
    GRADIENT_FROST_PCT.store(pct, Ordering::Relaxed);
    persist_u32(GRADIENT_FROST_VALUE, pct);
}

/// Number of gradient pegs in play (1..=4; harmony/custom modes only).
pub fn gradient_pegs_count() -> u8 {
    GRADIENT_PEGS.load(Ordering::Relaxed).clamp(1, 4) as u8
}
pub fn set_gradient_pegs_count(n: u8) {
    let v = n.clamp(1, 4) as u32;
    GRADIENT_PEGS.store(v, Ordering::Relaxed);
    persist_u32(GRADIENT_PEGS_VALUE, v);
}

/// Index into `color::HARMONY_RULES` used in harmony mode.
pub fn gradient_harmony() -> u8 {
    GRADIENT_HARMONY.load(Ordering::Relaxed) as u8
}
pub fn set_gradient_harmony(v: u8) {
    GRADIENT_HARMONY.store(v as u32, Ordering::Relaxed);
    persist_u32(GRADIENT_HARMONY_VALUE, v as u32);
}

/// Gradient source mode: >= 0 is a `theme::GRADIENT_PRESETS` index, -1 is
/// harmony mode, -2 is custom mode.
pub fn gradient_preset() -> i16 {
    *GRADIENT_PRESET.read().unwrap()
}
pub fn set_gradient_preset(v: i16) {
    *GRADIENT_PRESET.write().unwrap() = v;
    persist_str(GRADIENT_PRESET_VALUE, &v.to_string());
}

/// Whether picking a preset also rethemes the app (accent = the preset's most
/// saturated stop). Default ON: "theme IS the gradient".
pub fn gradient_preset_sync() -> bool {
    GRADIENT_PRESET_SYNC.load(Ordering::Relaxed)
}
pub fn set_gradient_preset_sync(on: bool) {
    GRADIENT_PRESET_SYNC.store(on, Ordering::Relaxed);
    persist(GRADIENT_PRESET_SYNC_VALUE, on);
}

/// Manual pegs for custom mode (slot 0 is overridden by the live accent at
/// read time in `theme::gradient_pegs`; slots 1..4 are these values verbatim).
pub fn gradient_custom() -> [Rgb; 4] {
    *GRADIENT_CUSTOM.read().unwrap()
}
pub fn set_gradient_custom(pegs: [Rgb; 4]) {
    *GRADIENT_CUSTOM.write().unwrap() = pegs;
    let joined = pegs.iter().map(|&c| color::rgb_to_hex(c)).collect::<Vec<_>>().join(",");
    persist_str(GRADIENT_CUSTOM_VALUE, &joined);
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
