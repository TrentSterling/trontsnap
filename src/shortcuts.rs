//! Double-click launchers: `TrontSnap.lnk` and `TrontSnap Cam.lnk` on the Desktop
//! and in the Start menu, both pointing at the running exe (portable or installed).
//! One button in About / Settings makes them; making them again just refreshes the
//! target, so moving the exe and clicking again repairs stale shortcuts.

use std::path::PathBuf;

use windows::core::{Interface, HSTRING};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

/// (file name, arguments, description)
const LINKS: [(&str, &str, &str); 2] = [
    ("TrontSnap.lnk", "", "Screenshots, gallery and recording"),
    ("TrontSnap Cam.lnk", "cam", "Recorder frame: GIF, MP4 or WEBM of any area"),
];

fn dirs_for_links() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(d) = dirs::desktop_dir() {
        v.push(d);
    }
    if let Some(d) = dirs::data_dir() {
        v.push(d.join("Microsoft\\Windows\\Start Menu\\Programs"));
    }
    v
}

/// True when the Desktop already has both launchers.
pub fn present() -> bool {
    dirs::desktop_dir().is_some_and(|d| LINKS.iter().all(|(n, _, _)| d.join(n).is_file()))
}

/// Create (or refresh) every launcher. Returns how many were written.
pub fn create_all() -> anyhow::Result<usize> {
    let exe = std::env::current_exe()?;
    let mut n = 0;
    unsafe {
        // The UI thread is already an OLE STA (drag-out); a repeat init just fails.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        for dir in dirs_for_links() {
            if std::fs::create_dir_all(&dir).is_err() {
                continue;
            }
            for (name, args, desc) in LINKS {
                let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
                link.SetPath(&HSTRING::from(exe.as_os_str()))?;
                link.SetArguments(&HSTRING::from(args))?;
                link.SetDescription(&HSTRING::from(desc))?;
                link.SetIconLocation(&HSTRING::from(exe.as_os_str()), 0)?;
                if let Some(wd) = exe.parent() {
                    link.SetWorkingDirectory(&HSTRING::from(wd.as_os_str()))?;
                }
                let file: IPersistFile = link.cast()?;
                file.Save(&HSTRING::from(dir.join(name).as_os_str()), true)?;
                n += 1;
            }
        }
    }
    Ok(n)
}
