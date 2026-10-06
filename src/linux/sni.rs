// The desktop tray icon, Linux side: a StatusNotifierItem (the protocol behind Ubuntu's AppIndicator
// extension, KDE and most trays) plus its com.canonical.dbusmenu menu. There is no tray library
// (tray-icon wants gtk + libappindicator, which would drag C build deps into the AppImage): it is
// two D-Bus objects on zbus, already a dependency, and one RegisterStatusNotifierItem call.
//
// What it is for: Windows closes to the tray; Linux had nothing to close to, so Close quit the app.
// With a registered item the window can hide instead (app.rs decides, from `registered()`), the
// menu mirrors the Windows tray menu, and the SAME icon doubles as the recording cue: a red-badged
// variant from the moment frames flow to the stop press. The old design was a second, bare item that
// existed only while recording; one persistent item replaces it (TrayGuard/show/hide keep their API).
//
// Lifecycle: `start()` runs once, BEFORE the window exists, because whether a tray is there decides
// which windowing backend the main window gets (see app.rs: only X11 can hide and re-map). A watcher
// thread owns registration: it registers whenever org.kde.StatusNotifierWatcher has an owner, and
// again when that owner changes (shell restart, extension toggled), so `registered()` is live truth,
// not a startup snapshot. No watcher = no tray = the app behaves exactly as it did before.
//
// Menu actions leave over a channel (`actions()`), not callbacks: the UI context does not exist yet
// when `start()` runs, and clicks that arrive early simply queue.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use zbus::blocking::Connection;
use zbus::{interface, SignalContext};
use zbus::zvariant::{OwnedValue, StructureBuilder, Value};

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
const ITEM_IFACE: &str = "org.kde.StatusNotifierItem";
const MENU_IFACE: &str = "com.canonical.dbusmenu";

/// What a tray click asks the app to do. Check items (cursor / audio / login) are applied here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Show,
    Full,
    Region,
    Record,
    RecordRegion,
    Ocr,
    Quit,
}



static CONN: OnceLock<Connection> = OnceLock::new();
static ACTIONS: OnceLock<(Sender<Action>, Receiver<Action>)> = OnceLock::new();
/// The watcher has our item AND a host is there to draw it. Flips with the watcher's life.
static REGISTERED: AtomicBool = AtomicBool::new(false);
/// The watcher thread has made its first look at the bus (owner or not).
static LOOKED: AtomicBool = AtomicBool::new(false);
/// Recordings currently entitled to the red badge (a quick restart overlaps two sessions).
static WANT: AtomicUsize = AtomicUsize::new(0);
/// dbusmenu layout revision.
static REV: AtomicU32 = AtomicU32::new(1);

fn actions_chan() -> &'static (Sender<Action>, Receiver<Action>) {
    ACTIONS.get_or_init(crossbeam_channel::unbounded)
}

/// The receiving end of every menu click / activation. Cloneable; the app drains it on a thread.
pub fn actions() -> Receiver<Action> {
    actions_chan().1.clone()
}

fn send(a: Action) {
    let _ = actions_chan().0.send(a);
}

/// Is the item registered with a live watcher + host right now? Close-to-tray rides on this.
pub fn registered() -> bool {
    REGISTERED.load(Ordering::SeqCst)
}

/// A recording is live (red badge up): the one state where a click means "stop".
fn recording() -> bool {
    WANT.load(Ordering::SeqCst) > 0
}

// === Icon ===

type Pixmap = (i32, i32, Vec<u8>);

const SIZES: [u32; 6] = [16, 22, 24, 32, 48, 64];

/// The rainbow app icon (assets/trontsnap.png) as SNI pixmaps: ARGB32, network byte order (alpha
/// first). `badge` adds the recording dot. Built once; the host picks the size nearest its bar.
fn pixmaps(badge: bool) -> &'static Vec<Pixmap> {
    static IDLE: OnceLock<Vec<Pixmap>> = OnceLock::new();
    static REC: OnceLock<Vec<Pixmap>> = OnceLock::new();
    let cell = if badge { &REC } else { &IDLE };
    cell.get_or_init(|| {
        SIZES
            .iter()
            .map(|&s| {
                let mut rgba = crate::app::face_rgba(s).unwrap_or_else(|| vec![255; (s * s * 4) as usize]);
                if badge {
                    paint_badge(&mut rgba, s);
                }
                let mut argb = Vec::with_capacity(rgba.len());
                for p in rgba.chunks_exact(4) {
                    argb.extend_from_slice(&[p[3], p[0], p[1], p[2]]);
                }
                (s as i32, s as i32, argb)
            })
            .collect()
    })
}

