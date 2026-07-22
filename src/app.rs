// The persistent TrontSnap app: system-tray resident, owns the global hotkeys,
// and hosts the history gallery window.
//
// Threads:
//   - eframe/winit (this thread): draws the gallery, pumps the tray message window,
//     handles tray clicks.
//   - hotkey pump (see keyhook.rs): RegisterHotKey-based, catches
//     PrintScreen / Ctrl+PrintScreen system-wide even while the gallery is hidden.
//   - single-instance acceptor: a second launch pokes us to show the window.
//   - region overlay (see region_win32.rs): each Ctrl+PrintScreen spawns a thread
//     that puts up a DEDICATED fullscreen Win32/GDI picker. The gallery window is
//     never touched, so there is no maximize/restore/focus churn.
//
// PrintScreen = fullscreen grab. Ctrl+PrintScreen = dedicated GDI region picker.

use std::io::Write as _;
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::num::NonZeroIsize;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::Receiver;
use eframe::egui;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow, ShowWindow, SW_RESTORE,
};

use crate::autostart;
use crate::capture;
use crate::gallery::Gallery;
use crate::keyhook::{HotkeyEvent, KeyboardHook};
use crate::region_win32;

const PORT: u16 = 48761;

pub fn run(start_hidden: bool) -> anyhow::Result<()> {
    // Single instance: bind a loopback port. If it's taken, another TrontSnap is
    // running — poke it to surface its window and exit quietly.
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, PORT)) {
        Ok(l) => l,
        Err(_) => {
            if let Ok(mut s) = TcpStream::connect((Ipv4Addr::LOCALHOST, PORT)) {
                let _ = s.write_all(b"open");
            }
            return Ok(());
        }
    };

    autostart::ensure_first_run();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("TrontSnap")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([680.0, 420.0])
            .with_visible(!start_hidden)
            .with_icon(Arc::new(app_icon())),
        ..Default::default()
    };

    eframe::run_native(
        "TrontSnap",
        options,
        Box::new(move |cc| match App::new(cc, listener) {
            Ok(app) => Ok(Box::new(app) as Box<dyn eframe::App>),
            Err(e) => Err(Box::<dyn std::error::Error + Send + Sync>::from(e.to_string())),
        }),
    )
    .map_err(|e| anyhow::anyhow!("app failed: {e}"))
}

