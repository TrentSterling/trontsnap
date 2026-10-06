// Tiny persisted key-value store behind the settings/autostart/printscreen state.
//
// Windows: HKCU\Software\TrontSnap, exactly the values v0.24-and-earlier wrote,
// so existing installs keep their settings across this refactor.
// Linux: ~/.config/trontsnap/settings.conf, one `Key=value` per line, loaded once
// into a RwLock'd map and written through on every set. The file is tiny and sets
// are rare (user toggles), so rewrite-whole-file is the simple correct choice.

#[cfg(windows)]
mod imp {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    use winreg::RegKey;

    const APP_KEY: &str = r"Software\TrontSnap";

    pub fn get_u32(name: &str) -> Option<u32> {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(APP_KEY, KEY_READ)
            .and_then(|k| k.get_value::<u32, _>(name))
            .ok()
    }

    pub fn get_string(name: &str) -> Option<String> {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(APP_KEY, KEY_READ)
            .and_then(|k| k.get_value::<String, _>(name))
            .ok()
    }

    pub fn set_u32(name: &str, v: u32) {
        if let Ok((key, _)) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(APP_KEY) {
            let _ = key.set_value(name, &v);
        }
    }

    pub fn set_string(name: &str, v: &str) {
        if let Ok((key, _)) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(APP_KEY) {
            let _ = key.set_value(name, &v.to_string());
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::{LazyLock, RwLock};

    fn conf_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("trontsnap").join("settings.conf"))
    }

    static MAP: LazyLock<RwLock<BTreeMap<String, String>>> = LazyLock::new(|| {
        let mut m = BTreeMap::new();
        if let Some(p) = conf_path() {
            if let Ok(text) = std::fs::read_to_string(p) {
                for line in text.lines() {
                    if let Some((k, v)) = line.split_once('=') {
                        m.insert(k.trim().to_string(), v.trim().to_string());
                    }
                }
            }
        }
        RwLock::new(m)
    });

    fn flush(m: &BTreeMap<String, String>) {
        let Some(p) = conf_path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut out = String::new();
        for (k, v) in m {
            out.push_str(k);
            out.push('=');
            out.push_str(v);
            out.push('\n');
        }
        // Write-then-rename so a crash mid-flush can never truncate the config
        // to nothing (rename is atomic on the same filesystem).
        let tmp = p.with_extension("conf.tmp");
        if std::fs::write(&tmp, out).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }

    pub fn get_u32(name: &str) -> Option<u32> {
        MAP.read().ok()?.get(name)?.parse().ok()
    }

    pub fn get_string(name: &str) -> Option<String> {
        MAP.read().ok()?.get(name).cloned()
    }

    pub fn set_u32(name: &str, v: u32) {
        set_string(name, &v.to_string());
    }

    pub fn set_string(name: &str, v: &str) {
        if let Ok(mut m) = MAP.write() {
            m.insert(name.to_string(), v.to_string());
            flush(&m);
        }
    }
}

pub use imp::{get_string, get_u32, set_string, set_u32};