/// A red dot with a dark rim in the bottom-right corner, anti-aliased. The rim keeps it readable on
/// the icon's bright rainbow border.
fn paint_badge(rgba: &mut [u8], size: u32) {
    const RING: [f32; 3] = [24.0, 16.0, 24.0];
    const RED: [f32; 3] = [236.0, 72.0, 72.0];
    let s = size as f32;
    let (cx, cy) = (s * 0.70, s * 0.70);
    let r = s * 0.30;
    let rim = (s * 0.08).max(1.0);
    for y in 0..size {
        for x in 0..size {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            let disc = (r + 0.5 - d).clamp(0.0, 1.0);
            if disc <= 0.0 {
                continue;
            }
            let fill = (r - rim + 0.5 - d).clamp(0.0, 1.0);
            let i = ((y * size + x) * 4) as usize;
            for c in 0..3 {
                let under = rgba[i + c] as f32;
                let ringed = under + (RING[c] - under) * disc;
                rgba[i + c] = (ringed + (RED[c] - ringed) * fill) as u8;
            }
            rgba[i + 3] = rgba[i + 3].max((disc * 255.0) as u8);
        }
    }
}

// === StatusNotifierItem ===

struct Item;

#[interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    #[zbus(property)]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }
    #[zbus(property)]
    fn id(&self) -> &str {
        "trontsnap"
    }
    #[zbus(property)]
    fn title(&self) -> &str {
        "TrontSnap"
    }
    #[zbus(property)]
    fn status(&self) -> &str {
        "Active"
    }
    // HARD-WON: the spec's WindowId is int32 ("i"). The old recording dot shipped it as "u" and the
    // real AppIndicator extension logged "property WindowId with type u does not match expected type i".
    #[zbus(property)]
    fn window_id(&self) -> i32 {
        0
    }
    #[zbus(property)]
    fn icon_name(&self) -> &str {
        ""
    }
    #[zbus(property)]
    fn icon_pixmap(&self) -> Vec<Pixmap> {
        pixmaps(recording()).clone()
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
        if recording() {
            (String::new(), Vec::new(), "TrontSnap is recording".into(), "Click to stop".into())
        } else {
            (
                String::new(),
                Vec::new(),
                "TrontSnap".into(),
                "Double-click: open - Middle click: capture a region - Right click: menu".into(),
            )
        }
    }
    /// False on purpose: a host then sends a left click as Activate instead of opening the menu.
    /// HARD-WON: GNOME's AppIndicator extension (checked live on a private shell) still opens the
    /// menu on a single left click when one is exported, and sends Activate only on a DOUBLE click;
    /// KDE-style hosts send Activate on a single click.
    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn menu(&self) -> zbus::zvariant::OwnedObjectPath {
        zbus::zvariant::OwnedObjectPath::try_from(MENU_PATH).expect("static path")
    }

    /// Activate (a single click on KDE-style hosts, a double click on GNOME's extension): open the
    /// window; while a recording is live it stops it (the red badge is the cue).
    fn activate(&self, _x: i32, _y: i32) {
        if recording() {
            std::thread::spawn(crate::recorder::toggle);
        } else {
            send(Action::Show);
        }
    }
    /// Middle click: the same instant region capture the Windows tray's left click does.
    fn secondary_activate(&self, _x: i32, _y: i32) {
        send(Action::Region);
    }
    fn context_menu(&self, _x: i32, _y: i32) {}
    fn scroll(&self, _delta: i32, _orientation: &str) {}

    // Declared so introspection lists them; emitted by name from notifier() (one thread, no
    // handler ever blocks on the bus).
    #[zbus(signal)]
    async fn new_icon(ctxt: &SignalContext<'_>) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn new_tool_tip(ctxt: &SignalContext<'_>) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn new_status(ctxt: &SignalContext<'_>, status: &str) -> zbus::Result<()>;
}

