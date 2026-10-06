// TrontSnap — fast screenshot tool + history gallery.
//
// Modes (first arg):
//   (none) / "app"  -> persistent tray app: global hotkeys + gallery (window shown)
//   "--startup"     -> same, but start hidden in the tray (used by the autostart entry)
//   "region"        -> one-shot freeze-frame region picker, deliver, exit
//   "full"          -> one-shot fullscreen grab, deliver, exit
//   "ocr"           -> one-shot region picker, TEXT to clipboard (Windows OCR), exit
//   "record" [region] -> Linux only: toggle the running app's recorder (starts the app if
//                       it isn't running); "region" picks a rectangle first
//   "edit <path>"   -> annotation editor (arrow/box/redact/pixelate/text/crop); save overwrites
//
// Hotkeys owned by the app: PrintScreen = fullscreen, Ctrl+PrintScreen = region.
// Every capture goes to the clipboard (lossless) AND saves a PNG to Pictures\TrontSnap.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Platform split: the OS-integration modules exist twice — the original Win32
// implementation and a Linux twin under src/linux/ with the SAME module name and
// public API, selected here with #[path]. The 8k+ lines of UI code (app, gallery,
// editor, theme, settings UI) call `crate::clipboard::set_all(..)` etc. and never
// know which one they got. Keep the APIs in lockstep when touching either side.
mod app;
mod capture;
mod color;
mod editor;
mod gallery;
mod gifexport;
mod index;
mod kvstore;
mod settings;
mod shutter_wav;
mod theme;
mod thumbs;
mod toast;
mod watcher;
mod window_chrome;

#[cfg(windows)]
mod audio; // WASAPI loopback; the Linux recorder lets ffmpeg own audio instead

#[cfg(windows)]
mod autostart;
#[cfg(not(windows))]
#[path = "linux/autostart.rs"]
mod autostart;

#[cfg(windows)]
mod clipboard;
#[cfg(not(windows))]
#[path = "linux/clipboard.rs"]
mod clipboard;

// Linux-only multi-target clipboard owner, run as `trontsnap clip-serve ...`.
#[cfg(not(windows))]
#[path = "linux/clipserve.rs"]
mod clipserve;

// Linux-only: the desktop tray icon + menu (StatusNotifierItem + dbusmenu over D-Bus), also the
// recording indicator.
#[cfg(not(windows))]
#[path = "linux/sni.rs"]
mod sni;

// Linux-only: raising/focusing the main window on X11 after it is un-hidden from the tray.
#[cfg(not(windows))]
#[path = "linux/x11win.rs"]
mod x11win;

// Linux-only: `trontsnap record [region]`, what the GNOME record keybindings run.
#[cfg(not(windows))]
#[path = "linux/recordcli.rs"]
mod recordcli;

// Linux-only: one-shot desktop notices that really expire (GNOME ignores notify-send -t).
#[cfg(not(windows))]
#[path = "linux/notice.rs"]
mod notice;

#[cfg(windows)]
mod cursor;
#[cfg(not(windows))]
#[path = "linux/cursor.rs"]
mod cursor;

#[cfg(windows)]
mod keyhook;
#[cfg(not(windows))]
#[path = "linux/keyhook.rs"]
mod keyhook;

#[cfg(windows)]
mod ocr;
#[cfg(not(windows))]
#[path = "linux/ocr.rs"]
mod ocr;

#[cfg(windows)]
mod overlay;
#[cfg(not(windows))]
#[path = "linux/overlay.rs"]
mod overlay;

#[cfg(windows)]
mod printscreen;
#[cfg(not(windows))]
#[path = "linux/printscreen.rs"]
mod printscreen;

#[cfg(windows)]
mod recorder;
#[cfg(not(windows))]
#[path = "linux/recorder.rs"]
mod recorder;

// The module keeps its Windows name on both platforms so app.rs/main.rs call
// sites stay identical; the Linux file is an egui freeze-frame picker.
#[cfg(windows)]
mod region_win32;
#[cfg(not(windows))]
#[path = "linux/region.rs"]
mod region_win32;

#[cfg(windows)]
mod shellexec;
#[cfg(not(windows))]
#[path = "linux/shellexec.rs"]
mod shellexec;

