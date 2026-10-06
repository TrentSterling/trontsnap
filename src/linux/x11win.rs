// X11 side of showing the main window, Linux twin of app.rs's Win32 restore_and_foreground.
//
// winit maps a hidden window (set_visible(true) = XMapWindow + raise) but cannot FOCUS it right
// after: its focus_window() is a no-op until the first VisibilityNotify, and Mutter's focus-stealing
// prevention otherwise leaves a freshly mapped window stacked below the focused one (a tray click
// then "does nothing" visibly). So we ask the window manager ourselves, the way a pager does:
// _NET_ACTIVE_WINDOW with source 2, which EWMH window managers honour without the stealing rules.

use std::time::Duration;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ClientMessageEvent, ConnectionExt as _, EventMask, MapState};
use x11rb::rust_connection::RustConnection;

/// Can we reach an X server at all (XWayland on a Wayland session)? Probed before forcing the main
/// window onto X11: a Wayland session without XWayland must keep its native window.
pub fn reachable() -> bool {
    RustConnection::connect(None).is_ok()
}

/// Raise + focus `win` once it is actually mapped. Off-thread: the map is the caller's winit command
/// still in flight, so this polls (<= 1 s) instead of racing it.
pub fn activate_when_mapped(win: u32) {
    std::thread::spawn(move || {
        let Ok((conn, screen)) = RustConnection::connect(None) else { return };
        let root = conn.setup().roots[screen].root;
        for _ in 0..20 {
            let mapped = conn
                .get_window_attributes(win)
                .ok()
                .and_then(|c| c.reply().ok())
                .map_or(false, |a| a.map_state == MapState::VIEWABLE);
            if mapped {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let Some(atom) = conn
            .intern_atom(false, b"_NET_ACTIVE_WINDOW")
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.atom)
        else {
            return;
        };
        let ev = ClientMessageEvent::new(32, win, atom, [2u32, 0, 0, 0, 0]);
        let _ = conn.send_event(
            false,
            root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            ev,
        );
        let _ = conn.flush();
    });
}