// === dbusmenu ===

const ID_SHOW: i32 = 1;
const ID_FULL: i32 = 2;
const ID_REGION: i32 = 3;
const ID_RECORD: i32 = 4;
const ID_RECORD_REGION: i32 = 5;
const ID_OCR: i32 = 6;
const ID_CURSOR: i32 = 7;
const ID_AUDIO: i32 = 8;
const ID_LOGIN: i32 = 9;
const ID_QUIT: i32 = 10;

enum Kind {
    Item,
    Sep,
    Check(bool),
}

struct Entry {
    id: i32,
    label: &'static str,
    kind: Kind,
    enabled: bool,
}

/// The menu as it should read RIGHT NOW. Rebuilt on every GetLayout, so a setting flipped in the
/// window (or a recording started by hotkey) is correct the next time the menu opens: AboutToShow
/// answers "yes, refetch" for exactly that.
fn entries() -> Vec<Entry> {
    use crate::recorder::Status;
    let busy = matches!(crate::recorder::status(), Status::Starting | Status::Recording(_));
    let item = |id, label, enabled| Entry { id, label, kind: Kind::Item, enabled };
    let sep = |id| Entry { id, label: "", kind: Kind::Sep, enabled: true };
    let check = |id, label, on| Entry { id, label, kind: Kind::Check(on), enabled: true };
    vec![
        item(ID_SHOW, "Show TrontSnap", true),
        sep(100),
        item(ID_FULL, "Capture fullscreen", true),
        item(ID_REGION, "Capture region", true),
        item(ID_RECORD, if busy { "Stop recording" } else { "Record screen" }, true),
        // A region press during a recording is a plain stop (recordcli), so it is only offered idle.
        item(ID_RECORD_REGION, "Record region", !busy),
        // Always enabled: without tesseract the click says how to install it (a greyed tray item
        // can't, there are no tooltips).
        item(ID_OCR, "Copy text (OCR)", true),
        sep(101),
        check(ID_CURSOR, "Capture cursor", crate::settings::capture_cursor()),
        check(ID_AUDIO, "Record audio", crate::settings::record_audio()),
        check(ID_LOGIN, "Start at login", crate::autostart::is_enabled()),
        sep(102),
        item(ID_QUIT, "Quit TrontSnap", true),
    ]
}

type Props = HashMap<String, Value<'static>>;

fn props(e: &Entry) -> Props {
    let mut p = Props::new();
    p.insert("visible".into(), Value::from(true));
    match e.kind {
        Kind::Sep => {
            p.insert("type".into(), Value::from("separator".to_string()));
        }
        Kind::Item => {
            p.insert("label".into(), Value::from(e.label.to_string()));
            p.insert("enabled".into(), Value::from(e.enabled));
        }
        Kind::Check(on) => {
            p.insert("label".into(), Value::from(e.label.to_string()));
            p.insert("enabled".into(), Value::from(e.enabled));
            p.insert("toggle-type".into(), Value::from("checkmark".to_string()));
            p.insert("toggle-state".into(), Value::from(on as i32));
        }
    }
    p
}

/// A leaf node as it nests inside its parent's `av`: a `(ia{sv}av)` struct in a variant.
fn leaf(e: &Entry) -> Value<'static> {
    Value::Structure(
        StructureBuilder::new()
            .add_field(e.id)
            .add_field(props(e))
            .add_field(Vec::<Value<'static>>::new())
            .build(),
    )
}

fn root_props() -> Props {
    let mut p = Props::new();
    p.insert("children-display".into(), Value::from("submenu".to_string()));
    p
}

fn owned(v: &Value<'_>) -> OwnedValue {
    v.try_to_owned().expect("menu values carry no fds")
}

struct Menu;

