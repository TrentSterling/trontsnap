// Smart-capture window enumeration for the region picker.
//
// This used to also host an eframe/GL region picker (which repurposed the app's own
// window). That was replaced by a dedicated Win32/GDI overlay (see region_win32.rs);
// all that remains here is the window enumeration, which the GDI picker consumes.
//
// WINDOW ENUMERATION (smart capture): fully decoupled from the frozen frame's
// dimensions. `enumerate_windows_raw` does the Win32/DWM syscalls (no frame size
// needed) and is safe to run on a background thread concurrently with the screen grab.

use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW, IsIconic,
    IsWindowVisible, GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

/// Window classes that are never a "real" capturable app window, just desktop/
/// shell/overlay plumbing that happens to be a full-screen-sized, visible,
/// non-cloaked top-level window. "Progman" (the desktop) is the big one — without
/// this filter it silently becomes the smart-capture target for the whole desktop
/// background, which reads exactly like "the fullscreen was the rect". Matches
/// ShareX's own ignore list.
const IGNORED_CLASSES: &[&str] = &["Progman", "WorkerW", "Button", "CEF-OSC-WIDGET"];

/// A window's on-screen bounds (physical pixels). A bare RECT so enumeration has zero
/// dependency on the frozen frame size.
pub type RawRect = RECT;

/// Enumerate visible top-level windows and their *visual* bounds (physical pixels),
/// front-to-back, filtering out anything that isn't a real capturable app window.
/// Pure Win32 + DWM calls, safe to run on a background thread.
///
/// Uses `DWMWA_EXTENDED_FRAME_BOUNDS` in preference to `GetWindowRect`: modern
/// borderless-styled windows report a GetWindowRect that overshoots the real visual
/// edges (invisible resize-border/shadow margin) and can extend past the monitor
/// bounds. DWMWA_EXTENDED_FRAME_BOUNDS gives the accurate on-screen rect DWM
/// composites (the same preference ShareX makes). Falls back to GetWindowRect.
pub fn enumerate_windows_raw() -> Vec<RawRect> {
    let mut hwnds: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut hwnds as *mut _ as isize));
    }

    let mut out = Vec::new();
    for hwnd in hwnds {
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
                continue;
            }
            // Skip cloaked (ghost / suspended UWP / off-screen) windows.
            let mut cloaked: u32 = 0;
            let _ = DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED,
                (&mut cloaked as *mut u32).cast(),
                std::mem::size_of::<u32>() as u32,
            );
            if cloaked != 0 {
                continue;
            }
            // Skip desktop/shell/overlay plumbing by class name.
            let mut cbuf = [0u16; 256];
            let clen = GetClassNameW(hwnd, &mut cbuf).max(0) as usize;
            let class = String::from_utf16_lossy(&cbuf[..clen]);
            if IGNORED_CLASSES.iter().any(|c| class.eq_ignore_ascii_case(c)) {
                continue;
            }
            // Skip titleless windows: message-only/utility, and phantom full-screen
            // ApplicationFrameWindow hosts of real UWP content.
            if GetWindowTextLengthW(hwnd) <= 0 {
                continue;
            }
            // Skip non-activatable tool windows (overlays/utilities). Requires BOTH
            // flags (matches ShareX) so genuine floating palettes survive.
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            if ex & WS_EX_TOOLWINDOW.0 != 0 && ex & WS_EX_NOACTIVATE.0 != 0 {
                continue;
            }

            let mut r = RECT::default();
            let mut ext = RECT::default();
            let has_ext = DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                (&mut ext as *mut RECT).cast(),
                std::mem::size_of::<RECT>() as u32,
            )
            .is_ok();
            if has_ext {
                r = ext;
            } else if GetWindowRect(hwnd, &mut r).is_err() {
                continue;
            }

            out.push(r);
        }
    }
    out
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let list = &mut *(lparam.0 as *mut Vec<HWND>);
    list.push(hwnd);
    TRUE
}
