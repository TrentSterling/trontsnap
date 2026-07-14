// The persistent TrontSnap app: system-tray resident, owns the global hotkeys,
// and hosts the history gallery window.
//
// Threads:
//   - eframe/winit (this thread): draws the gallery, pumps the tray message window,
//     handles tray clicks.
//   - keyboard hook (see keyhook.rs): a WH_KEYBOARD_LL hook that catches
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder, TrayIconEvent};

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
    open_id: MenuId,
    full_id: MenuId,
    region_id: MenuId,
    auto_id: MenuId,
    auto_item: CheckMenuItem,
    quit_id: MenuId,
    show_flag: Arc<AtomicBool>,
    want_quit: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, listener: TcpListener) -> anyhow::Result<Self> {
        crate::theme::apply(&cc.egui_ctx);

        // Global hotkeys via a low-level keyboard hook (best-effort — if it can't
        // install, the gallery still works). See keyhook.rs for why RegisterHotkey
        // was unreliable for bare PrintScreen.
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
        let auto_i = CheckMenuItem::new("Start at login", true, autostart::is_enabled(), None);
        let quit_i = MenuItem::new("Quit TrontSnap", true, None);
        menu.append(&open_i)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&full_i)?;
        menu.append(&region_i)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&auto_i)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&quit_i)?;

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("TrontSnap  —  PrtSc = full, Ctrl+PrtSc = region")
            .with_icon(tray_icon_image())
            .build()?;

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
            open_id: open_i.id().clone(),
            full_id: full_i.id().clone(),
            region_id: region_i.id().clone(),
            auto_id: auto_i.id().clone(),
            auto_item: auto_i,
            quit_id: quit_i.id().clone(),
            show_flag,
            want_quit: false,
        })
    }

    fn show(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        // No rescan here: the live capture watcher keeps the gallery current, so
        // opening the window no longer flashes/reloads. Manual Refresh still exists.
    }

    fn hide(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Keep ticking so tray/second-instance events are serviced even when hidden.
        ctx.request_repaint_after(Duration::from_millis(150));

        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            if ev.id == self.open_id {
                self.show(ctx);
            } else if ev.id == self.full_id {
                do_full();
            } else if ev.id == self.region_id {
                std::thread::spawn(region_win32::capture_region);
            } else if ev.id == self.auto_id {
                autostart::set(self.auto_item.is_checked());
            } else if ev.id == self.quit_id {
                self.want_quit = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }

        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::DoubleClick { .. } = ev {
                self.show(ctx);
            }
        }

        if self.show_flag.swap(false, Ordering::SeqCst) {
            self.show(ctx);
        }

        // Closing the window hides to tray instead of quitting.
        if ctx.input(|i| i.viewport().close_requested()) && !self.want_quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.hide(ctx);
        }

        let hwnd = hwnd_from_frame(frame);
        egui::CentralPanel::default().show(ctx, |ui| {
            self.gallery.ui(ui, ctx, hwnd);
        });
    }
}

/// Install the WH_KEYBOARD_LL hook and start the consumer thread. PrintScreen runs
/// do_full(); Ctrl+PrintScreen spawns the dedicated Win32 region overlay on its own
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
            }
        }
    });
    Ok(hook)
}

fn do_full() {
    // Run off the hotkey thread so PrintScreen returns instantly. deliver() saves
    // + copies to the clipboard (all formats) + plays the shutter.
    std::thread::spawn(|| match capture::grab_primary() {
        Ok(img) => match capture::deliver(&img) {
            Ok(path) => eprintln!("trontsnap: full -> {}", path.display()),
            Err(e) => eprintln!("trontsnap: full deliver failed: {e:#}"),
        },
        Err(e) => eprintln!("trontsnap: full capture failed: {e:#}"),
    });
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

/// Decode the embedded face (Trent's avatar) resized to `size`, or None on failure.
fn face_rgba(size: u32) -> Option<Vec<u8>> {
    let bytes = include_bytes!("../assets/tront.png");
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