#[interface(name = "com.canonical.dbusmenu")]
impl Menu {
    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }
    #[zbus(property)]
    fn text_direction(&self) -> &str {
        "ltr"
    }
    #[zbus(property)]
    fn status(&self) -> &str {
        "normal"
    }
    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> {
        Vec::new()
    }

    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        _property_names: Vec<String>,
    ) -> zbus::fdo::Result<(u32, (i32, Props, Vec<Value<'static>>))> {
        let rev = REV.load(Ordering::SeqCst);
        let es = entries();
        if parent_id == 0 {
            let kids = if recursion_depth == 0 { Vec::new() } else { es.iter().map(leaf).collect() };
            return Ok((rev, (0, root_props(), kids)));
        }
        match es.iter().find(|e| e.id == parent_id) {
            Some(e) => Ok((rev, (e.id, props(e), Vec::new()))),
            None => Err(zbus::fdo::Error::InvalidArgs(format!("no menu item {parent_id}"))),
        }
    }

    fn get_group_properties(&self, ids: Vec<i32>, _property_names: Vec<String>) -> Vec<(i32, Props)> {
        let mut out = vec![(0, root_props())];
        out.extend(entries().iter().filter(|e| ids.is_empty() || ids.contains(&e.id)).map(|e| (e.id, props(e))));
        out.retain(|(id, _)| ids.is_empty() || ids.contains(id));
        out
    }

    fn get_property(&self, id: i32, name: String) -> zbus::fdo::Result<OwnedValue> {
        let p = if id == 0 {
            root_props()
        } else {
            entries().iter().find(|e| e.id == id).map(props).unwrap_or_default()
        };
        p.get(&name)
            .map(owned)
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs(format!("no property {name} on {id}")))
    }

    fn event(&self, id: i32, event_id: String, _data: Value<'_>, _timestamp: u32) {
        if event_id == "clicked" {
            clicked(id);
        }
    }

    fn event_group(&self, events: Vec<(i32, String, Value<'_>, u32)>) -> Vec<i32> {
        for (id, ev, _, _) in events {
            if ev == "clicked" {
                clicked(id);
            }
        }
        Vec::new()
    }

    /// True = "my layout may have changed, fetch it again" (the labels and checks are live state).
    fn about_to_show(&self, _id: i32) -> bool {
        true
    }

    fn about_to_show_group(&self, ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        (ids, Vec::new())
    }

    #[zbus(signal)]
    async fn layout_updated(ctxt: &SignalContext<'_>, revision: u32, parent: i32) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn item_activation_requested(ctxt: &SignalContext<'_>, id: i32, timestamp: u32) -> zbus::Result<()>;
}

/// A menu item was chosen. Runs on zbus's executor thread: keep it short, nothing here blocks.
fn clicked(id: i32) {
    match id {
        ID_SHOW => send(Action::Show),
        ID_FULL => send(Action::Full),
        ID_REGION => send(Action::Region),
        ID_RECORD => send(Action::Record),
        ID_RECORD_REGION => send(Action::RecordRegion),
        ID_OCR => send(Action::Ocr),
        ID_QUIT => send(Action::Quit),
        ID_CURSOR | ID_AUDIO | ID_LOGIN => {
            // Off the executor: the autostart write is a file write, and the refresh emits signals.
            std::thread::spawn(move || {
                match id {
                    ID_CURSOR => crate::settings::set_capture_cursor(!crate::settings::capture_cursor()),
                    ID_AUDIO => crate::settings::set_record_audio(!crate::settings::record_audio()),
                    _ => crate::autostart::set(!crate::autostart::is_enabled()),
                }
                notify(Note::Checks);
            });
        }
        _ => {}
    }
}

// === Signals ===

enum Note {
    /// Icon / tooltip / menu labels changed (recording started or ended).
    State,
    /// A check item flipped.
    Checks,
}

fn notifier() -> &'static Sender<Note> {
    static TX: OnceLock<Sender<Note>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = crossbeam_channel::unbounded::<Note>();
        // One thread so emits stay ordered, and so no handler ever blocks on the bus.
        std::thread::spawn(move || {
            while let Ok(n) = rx.recv() {
                let Some(conn) = CONN.get() else { continue };
                if matches!(n, Note::State) {
                    for name in ["NewIcon", "NewToolTip"] {
                        if let Err(e) = conn.emit_signal(None::<&str>, ITEM_PATH, ITEM_IFACE, name, &()) {
                            eprintln!("trontsnap: tray signal {name} failed: {e}");
                        }
                    }
                }
                let rev = REV.fetch_add(1, Ordering::SeqCst) + 1;
                if let Err(e) = conn.emit_signal(None::<&str>, MENU_PATH, MENU_IFACE, "LayoutUpdated", &(rev, 0i32)) {
                    eprintln!("trontsnap: tray LayoutUpdated failed: {e}");
                }
            }
        });
        tx
    })
}

