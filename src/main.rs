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

// Linux-only: `trontsnap record [region]`, what the GNOME record keybindings run.
#[cfg(not(windows))]
#[path = "linux/recordcli.rs"]
mod recordcli;

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
    if printscreen::free_printscreen_key() {
        eprintln!("trontsnap: freed PrintScreen key from Windows' Snipping Tool binding");
    }

    let mode = std::env::args().nth(1).unwrap_or_default();
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
            eprintln!(
                "trontsnap: unknown mode '{other}' (use app / region / full / ocr / record / edit <png> / toast <png>)"
            );
            std::process::exit(2);
        }
    };
    if let Err(e) = result {
        eprintln!("trontsnap: {e:#}");
        std::process::exit(1);
    }
}
