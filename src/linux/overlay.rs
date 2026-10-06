// Smart-capture window enumeration, Linux side. xcap's Window::all() does the
// real work on X11; on Wayland compositors a client cannot enumerate other
// windows at all, so this returns an empty list and the region picker simply
// has no smart-click targets (drag still works, which is the primary gesture).

/// A window's on-screen bounds in physical pixels. Field names match Win32's
/// RECT so the picker code is identical on both platforms.
#[derive(Clone, Copy, Debug, Default)]
pub struct RawRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// Enumerate visible top-level windows front-to-back. Best-effort: any error
/// (headless, Wayland, no compositor cooperation) is an empty list, never a panic.
pub fn enumerate_windows_raw() -> Vec<RawRect> {
    let Ok(windows) = xcap::Window::all() else { return Vec::new() };
    windows
        .iter()
        .filter(|w| !w.is_minimized())
        .map(|w| RawRect {
            left: w.x(),
            top: w.y(),
            right: w.x() + w.width() as i32,
            bottom: w.y() + w.height() as i32,
        })
        .filter(|r| r.right > r.left && r.bottom > r.top)
        .collect()
}
