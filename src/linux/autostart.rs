// Run-at-login via XDG autostart (~/.config/autostart/trontsnap.desktop) — the
// Linux twin of the HKCU Run key. The entry launches us hidden (`--startup`) so
// login is quiet.

use std::path::PathBuf;

fn desktop_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("autostart").join("trontsnap.desktop"))
}

fn desktop_entry() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(format!(
        "[Desktop Entry]\nType=Application\nName=TrontSnap\nExec=\"{}\" --startup\nX-GNOME-Autostart-enabled=true\nComment=Screenshot tool (starts hidden)\n",
        exe.display()
    ))
}

pub fn is_enabled() -> bool {
    let Some(p) = desktop_path() else { return false };
    let Ok(stored) = std::fs::read_to_string(p) else { return false };
    // Only "enabled" if the entry points at THIS exe — a stale file from an old
    // location reads as disabled so the toggle rewrites it (same rule as Windows).
    match std::env::current_exe() {
        Ok(exe) => stored.contains(&exe.display().to_string()),
        Err(_) => !stored.is_empty(),
    }
}

pub fn enable() -> anyhow::Result<()> {
    let p = desktop_path().ok_or_else(|| anyhow::anyhow!("no config dir"))?;
    let entry = desktop_entry().ok_or_else(|| anyhow::anyhow!("no exe path"))?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(p, entry)?;
    Ok(())
}

pub fn disable() -> anyhow::Result<()> {
    if let Some(p) = desktop_path() {
        let _ = std::fs::remove_file(p);
    }
    Ok(())
}

/// Turn autostart on exactly once — the first time TrontSnap ever runs. After
/// that we never touch it automatically, so a user's "off" sticks across restarts.
pub fn ensure_first_run() {
    if crate::kvstore::get_u32("AutostartInit").is_some() {
        return;
    }
    let _ = enable();
    crate::kvstore::set_u32("AutostartInit", 1);
}

pub fn set(enabled: bool) {
    let _ = if enabled { enable() } else { disable() };
}
