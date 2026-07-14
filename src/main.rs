// TrontSnap — fast screenshot tool + history gallery.
//
// Modes (first arg):
//   (none) / "app"  -> persistent tray app: global hotkeys + gallery (window shown)
//   "--startup"     -> same, but start hidden in the tray (used by the autostart entry)
//   "region"        -> one-shot freeze-frame region picker, deliver, exit
//   "full"          -> one-shot fullscreen grab, deliver, exit
//
// Hotkeys owned by the app: PrintScreen = fullscreen, Ctrl+PrintScreen = region.
// Every capture goes to the clipboard (lossless) AND saves a PNG to Pictures\TrontSnap.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod autostart;
mod capture;
mod clipboard;
mod gallery;
mod index;
mod keyhook;
mod overlay;
mod region_win32;
mod sound;
mod theme;
mod thumbs;
mod toast;
mod watcher;

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let result = match mode.as_str() {
        "" | "app" => app::run(false),
        "--startup" | "tray" => app::run(true),
        "full" => capture::capture_full(),
        "region" => {
            region_win32::capture_region();
            Ok(())
        }
        "toast" => match std::env::args().nth(2) {
            Some(p) => toast::show(std::path::PathBuf::from(p)),
            None => Ok(()),
        },
        other => {
            eprintln!("trontsnap: unknown mode '{other}' (use app / region / full)");
            std::process::exit(2);
        }
    };
    if let Err(e) = result {
        eprintln!("trontsnap: {e:#}");
        std::process::exit(1);
    }
}