pub struct App {
    gallery: Gallery,
    _hotkeys: Option<KeyboardHook>,
    _tray: TrayIcon,
    auto_id: MenuId,
    auto_item: CheckMenuItem,
    cursor_id: MenuId,
    cursor_item: CheckMenuItem,
    audio_id: MenuId,
    audio_item: CheckMenuItem,
    show_flag: Arc<AtomicBool>,
    // Self-heal for the intermittent restore-to-tiny desync (see update()): the last
    // content size seen at or above the window minimum, and how many consecutive frames
    // we've observed a sub-minimum (i.e. desynced) size.
    good_size: egui::Vec2,
    small_frames: u32,
    // Autostart toggles (which need &self) are forwarded here for update() to apply.
    // Every other tray/menu action is done DIRECTLY in the event handlers, because
    // eframe does not run update() at all while the window is hidden to the tray, so
    // anything routed through it would just queue until the window happened to reappear.
    menu_rx: Receiver<MenuId>,
    // Last known-good main-window HWND, cached by update() so the tray menu handler can
    // restore the window directly via Win32 while it's hidden.
    hwnd_cell: Arc<AtomicIsize>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, listener: TcpListener) -> anyhow::Result<Self> {
        crate::theme::apply(&cc.egui_ctx);

        // Global hotkeys via RegisterHotKey (best-effort — if it can't install, the
        // gallery still works). See keyhook.rs.
        let hotkeys = match setup_hotkeys() {
            Ok(m) => Some(m),
            Err(e) => {
                eprintln!("trontsnap: hotkeys unavailable: {e:#}");
                None
            }
        };

        // Tray icon + menu.
        let menu = Menu::new();
        let open_i = MenuItem::new("Open TrontSnap", true, None);
        let full_i = MenuItem::new("Capture Fullscreen   (PrtSc)", true, None);
        let region_i = MenuItem::new("Capture Region   (Ctrl+PrtSc)", true, None);
        let record_i = MenuItem::new("Record Region   (Ctrl+Shift+PrtSc)", true, None);
        let cursor_i =
            CheckMenuItem::new("Capture cursor", true, crate::settings::capture_cursor(), None);
        let audio_i =
            CheckMenuItem::new("Record audio", true, crate::settings::record_audio(), None);
        let auto_i = CheckMenuItem::new("Start at login", true, autostart::is_enabled(), None);
        let quit_i = MenuItem::new("Quit TrontSnap", true, None);
        menu.append(&open_i)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&full_i)?;
        menu.append(&region_i)?;
        menu.append(&record_i)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&cursor_i)?;
        menu.append(&audio_i)?;
        menu.append(&auto_i)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&quit_i)?;

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            // Left-click starts a region capture (handled below); the menu opens on
            // right-click only.
            .with_menu_on_left_click(false)
            .with_tooltip("TrontSnap: left-click = region, right-click = menu")
            .with_icon(tray_icon_image())
            .build()?;

        // Handle tray + menu actions DIRECTLY in these handlers. They fire on the UI
        // thread when the event is dispatched, so a Win32 restore / a spawned capture
        // works immediately even while the window is hidden. Routing them through
        // update() does NOT work: eframe never runs update() while the window is hidden
        // to the tray, so those actions would silently queue until the window reappeared.
        // Only the autostart toggle (needs &self) is forwarded to update().
        let hwnd_cell = Arc::new(AtomicIsize::new(0));
        let (menu_tx, menu_rx) = crossbeam_channel::unbounded::<MenuId>();
        {
            let ctx = cc.egui_ctx.clone();
            let cell = hwnd_cell.clone();
            let open = open_i.id().clone();
            let full = full_i.id().clone();
            let region = region_i.id().clone();
            let record = record_i.id().clone();
            let quit = quit_i.id().clone();
            MenuEvent::set_event_handler(Some(move |ev: MenuEvent| {
                if ev.id == open {
                    let h = cell.load(Ordering::Relaxed);
                    if h != 0 {
                        restore_and_foreground(h);
                    }
                } else if ev.id == full {
                    do_full();
                } else if ev.id == region {
                    std::thread::spawn(region_win32::capture_region);
                } else if ev.id == record {
                    // Toggle: first click picks a region + records, second stops.
                    crate::recorder::toggle();
                } else if ev.id == quit {
                    std::process::exit(0);
                } else {
                    // Autostart toggle -> update() (reads the check state off &self).
                    let _ = menu_tx.send(ev.id);
                }
                ctx.request_repaint();
            }));
        }
        {
            let ctx = cc.egui_ctx.clone();
            TrayIconEvent::set_event_handler(Some(move |ev: TrayIconEvent| {
                // Left-click = instant region capture (its own window, independent of
                // the hidden gallery). Right-click opens the menu natively.
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = ev
                {
                    std::thread::spawn(region_win32::capture_region);
                }
                ctx.request_repaint();
            }));
        }

        // Single-instance acceptor: any connection means "show the window".
        let show_flag = Arc::new(AtomicBool::new(false));
        {
            let flag = show_flag.clone();
            let ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stream.is_ok() {
                        flag.store(true, Ordering::SeqCst);
                        ctx.request_repaint();
                    }
                }
            });
        }

        Ok(Self {
            gallery: Gallery::new(&cc.egui_ctx),
            _hotkeys: hotkeys,
            _tray: tray,
            auto_id: auto_i.id().clone(),
            auto_item: auto_i,
            cursor_id: cursor_i.id().clone(),
            cursor_item: cursor_i,
            audio_id: audio_i.id().clone(),
            audio_item: audio_i,
            show_flag,
            menu_rx,
            hwnd_cell,
            good_size: egui::vec2(1180.0, 760.0),
            small_frames: 0,
        })
    }

    fn show(&mut self, ctx: &egui::Context) {
        // Win32 restore is the reliable path (see restore_and_foreground). Use the
        // cached last-good HWND, since the current frame's HWND may be unavailable
        // while hidden. The viewport commands keep eframe's own state in sync.
        let h = self.hwnd_cell.load(Ordering::Relaxed);
        if h != 0 {
            restore_and_foreground(h);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        // No rescan here: the live capture watcher keeps the gallery current, so
        // opening the window no longer flashes/reloads. Manual Refresh still exists.
    }

    fn hide(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }

    /// Self-heal an intermittent winit/eframe desync where the gallery comes back from a
    /// tray/minimize restore sized far below its own minimum (observed once: it restored
    /// tiny and only snapped right after I nudged a resize handle). winit enforces
    /// `min_inner_size`, so the USER can never drag the content below it — a visible
    /// sub-minimum size therefore means the OS window and eframe's cached size have
    /// desynced. Detect that (debounced a few frames so a one-frame hide/show/DPI
    /// transition never triggers it) and re-assert the last known-good size. It can't
    /// fight a legitimate resize, because legitimate sizes are always >= the minimum.
    fn heal_tiny_window(&mut self, ctx: &egui::Context) {
        // update() doesn't run while hidden, so if we're here the window is live; still
        // skip while minimized (its reported size is meaningless then).
        if ctx.input(|i| i.viewport().minimized).unwrap_or(false) {
            return;
        }
        // Just under the 680x420 min_inner_size, so only genuine breakage trips it.
        const MIN_W: f32 = 670.0;
        const MIN_H: f32 = 410.0;
        let sz = ctx.input(|i| i.screen_rect()).size();
        if sz.x >= MIN_W && sz.y >= MIN_H {
            self.good_size = sz; // remember the user's actual size to restore to
            self.small_frames = 0;
        } else if sz.x > 1.0 && sz.y > 1.0 {
            // Non-degenerate but sub-minimum: the desync. Confirm it persists, then fix.
            self.small_frames += 1;
            if self.small_frames >= 4 {
                eprintln!(
                    "trontsnap: window desynced to {:.0}x{:.0}, restoring to {:.0}x{:.0}",
                    sz.x, sz.y, self.good_size.x, self.good_size.y
                );
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(self.good_size));
                self.small_frames = 0;
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Keep ticking so tray/second-instance events are serviced even when hidden.
        ctx.request_repaint_after(Duration::from_millis(150));

        let hwnd = hwnd_from_frame(frame);
        if let Some(h) = hwnd {
            self.hwnd_cell.store(h, Ordering::Relaxed);
        }

        self.heal_tiny_window(ctx);

        // Only the checkbox toggles come through here (they need &self to read their
        // check state); everything else is handled directly in the tray/menu handlers,
        // since update() doesn't run while the window is hidden to the tray.
        while let Ok(id) = self.menu_rx.try_recv() {
            if id == self.auto_id {
                autostart::set(self.auto_item.is_checked());
            } else if id == self.cursor_id {
                crate::settings::set_capture_cursor(self.cursor_item.is_checked());
            } else if id == self.audio_id {
                crate::settings::set_record_audio(self.audio_item.is_checked());
            }
        }

        if self.show_flag.swap(false, Ordering::SeqCst) {
            self.show(ctx);
        }

        // Closing the window hides to tray instead of quitting (Quit is in the menu).
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.hide(ctx);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            self.gallery.ui(ui, ctx, hwnd);
        });
    }
}

