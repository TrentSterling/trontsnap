// Recording indicator in the desktop tray, Linux side: a StatusNotifierItem (the protocol
// behind Ubuntu's AppIndicator extension, KDE and most trays) that exists exactly while a
// recording does. There is no tray library: it is one D-Bus object (zbus, already a dependency)
// plus a RegisterStatusNotifierItem call on the shell's watcher.
//
// Why it exists: the kms recorder starts ~3 s after the key and a banner would be filmed (GNOME
// pins banners inside the picture), so the title-bar pill only helps while the TrontSnap window
// is open. A tray dot is the one persistent cue that works with the window hidden. It IS in the
// top bar of whatever the screen shows, so it appears in a full-screen recording (no
// capture-exclusion exists on Linux); `hidetopbar`-style extensions keep it out of sight.
//
// Create/destroy per recording instead of flipping `Status`: dropping the connection releases
// the bus name and the watcher removes the item by itself, so there is no signal plumbing and
// nothing to leak. Everything is best-effort and off-thread: no watcher (not GNOME + the
// extension, no tray at all) just means no icon.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

use zbus::blocking::Connection;
use zbus::interface;

const PATH: &str = "/StatusNotifierItem";

static TRAY: Mutex<Option<Connection>> = Mutex::new(None);
/// Recordings currently entitled to the icon (a quick restart overlaps two sessions).
static WANT: AtomicUsize = AtomicUsize::new(0);

fn lk<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

type Pixmap = (i32, i32, Vec<u8>);

/// A red dot, ARGB32 in network byte order (alpha first), anti-aliased edge.
fn dot(size: i32) -> Pixmap {
    let c = size as f32 / 2.0;
    let r = c - 1.5;
    let mut px = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let d = ((x as f32 + 0.5 - c).powi(2) + (y as f32 + 0.5 - c).powi(2)).sqrt();
            let a = (r + 0.5 - d).clamp(0.0, 1.0);
            px.extend_from_slice(&[(a * 255.0) as u8, 236, 72, 72]);
        }
    }
    (size, size, px)
}

struct Item;

#[interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    #[zbus(property)]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }
    #[zbus(property)]
    fn id(&self) -> &str {
        "trontsnap-recording"
    }
    #[zbus(property)]
    fn title(&self) -> &str {
        "TrontSnap is recording"
    }
    #[zbus(property)]
    fn status(&self) -> &str {
        "Active"
    }
    #[zbus(property)]
    fn window_id(&self) -> u32 {
        0
    }
    #[zbus(property)]
    fn icon_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn icon_pixmap(&self) -> Vec<Pixmap> {
        vec![dot(16), dot(22), dot(32)]
    }
    #[zbus(property)]
    fn overlay_icon_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn attention_icon_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn tool_tip(&self) -> (String, Vec<Pixmap>, String, String) {
        (String::new(), Vec::new(), "TrontSnap is recording".into(), "Click to stop".into())
    }
    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn menu(&self) -> zbus::zvariant::OwnedObjectPath {
        // The spec's "no menu" path.
        zbus::zvariant::OwnedObjectPath::try_from("/NO_DBUSMENU").expect("static path")
    }

    /// Left click: stop the recording.
    fn activate(&self, _x: i32, _y: i32) {
        std::thread::spawn(crate::recorder::toggle);
    }
    fn secondary_activate(&self, _x: i32, _y: i32) {
        std::thread::spawn(crate::recorder::toggle);
    }
    fn context_menu(&self, _x: i32, _y: i32) {}
    fn scroll(&self, _delta: i32, _orientation: &str) {}
}

fn build() -> zbus::Result<Connection> {
    let name = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
    let conn = zbus::blocking::connection::Builder::session()?
        .name(name.as_str())?
        .serve_at(PATH, Item)?
        .build()?;
    let watcher = zbus::blocking::Proxy::new(
        &conn,
        "org.kde.StatusNotifierWatcher",
        "/StatusNotifierWatcher",
        "org.kde.StatusNotifierWatcher",
    )?;
    watcher.call::<_, _, ()>("RegisterStatusNotifierItem", &(name.as_str(),))?;
    Ok(conn)
}

/// A recording started: put the dot in the tray (returns at once).
pub fn show() {
    if WANT.fetch_add(1, Ordering::SeqCst) > 0 {
        return;
    }
    std::thread::spawn(|| {
        let made = build();
        let mut slot = lk(&TRAY);
        match made {
            // `hide()` may have run while the bus calls were in flight.
            Ok(conn) if WANT.load(Ordering::SeqCst) > 0 => *slot = Some(conn),
            Ok(_) => {}
            Err(e) => eprintln!("trontsnap: no tray indicator ({e})"),
        }
    });
}

/// A recording ended (delivered or failed). The dot goes with the last one.
pub fn hide() {
    if WANT.fetch_sub(1, Ordering::SeqCst) == 1 {
        *lk(&TRAY) = None;
    }
}

/// Pairs show()/hide() around a session's whole life, early returns included.
pub struct TrayGuard;

impl TrayGuard {
    pub fn new() -> Self {
        show();
        TrayGuard
    }
}

impl Drop for TrayGuard {
    fn drop(&mut self) {
        hide();
    }
}
