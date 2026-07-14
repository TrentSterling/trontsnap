// The persistent TrontSnap app: system-tray resident, owns the global hotkeys,
// and hosts the history gallery window.
//
// Threads:
//   - eframe/winit (this thread): draws the gallery, keeps the pre-warmed region
//     overlay viewport alive, pumps the tray message window, handles tray clicks.
//   - keyboard hook (see keyhook.rs): a WH_KEYBOARD_LL hook that catches
//     PrintScreen / Ctrl+PrintScreen system-wide even while the gallery is hidden.
//   - single-instance acceptor: a second launch pokes us to show the window.
//
// PrintScreen = fullscreen grab (in-process). Ctrl+PrintScreen = region: the hotkey
// thread grabs the frozen frame and update() repurposes THIS window into a fullscreen
// borderless picker (no subprocess, no new GL context) — no launch cost, no black flash.

use std::io::Write as _;
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::num::NonZeroIsize;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use image::RgbaImage;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::autostart;
use crate::capture;
use crate::gallery::Gallery;
use crate::keyhook::{HotkeyEvent, KeyboardHook};
use crate::overlay::{build_rect_map, enumerate_windows_raw, RegionOutcome, RegionPicker};

const PORT: u16 = 48761;

/// A freshly grabbed monitor image + pre-enumerated smart-capture window rects,
/// handed from the hotkey thread to the UI thread to enter in-process region mode.
struct RegionCapture {
    img: RgbaImage,
    windows: Vec<egui::Rect>,
}

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
        Box::new(move |cc| match App::new(cc, listener, start_hidden) {
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
    // In-process region capture (no subprocess): the hotkey thread grabs the frame
    // and sends it here; update() repurposes this window into the fullscreen picker.
    region_tx: Sender<RegionCapture>,
    region_rx: Receiver<RegionCapture>,
    region: Option<RegionPicker>,
    window_shown: bool,
    was_visible_before_region: bool,
}

impl App {
    fn new(
        cc: &eframe::CreationContext<'_>,
        listener: TcpListener,
        start_hidden: bool,
    ) -> anyhow::Result<Self> {
        crate::theme::apply(&cc.egui_ctx);

        let (region_tx, region_rx) = crossbeam_channel::unbounded::<RegionCapture>();

        // Global hotkeys via a low-level keyboard hook (best-effort — if it can't
        // install, the gallery still works). See keyhook.rs for why RegisterHotkey
        // was unreliable for bare PrintScreen.
        let hotkeys = match setup_hotkeys(cc.egui_ctx.clone(), region_tx.clone()) {
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
            region_tx,
            region_rx,
            region: None,
            window_shown: !start_hidden,
            was_visible_before_region: true,
        })
    }

    fn show(&mut self, ctx: &egui::Context) {
        self.window_shown = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        // No rescan here: the live capture watcher keeps the gallery current, so
        // opening the window no longer flashes/reloads. Manual Refresh still exists.
    }

    fn hide(&mut self, ctx: &egui::Context) {
        self.window_shown = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Keep ticking so tray/second-instance events are serviced even when hidden.
        ctx.request_repaint_after(Duration::from_millis(150));

        // In-process region capture: a fresh frozen frame arrived from the hotkey
        // thread. Repurpose THIS (already-warm) window into a fullscreen borderless
        // picker — no subprocess, no new GL context, so no launch cost and no black
        // flash. All viewport commands apply before this frame paints.
        if self.region.is_none() {
            // Drain to the newest queued capture only: if several presses stacked
            // up (e.g. while hidden/throttled), only the freshest frame should open
            // a picker — never a stale one sitting behind it in the channel.
            let mut latest: Option<RegionCapture> = None;
            while let Ok(cap) = self.region_rx.try_recv() {
                latest = Some(cap);
            }
            if let Some(RegionCapture { img, windows }) = latest {
                self.was_visible_before_region = self.window_shown;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                    egui::viewport::WindowLevel::AlwaysOnTop,
                ));
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                self.region = Some(RegionPicker::new(img, windows));
            }
        }

        if self.region.is_some() {
            let mut done = false;
            egui::CentralPanel::default()
                .frame(egui::Frame::none())
                .show(ctx, |ui| {
                    if let Some(picker) = &mut self.region {
                        if let RegionOutcome::Done = picker.ui(ctx, ui) {
                            done = true;
                        }
                    }
                });
            if done {
                if self.was_visible_before_region {
                    // Gallery was open: exit fullscreen in place. Measured
                    // flash-free (winit's fullscreen-exit on a visible window is a
                    // single-frame cut on this stack).
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                        egui::viewport::WindowLevel::Normal,
                    ));
                } else {
                    // Tray-hidden target: hide FIRST, before touching Fullscreen/
                    // Decorations/WindowLevel. With Visible(false) sent LAST, DWM
                    // plays a ~130-150ms fade-to-dark while the AlwaysOnTop
                    // fullscreen-exit is still on screen — that fade IS the reported
                    // close flash. Hiding first means the geometry restore happens
                    // on an already-invisible window: DWM has nothing to composite,
                    // so it just vanishes in one frame (probe-verified).
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                        egui::viewport::WindowLevel::Normal,
                    ));
                    self.window_shown = false;
                }
                self.region = None;
                // Discard any capture(s) queued while this session was open (a
                // second Ctrl+PrtSc mid-picker), so the next frame doesn't
                // resurrect the picker with stale mid-session pixels.
                while self.region_rx.try_recv().is_ok() {}
            }
            return; // don't lay out the gallery underneath the overlay this frame
        }

        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            if ev.id == self.open_id {
                self.show(ctx);
            } else if ev.id == self.full_id {
                do_full();
            } else if ev.id == self.region_id {
                // Grab off the UI thread (xcap is ~90ms) then hand to update().
                let ctx2 = ctx.clone();
                let tx = self.region_tx.clone();
                std::thread::spawn(move || start_region(&ctx2, &tx));
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
/// do_full(); Ctrl+PrintScreen grabs the frame here (off the UI thread) and hands it
/// to the UI thread, which repurposes its own window into the picker — no subprocess.
fn setup_hotkeys(
    ctx: egui::Context,
    region_tx: Sender<RegionCapture>,
) -> anyhow::Result<KeyboardHook> {
    let (hook, rx) = KeyboardHook::install()?;
    std::thread::spawn(move || {
        while let Ok(ev) = rx.recv() {
            match ev {
                HotkeyEvent::Full => do_full(),
                HotkeyEvent::Region => start_region(&ctx, &region_tx),
            }
        }
    });
    Ok(hook)
}

/// Grab the primary monitor and enumerate smart-capture window targets (both off
/// the UI thread — xcap is ~90ms), then hand both to the UI thread to enter region
/// mode. Enumeration runs on a nested thread concurrently with the grab, so it
/// costs ~0 extra wall-clock and never touches the UI thread.
fn start_region(ctx: &egui::Context, region_tx: &Sender<RegionCapture>) {
    // Nudge the loop awake early — if the window is tray-hidden and idling on the
    // 150ms tick, warm it up before the ~90ms capture even finishes.
    ctx.request_repaint();
    let enum_handle = std::thread::spawn(enumerate_windows_raw);
    match capture::grab_primary() {
        Ok(img) => {
            let raw = enum_handle.join().unwrap_or_default();
            let windows = build_rect_map(&raw, img.width() as i32, img.height() as i32);
            let _ = region_tx.send(RegionCapture { img, windows });
            ctx.request_repaint();
        }
        Err(e) => {
            let _ = enum_handle.join();
            eprintln!("trontsnap: region capture failed: {e:#}");
        }
    }
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