/// Register the RegisterHotKey-based hotkeys and start the consumer thread. PrintScreen
/// runs do_full(); Ctrl+PrintScreen spawns the dedicated Win32 region overlay on its own
/// thread (it blocks in a modal message loop until you pick or cancel).
fn setup_hotkeys() -> anyhow::Result<KeyboardHook> {
    let (hook, rx) = KeyboardHook::install()?;
    std::thread::spawn(move || {
        while let Ok(ev) = rx.recv() {
            match ev {
                HotkeyEvent::Full => do_full(),
                HotkeyEvent::Region => {
                    std::thread::spawn(region_win32::capture_region);
                }
                // Toggle: first press picks a region + starts recording, second stops.
                HotkeyEvent::Record => crate::recorder::toggle(),
            }
        }
    });
    Ok(hook)
}

fn do_full() {
    // Run off the hotkey thread so PrintScreen returns instantly. deliver() saves
    // + copies to the clipboard (all formats) + plays the shutter.
    std::thread::spawn(|| match capture::grab_for_shot() {
        Ok(img) => match capture::deliver(&img) {
            Ok(path) => eprintln!("trontsnap: full -> {}", path.display()),
            Err(e) => eprintln!("trontsnap: full deliver failed: {e:#}"),
        },
        Err(e) => eprintln!("trontsnap: full capture failed: {e:#}"),
    });
}