fn notify(n: Note) {
    let _ = notifier().send(n);
}

// === Registration ===

fn bus_name() -> String {
    format!("org.kde.StatusNotifierItem-{}-1", std::process::id())
}

fn build() -> zbus::Result<Connection> {
    zbus::blocking::connection::Builder::session()?
        .name(bus_name())?
        .serve_at(ITEM_PATH, Item)?
        .serve_at(MENU_PATH, Menu)?
        .build()
}

/// Tell the watcher about us and ask whether anything will actually draw the item.
fn register(conn: &Connection) -> bool {
    let go = || -> zbus::Result<bool> {
        let w = zbus::blocking::Proxy::new(conn, WATCHER, "/StatusNotifierWatcher", WATCHER)?;
        w.call::<_, _, ()>("RegisterStatusNotifierItem", &(bus_name().as_str(),))?;
        // No host = nobody draws it: hiding the window then would strand it.
        Ok(w.get_property::<bool>("IsStatusNotifierHostRegistered").unwrap_or(false))
    };
    match go() {
        Ok(host) => host,
        Err(e) => {
            eprintln!("trontsnap: tray registration failed ({e})");
            false
        }
    }
}

/// The watcher thread: register now if a watcher exists, again whenever its owner changes. Runs for
/// the life of the process (one thread parked in a signal stream).
fn watch(conn: Connection) {
    let dbus = match zbus::blocking::fdo::DBusProxy::new(&conn) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("trontsnap: tray: no bus proxy ({e})");
            LOOKED.store(true, Ordering::SeqCst);
            return;
        }
    };
    // Subscribe BEFORE looking, or an owner that appears in between is missed.
    let owners = dbus.receive_name_owner_changed_with_args(&[(0, WATCHER)]);
    if dbus.name_has_owner(WATCHER.try_into().expect("static name")).unwrap_or(false) {
        REGISTERED.store(register(&conn), Ordering::SeqCst);
    }
    LOOKED.store(true, Ordering::SeqCst);
    let Ok(owners) = owners else { return };
    for sig in owners {
        let Ok(args) = sig.args() else { continue };
        if args.new_owner().is_some() {
            let ok = register(&conn);
            REGISTERED.store(ok, Ordering::SeqCst);
            eprintln!("trontsnap: tray re-registered with a new watcher (host: {ok})");
        } else {
            REGISTERED.store(false, Ordering::SeqCst);
            eprintln!("trontsnap: tray watcher went away");
            // A hidden window with no tray is unreachable: bring it back.
            send(Action::Show);
        }
    }
}

/// Bring the tray up. Returns whether it is registered, waiting up to `patience` for a watcher that
/// is not there yet (GNOME at login: the extension can load after our autostart). A missing watcher
/// is answered at once, so a tray-less desktop pays only for the bus round trip. The watcher thread
/// keeps going either way, so a tray that shows up later still gets the item.
pub fn start(patience: Duration) -> bool {
    if CONN.get().is_none() {
        match build() {
            Ok(conn) => {
                let _ = CONN.set(conn.clone());
                std::thread::spawn(move || watch(conn));
            }
            Err(e) => {
                eprintln!("trontsnap: no tray ({e})");
                return false;
            }
        }
    }
    let t0 = Instant::now();
    while !LOOKED.load(Ordering::SeqCst) && t0.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(10));
    }
    while !registered() && t0.elapsed() < patience {
        std::thread::sleep(Duration::from_millis(50));
    }
    registered()
}

// === Recording badge ===

/// A recording started: badge the icon (returns at once).
pub fn show() {
    WANT.fetch_add(1, Ordering::SeqCst);
    notify(Note::State);
}

/// A recording ended (delivered or failed). The badge goes with the last one.
pub fn hide() {
    WANT.fetch_sub(1, Ordering::SeqCst);
    notify(Note::State);
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