#[cfg(windows)]
mod sound;
#[cfg(not(windows))]
#[path = "linux/sound.rs"]
mod sound;

#[cfg(windows)]
mod videothumb;
#[cfg(not(windows))]
#[path = "linux/videothumb.rs"]
mod videothumb;

#[cfg(test)]
mod visual;

fn main() {
    // `trontsnap --version`: first thing, before any setup (bug reports, packaging checks, scripts).
    if matches!(std::env::args().nth(1).as_deref(), Some("--version" | "-V")) {
        println!("trontsnap {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    // The clipboard owner daemon needs none of the setup below (settings, a
    // gsettings fork) and its startup time is on the paste-latency path.
    #[cfg(not(windows))]
    if std::env::args().nth(1).as_deref() == Some("clip-serve") {
        if let Err(e) = clipserve::run() {
            eprintln!("trontsnap clip-serve: {e:#}");
            std::process::exit(1);
        }
        return;
    }

    // The detached half of notice::show: sleeps, then closes one banner. No setup needed either.
    #[cfg(not(windows))]
    if std::env::args().nth(1).as_deref() == Some("notice-close") {
        notice::run_closer();
        return;
    }

    // The record keybinding: a plain toggle needs none of the setup below (it hands one
    // command to the running app, or starts the app), and its latency is the hotkey's.
    #[cfg(not(windows))]
    if std::env::args().nth(1).as_deref() == Some("record") {
        let rest: Vec<String> = std::env::args().skip(2).collect();
        if let Err(e) = recordcli::run(&rest) {
            eprintln!("trontsnap record: {e:#}");
            std::process::exit(1);
        }
        return;
    }

    // Load persisted settings before any capture path can read them (all modes capture).
    settings::load();

    // Free the PrintScreen key from Windows' own Snipping Tool binding (per-user,
    // no-admin registry flag) before any mode that installs the RegisterHotKey
    // hotkeys runs. Idempotent + best-effort: never blocks startup.
    // Unless the user opted out in Settings (PrintKeyOptOut): that choice used to be undone here on
    // every launch, including the one-shot modes.
    let mode = std::env::args().nth(1).unwrap_or_default();
    // Linux: only the app frees the key (it also installs the bindings). A one-shot (`full`, `region`)
    // forked `gsettings get` on every hotkey press for nothing, in the latency path.
    #[cfg(not(windows))]
    let frees_key = matches!(mode.as_str(), "" | "app" | "--startup" | "tray");
    #[cfg(windows)]
    let frees_key = true;
    if frees_key && !settings::print_key_opt_out() && printscreen::free_printscreen_key() {
        eprintln!("trontsnap: freed PrintScreen key from Windows' Snipping Tool binding");
    }

    let result = match mode.as_str() {
        "" | "app" => app::run(false),
        "--startup" | "tray" => app::run(true),
        "full" => capture::capture_full(),
        "region" => {
            region_win32::capture_region();
            Ok(())
        }
        "ocr" => {
            region_win32::capture_region_ocr();
            Ok(())
        }
        "edit" => match std::env::args().nth(2) {
            Some(p) => editor::run(std::path::PathBuf::from(p)),
            None => Ok(()),
        },
        "toast" => match std::env::args().nth(2) {
            Some(p) => {
                let clipboard_ok = !std::env::args().any(|a| a == "--clipboard-failed");
                toast::show(std::path::PathBuf::from(p), clipboard_ok)
            }
            None => Ok(()),
        },
        other => {
            #[cfg(windows)]
            const MODES: &str = "app / region / full / ocr / edit <png> / toast <png>";
            #[cfg(not(windows))]
            const MODES: &str = "app / region / full / ocr / record [region] / edit <png> / toast <png>";
            eprintln!("trontsnap: unknown mode '{other}' (use {MODES})");
            std::process::exit(2);
        }
    };
    if let Err(e) = result {
        eprintln!("trontsnap: {e:#}");
        // A hotkey-spawned one-shot has no terminal: stderr goes nowhere, so a failed grab was silent.
        #[cfg(not(windows))]
        if matches!(mode.as_str(), "full" | "region" | "ocr") {
            notice::show("Screenshot failed", &format!("{e:#}"), 10);
        }
        std::process::exit(1);
    }
}
