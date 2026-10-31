//! Everything TrontSnap writes outside its own exe, in one place: the About tab
//! lists it, and "Remove TrontSnap's data" undoes all of it except your captures.
//! Keep this list in sync with README "Where your stuff lives" and the site.

use std::path::PathBuf;

use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

pub struct Place {
    pub what: &'static str,
    pub where_: String,
    /// A folder Explorer can open; None for registry values.
    pub folder: Option<PathBuf>,
}

pub fn places() -> Vec<Place> {
    let local = dirs::data_local_dir().map(|d| d.join("TrontSnap"));
    let temp = std::env::temp_dir().join("TrontSnap");
    let pics = crate::capture::trontsnap_dir().ok();
    let show = |p: &Option<PathBuf>| p.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
    vec![
        Place {
            what: "Your captures (kept)",
            where_: show(&pics),
            folder: pics.clone(),
        },
        Place {
            what: "Thumbnail cache + error log",
            where_: show(&local),
            folder: local.clone(),
        },
        Place {
            what: "Instant replay buffer",
            where_: temp.join("replay").display().to_string(),
            folder: Some(temp),
        },
        Place {
            what: "Settings",
            where_: r"Registry: HKCU\Software\TrontSnap".into(),
            folder: None,
        },
        Place {
            what: "Start at login (if on)",
            where_: r"Registry: HKCU\...\CurrentVersion\Run\TrontSnap".into(),
            folder: None,
        },
        Place {
            what: "Shortcuts (if made)",
            where_: "Desktop + Start menu: TrontSnap, TrontSnap Cam".into(),
            folder: None,
        },
        Place {
            what: "PrtSc handed to TrontSnap",
            where_: r"Registry: HKCU\Control Panel\Keyboard\PrintScreenKeyForSnippingEnabled = 0".into(),
            folder: None,
        },
    ]
}

/// Remove everything above except the captures, then the caller exits.
/// Best-effort per item; returns the items that could not be removed.
pub fn remove_all() -> Vec<String> {
    let mut failed = Vec::new();
    crate::replay::stop();
    crate::cam::close_running();
    if let Err(e) = crate::autostart::disable() {
        failed.push(format!("login entry: {e}"));
    }
    crate::shortcuts::remove_all();
    crate::printscreen::set_free(false);
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    match hkcu.delete_subkey_all(r"Software\TrontSnap") {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => failed.push(format!("settings: {e}")),
    }
    for p in places().into_iter().skip(1).filter_map(|p| p.folder) {
        if p.exists() {
            if let Err(e) = std::fs::remove_dir_all(&p) {
                failed.push(format!("{}: {e}", p.display()));
            }
        }
    }
    failed
}