/// Reliably restore + foreground the main window via Win32. eframe's Visible(true)
/// leaves a tray-hidden window behind the focused app and doesn't un-minimize an
/// iconic one; SW_RESTORE handles both, and the attach-thread-input dance beats
/// Windows' foreground lock so the gallery actually pops to the front.
fn restore_and_foreground(hwnd: isize) {
    unsafe {
        let h = HWND(hwnd);
        let _ = ShowWindow(h, SW_RESTORE);
        let fg = GetForegroundWindow();
        let fg_thread = GetWindowThreadProcessId(fg, None);
        let our = GetCurrentThreadId();
        if fg_thread != 0 && fg_thread != our {
            let _ = AttachThreadInput(our, fg_thread, true);
            let _ = SetForegroundWindow(h);
            let _ = AttachThreadInput(our, fg_thread, false);
        } else {
            let _ = SetForegroundWindow(h);
        }
    }
}

/// Extract the raw Win32 HWND from eframe's window handle. eframe::Frame
/// implements raw_window_handle::HasWindowHandle directly (eframe 0.29), so no
/// FindWindowW/GetActiveWindow workaround is needed.
fn hwnd_from_frame(frame: &eframe::Frame) -> Option<isize> {
    match frame.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

/// Native OS file drag-out via the `drag` crate (DoDragDrop under the hood).
/// Blocking/modal on the calling thread by Win32 design; gallery.rs calls this
/// synchronously from update() (the UI thread), which is required.
pub fn start_file_drag(hwnd: Option<isize>, path: &Path) -> bool {
    let Some(hwnd) = hwnd.and_then(NonZeroIsize::new) else {
        return false;
    };
    let handle = RawHwnd(hwnd);
    let path = path.to_path_buf();
    let item = drag::DragItem::Files(vec![path.clone()]);
    let image = drag::Image::File(path);
    drag::start_drag(
        &handle,
        item,
        image,
        |_result, _cursor| {},
        drag::Options::default(),
    )
    .is_ok()
}

/// Thin HasWindowHandle wrapper so `drag::start_drag` can be handed a bare HWND
/// (as an isize) without keeping the whole eframe::Frame alive.
struct RawHwnd(NonZeroIsize);

impl HasWindowHandle for RawHwnd {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        let handle = raw_window_handle::Win32WindowHandle::new(self.0);
        // Safety: this HWND was read from eframe::Frame's own HasWindowHandle impl
        // this same frame, and stays valid for the drag's short, synchronous,
        // same-thread lifetime.
        unsafe {
            Ok(raw_window_handle::WindowHandle::borrow_raw(
                RawWindowHandle::Win32(handle),
            ))
        }
    }
}

/// Decode the embedded TrontSnap icon (the face in its rainbow capture-frame,
/// distinct from TrontEQ's bare face) resized to `size`, or None on failure.
fn face_rgba(size: u32) -> Option<Vec<u8>> {
    let bytes = include_bytes!("../assets/trontsnap.png");
    let img = image::load_from_memory(bytes).ok()?;
    let resized = img.resize_exact(size, size, image::imageops::FilterType::Lanczos3);
    Some(resized.to_rgba8().into_raw())
}

fn app_icon() -> egui::IconData {
    let size = 64;
    let rgba = face_rgba(size).unwrap_or_else(|| make_icon_rgba(size));
    egui::IconData { rgba, width: size, height: size }
}

fn tray_icon_image() -> tray_icon::Icon {
    let size = 32;
    let rgba = face_rgba(size).unwrap_or_else(|| make_icon_rgba(size));
    tray_icon::Icon::from_rgba(rgba, size, size).expect("valid tray icon")
}

/// A simple TrontSnap glyph: accent rounded square with a light camera lens.
fn make_icon_rgba(size: u32) -> Vec<u8> {
    let s = size as f32;
    let inset = s * 0.08;
    let corner = s * 0.24;
    let (cx, cy) = (s / 2.0, s / 2.0);
    let lens = s * 0.22;

    let mut buf = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            let i = ((y * size + x) * 4) as usize;
            let mut px = [0u8, 0, 0, 0];
            if rr_contains(fx, fy, inset, s - inset, corner) {
                px = [90, 209, 255, 255];
                let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
                if d < lens {
                    px = [245, 250, 255, 255];
                }
                if d < lens * 0.5 {
                    px = [90, 209, 255, 255];
                }
            }
            buf[i..i + 4].copy_from_slice(&px);
        }
    }
    buf
}

/// Rounded-rectangle coverage test (SDF-style), for the icon corners.
fn rr_contains(x: f32, y: f32, lo: f32, hi: f32, r: f32) -> bool {
    let qx = ((lo + r) - x).max(x - (hi - r)).max(0.0);
    let qy = ((lo + r) - y).max(y - (hi - r)).max(0.0);
    qx * qx + qy * qy <= r * r
}
