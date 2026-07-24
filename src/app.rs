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
            .with_icon(Arc::new(app_icon()))
            // Custom chrome: we draw our own title bar (tabs + window buttons) and
            // handle move/resize ourselves (see App::title_bar / App::edge_resize).
            .with_decorations(false),
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

/// The top-level tabs in the custom title bar's tab strip. Capture used to be a
/// tab of its own (three lonely buttons in an otherwise empty page); it's now a
/// quick-menu in the chrome instead (see `title_bar`), so it isn't one of these.
#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Gallery,
    Settings,
    About,
}

/// Which glyph a painted window button draws. The real four-button set (min /
/// max / restore / close) always renders crisp lines/rects instead of relying
/// on font glyphs — "🗖"/"❐"/"✕" render as tofu on some systems.
#[derive(Clone, Copy, PartialEq)]
enum WinBtn {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// Footprint the caption overlay occupies at the top-right (3 buttons + frame
/// margins). The chrome row reserves this much so its content normally stops
/// short of the buttons; at extreme widths the opaque overlay simply wins.
const CAPTION_W: f32 = 3.0 * 40.0 + 10.0;

/// Paint one window-chrome button: a rounded hover fill (reddish for close,
/// theme hover tint otherwise) plus a hand-drawn glyph, so it never depends on
/// the font having the right symbol. Returns the Response so the caller can
/// still do `.on_hover_text(...).clicked()`.
fn window_button(ui: &mut egui::Ui, kind: WinBtn) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(40.0, 30.0), egui::Sense::click());
    let painter = ui.painter();

    if resp.hovered() {
        let fill = match kind {
            WinBtn::Close => egui::Color32::from_rgb(200, 60, 60),
            _ => crate::theme::t().widget_hover,
        };
        painter.rect_filled(rect, 4.0, fill);
    }

    let stroke = egui::Stroke::new(1.3, crate::theme::t().text_primary);
    let c = rect.center();
    match kind {
        WinBtn::Minimize => {
            painter.line_segment(
                [egui::pos2(c.x - 5.0, c.y), egui::pos2(c.x + 5.0, c.y)],
                stroke,
            );
        }
        WinBtn::Maximize => {
            let r = egui::Rect::from_center_size(c, egui::vec2(10.0, 10.0));
            painter.rect_stroke(r, 1.0, stroke);
        }
        WinBtn::Restore => {
            let back = egui::Rect::from_center_size(c + egui::vec2(2.5, -2.5), egui::vec2(8.0, 8.0));
            let front = egui::Rect::from_center_size(c + egui::vec2(-2.0, 2.0), egui::vec2(8.0, 8.0));
            painter.rect_stroke(back, 1.0, stroke);
            painter.rect_stroke(front, 1.0, stroke);
        }
        WinBtn::Close => {
            painter.line_segment(
                [egui::pos2(c.x - 5.0, c.y - 5.0), egui::pos2(c.x + 5.0, c.y + 5.0)],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(c.x - 5.0, c.y + 5.0), egui::pos2(c.x + 5.0, c.y - 5.0)],
                stroke,
            );
        }
    }
    resp
}

pub struct App {
    gallery: Gallery,
    tab: Tab,
    // Small app-icon texture for the title bar wordmark and the About tab; loaded
    // once at startup (egui textures must not be re-uploaded every frame).
    icon_tex: egui::TextureHandle,
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
        // Chrome text (wordmark, tabs, labels) shouldn't show an I-beam or highlight
        // like an edit field — this is a native app chrome, not a document.
        cc.egui_ctx.style_mut(|s| s.interaction.selectable_labels = false);

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

        let icon_tex = {
            let size = 64u32;
            let rgba = face_rgba(size).unwrap_or_else(|| make_icon_rgba(size));
            let img = egui::ColorImage::from_rgba_unmultiplied([size as usize, size as usize], &rgba);
            cc.egui_ctx.load_texture("trontsnap-icon", img, egui::TextureOptions::default())
        };

        // First run EVER opens on About (welcome + author credit), then flips a flag so
        // every launch after opens on Gallery — unless the user opted into "Show this tab
        // on launch". About stays one click away in the tab strip regardless.
        let tab = if !crate::settings::has_run_before() {
            crate::settings::set_has_run();
            Tab::About
        } else if crate::settings::show_about_on_launch() {
            Tab::About
        } else {
            Tab::Gallery
        };

        Ok(Self {
            gallery: Gallery::new(&cc.egui_ctx),
            tab,
            icon_tex,
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

    /// Draw the single top chrome: app icon + wordmark, the Capture quick-menu, the
    /// tab strip, (on the Gallery tab) the filter chips/count/legend inline, a
    /// draggable middle strip, and minimize/maximize/close buttons — all in ONE
    /// row / one `TopBottomPanel`. This replaces the OS title bar entirely, so it
    /// also owns window move (StartDrag / double-click-to-maximize) and the
    /// close-hides-to-tray behavior. No separate header strip inside the gallery
    /// body, no bottom bar — everything lives in this single chrome row.
    fn title_bar(&mut self, ctx: &egui::Context) {
        let maximized = ctx.input(|i| i.viewport().maximized).unwrap_or(false);
        let on_gallery = self.tab == Tab::Gallery;

        egui::TopBottomPanel::top("trontsnap_chrome_main")
            .exact_height(40.0)
            .frame(
                egui::Frame::none()
                    // Derived from visuals (translucent when the gradient wash is
                    // on, see theme::build_visuals) rather than the raw solid
                    // token, so the wash reads through the top chrome too.
                    .fill(ctx.style().visuals.panel_fill)
                    .inner_margin(egui::Margin::symmetric(10.0, 0.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    // Logo + wordmark are drag handles (boxel-campaign rule:
                    // a packed chrome row always keeps a grab point) — drag
                    // moves the window, double-click toggles maximize.
                    let drag_handle = |ctx: &egui::Context, resp: &egui::Response| {
                        if resp.drag_started() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                        } else if resp.double_clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                        }
                    };
                    let logo = ui.add(
                        egui::Image::new(&self.icon_tex)
                            .fit_to_exact_size(egui::vec2(22.0, 22.0))
                            .sense(egui::Sense::click_and_drag()),
                    );
                    drag_handle(ctx, &logo);
                    ui.add_space(6.0);
                    let wm = ui.add(egui::Label::new(
                        egui::RichText::new("TrontSnap")
                            .color(crate::theme::t().accent)
                            .strong()
                            .size(16.0),
                    )
                    .sense(egui::Sense::click_and_drag()));
                    drag_handle(ctx, &wm);
                    ui.add_space(16.0);

                    // Uniform nav strip: the Capture action-menu and the three view tabs
                    // share one spacing + one visual treatment, so the row reads as a
                    // single clean group instead of a boxed button next to plain tabs.
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.scope(|ui| {
                        // Frameless at rest so the menu button matches the borderless
                        // tabs; hover + open still use the shared accent visuals below.
                        let v = ui.visuals_mut();
                        v.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
                        v.widgets.inactive.bg_fill = egui::Color32::TRANSPARENT;
                        v.widgets.inactive.bg_stroke = egui::Stroke::NONE;
                        ui.menu_button("Capture", |ui| {
                            if ui.button("Fullscreen   (PrtSc)").clicked() {
                                do_full();
                                ui.close_menu();
                            }
                            if ui.button("Region   (Ctrl+PrtSc)").clicked() {
                                std::thread::spawn(region_win32::capture_region);
                                ui.close_menu();
                            }
                            if ui.button("Record   (Ctrl+Shift+PrtSc)").clicked() {
                                crate::recorder::toggle();
                                ui.close_menu();
                            }
                        });
                    });

                    ui.selectable_value(&mut self.tab, Tab::Gallery, "Gallery");
                    ui.selectable_value(&mut self.tab, Tab::Settings, "Settings");
                    ui.selectable_value(&mut self.tab, Tab::About, "About");

                    // Filter chips + count + legend, inline in the same row (Gallery only).
                    // May clip on very narrow windows — acceptable, the window buttons
                    // below stay right-aligned and always visible.
                    if on_gallery {
                        ui.add_space(14.0);
                        ui.separator();
                        self.gallery.filter_bar_ui(ui);
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // The min/max/close buttons live in a floating top-right
                        // overlay (caption_overlay) so a cramped chrome row (long
                        // filter bar, narrow window) can never draw through them.
                        // Here we only reserve their footprint.
                        ui.add_space(CAPTION_W);

                        // Whatever's left between the tabs (+ filters) and the buttons is
                        // the window's drag handle (move on drag, maximize on double-click).
                        let (_, drag) = ui.allocate_exact_size(
                            ui.available_size(),
                            egui::Sense::click_and_drag(),
                        );
                        if drag.drag_started() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                        } else if drag.double_clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                        }
                    });
                });
            });
        self.caption_overlay(ctx, maximized);
    }

    /// The caption buttons as an always-on-top overlay pinned to the window's
    /// top-right corner with an opaque panel background: whatever the chrome row
    /// squeezes underneath them at narrow widths, the buttons stay visible and
    /// clickable — nothing draws through.
    fn caption_overlay(&mut self, ctx: &egui::Context, maximized: bool) {
        egui::Area::new(egui::Id::new("trontsnap_caption_overlay"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                egui::Frame::none()
                    // Same translucent-when-gradient fill as the chrome row above.
                    .fill(ui.visuals().panel_fill)
                    .rounding(egui::Rounding { sw: 6.0, ..Default::default() })
                    .inner_margin(egui::Margin {
                        left: 6.0,
                        right: 4.0,
                        top: 5.0,
                        bottom: 5.0,
                    })
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.horizontal(|ui| {
                            if window_button(ui, WinBtn::Minimize)
                                .on_hover_text("Minimize")
                                .clicked()
                            {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                            }
                            let max_kind =
                                if maximized { WinBtn::Restore } else { WinBtn::Maximize };
                            if window_button(ui, max_kind)
                                .on_hover_text("Maximize / restore")
                                .clicked()
                            {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(
                                    !maximized,
                                ));
                            }
                            if window_button(ui, WinBtn::Close)
                                .on_hover_text("Close to tray")
                                .clicked()
                            {
                                self.hide(ctx);
                            }
                        });
                    });
            });
    }

    /// Thin invisible interact strips along the 4 edges + 4 corners, so the window
    /// can still be resized by dragging even though `.with_decorations(false)`
    /// removed the OS resize border. Each strip is its own foreground `Area` so it
    /// takes priority over whatever tab content sits underneath it. Skipped while
    /// maximized (nothing to resize against).
    fn edge_resize(&self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().maximized).unwrap_or(false) {
            return;
        }
        use egui::viewport::ResizeDirection as Dir;
        use egui::CursorIcon as Cur;

        let screen = ctx.input(|i| i.screen_rect());
        const EDGE: f32 = 6.0;
        const CORNER: f32 = 10.0;

        let strips: [(egui::Rect, Dir, Cur); 8] = [
            (
                egui::Rect::from_min_max(
                    egui::pos2(screen.left() + CORNER, screen.top()),
                    egui::pos2(screen.right() - CORNER, screen.top() + EDGE),
                ),
                Dir::North,
                Cur::ResizeVertical,
            ),
            (
                egui::Rect::from_min_max(
                    egui::pos2(screen.left() + CORNER, screen.bottom() - EDGE),
                    egui::pos2(screen.right() - CORNER, screen.bottom()),
                ),
                Dir::South,
                Cur::ResizeVertical,
            ),
            (
                egui::Rect::from_min_max(
                    egui::pos2(screen.left(), screen.top() + CORNER),
                    egui::pos2(screen.left() + EDGE, screen.bottom() - CORNER),
                ),
                Dir::West,
                Cur::ResizeHorizontal,
            ),
            (
                egui::Rect::from_min_max(
                    egui::pos2(screen.right() - EDGE, screen.top() + CORNER),
                    egui::pos2(screen.right(), screen.bottom() - CORNER),
                ),
                Dir::East,
                Cur::ResizeHorizontal,
            ),
            (
                egui::Rect::from_min_size(screen.left_top(), egui::vec2(CORNER, CORNER)),
                Dir::NorthWest,
                Cur::ResizeNwSe,
            ),
            (
                egui::Rect::from_min_size(
                    egui::pos2(screen.right() - CORNER, screen.top()),
                    egui::vec2(CORNER, CORNER),
                ),
                Dir::NorthEast,
                Cur::ResizeNeSw,
            ),
            (
                egui::Rect::from_min_size(
                    egui::pos2(screen.left(), screen.bottom() - CORNER),
                    egui::vec2(CORNER, CORNER),
                ),
                Dir::SouthWest,
                Cur::ResizeNeSw,
            ),
            (
                egui::Rect::from_min_size(
                    egui::pos2(screen.right() - CORNER, screen.bottom() - CORNER),
                    egui::vec2(CORNER, CORNER),
                ),
                Dir::SouthEast,
                Cur::ResizeNwSe,
            ),
        ];

        for (i, (rect, dir, cursor)) in strips.into_iter().enumerate() {
            if rect.width() <= 0.0 || rect.height() <= 0.0 {
                continue; // window smaller than 2*CORNER while animating/restoring
            }
            let id = egui::Id::new("trontsnap_resize_edge").with(i);
            egui::Area::new(id)
                .order(egui::Order::Foreground)
                .fixed_pos(rect.min)
                .show(ctx, |ui| {
                    let (_, resp) =
                        ui.allocate_exact_size(rect.size(), egui::Sense::click_and_drag());
                    if resp.hovered() {
                        ui.output_mut(|o| o.cursor_icon = cursor);
                    }
                    if resp.drag_started() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
                    }
                });
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Keep ticking so tray/second-instance events are serviced even when hidden.
        ctx.request_repaint_after(Duration::from_millis(150));

        // Clamp UI zoom to a usable range. Extreme zoom-out makes the huge virtualized
        // gallery grid thrash and flicker; extreme zoom-in lets the min content size push
        // the OS window bigger. Ctrl+scroll and Ctrl +/- still work inside this range.
        let zoom = ctx.zoom_factor();
        let clamped = zoom.clamp(0.5, 2.0);
        if (clamped - zoom).abs() > f32::EPSILON {
            ctx.set_zoom_factor(clamped);
        }

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

        // Discord-style background wash: one quad painted into the background
        // layer before any panel draws (panel fills go translucent to let it
        // read through — see theme::build_visuals). Default ON; the Settings >
        // Appearance checkbox flips crate::settings::gradient().
        if crate::settings::gradient() {
            crate::theme::paint_gradient(ctx, &crate::theme::t());
        }

        self.title_bar(ctx);

        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Gallery => self.gallery.ui(ui, ctx, hwnd),
            Tab::Settings => settings_tab_ui(ui),
            Tab::About => self.about_tab_ui(ui),
        });

        self.edge_resize(ctx);
    }
}

/// Settings tab: sectioned toggle rows backed directly by the same functions the
/// tray menu checkboxes use (settings.rs / autostart.rs / printscreen.rs): the
/// tray menu and this tab read/write the same state, so either one stays in sync
/// with the other. Scrollable because the Hotkeys section makes it taller than
/// the window on smaller sizes.
fn settings_tab_ui(ui: &mut egui::Ui) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::none().inner_margin(egui::Margin::symmetric(24.0, 16.0)).show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_max_width(560.0);

                settings_section_header(ui, "Appearance");
                ui.label(
                    egui::RichText::new(format!("Current theme: {}", crate::settings::theme_name()))
                        .small()
                        .color(crate::theme::t().text_muted),
                );
                ui.add_space(10.0);

                let mut accent = crate::theme::t().accent;
                ui.horizontal(|ui| {
                    ui.label("Accent color");
                    ui.add_space(8.0);
                    // The default color swatch renders as a thin sliver (it's sized to
                    // interact_size); bump it to a proper, obviously-clickable box.
                    let changed = ui
                        .scope(|ui| {
                            ui.spacing_mut().interact_size = egui::vec2(96.0, 30.0);
                            ui.color_edit_button_srgba(&mut accent).changed()
                        })
                        .inner;
                    if changed {
                        let rgb = [accent.r(), accent.g(), accent.b()];
                        let tokens = crate::theme::from_accent(rgb);
                        crate::theme::set_theme(ui.ctx(), tokens);
                        crate::settings::set_theme("Custom", &[crate::color::rgb_to_hex(rgb)]);
                    }
                });
                ui.add_space(10.0);

                let current_name = crate::settings::theme_name();
                egui::ComboBox::from_id_salt("trontsnap-premade-theme")
                    .selected_text(current_name.clone())
                    .show_ui(ui, |ui| {
                        for p in crate::color::PREMADE_PALETTES {
                            if ui.selectable_label(current_name == p.name, p.name).clicked() {
                                if let Some((tokens, source)) = crate::theme::premade_tokens(p.name) {
                                    crate::theme::set_theme(ui.ctx(), tokens);
                                    crate::settings::set_theme(p.name, &source);
                                }
                            }
                        }
                    });
                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    if ui.button("Randomize").clicked() {
                        let (tokens, name, source) = crate::theme::randomize();
                        crate::theme::set_theme(ui.ctx(), tokens);
                        crate::settings::set_theme(&name, &source);
                    }
                    if ui.button("Reset to default").clicked() {
                        let tokens = crate::theme::resolve("Cyan", &[]);
                        crate::theme::set_theme(ui.ctx(), tokens);
                        crate::settings::set_theme("Cyan", &[]);
                    }
                });
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "Themes are generated with smart contrast, so text stays readable on any color.",
                    )
                    .small()
                    .color(crate::theme::t().text_muted),
                );

                ui.add_space(14.0);
                let mut gradient = crate::settings::gradient();
                if ui.checkbox(&mut gradient, "Gradient").changed() {
                    crate::settings::set_gradient(gradient);
                    // Panel-fill translucency lives in build_visuals and is keyed
                    // off settings::gradient(), so re-derive visuals from the
                    // current tokens to pick up the flip immediately.
                    crate::theme::set_theme(ui.ctx(), crate::theme::t());
                }
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Discord-style background wash behind the panels.")
                        .small()
                        .color(crate::theme::t().text_muted),
                );

                ui.add_space(22.0);
                settings_section_header(ui, "Capture");
                settings_toggle(
                    ui,
                    "Capture cursor",
                    "Include the mouse pointer in screenshots and recordings.",
                    crate::settings::capture_cursor(),
                    crate::settings::set_capture_cursor,
                );
                ui.add_space(14.0);
                settings_toggle(
                    ui,
                    "Use the PrintScreen key",
                    "Toggles the per-user Windows PrintScreenKeyForSnippingEnabled flag; when \
                     off, use the tray icon, the Capture menu, or Ctrl+PrtSc instead.",
                    crate::printscreen::is_free(),
                    crate::printscreen::set_free,
                );

                ui.add_space(22.0);
                settings_section_header(ui, "Recording");
                settings_toggle(
                    ui,
                    "Record audio",
                    "Recordings include a WASAPI loopback track of what you hear.",
                    crate::settings::record_audio(),
                    crate::settings::set_record_audio,
                );

                ui.add_space(22.0);
                settings_section_header(ui, "Region picker");
                let mut loupe = crate::settings::loupe_size();
                if ui
                    .add(
                        egui::Slider::new(&mut loupe, 96..=528)
                            .step_by(48.0)
                            .text("Zoom loupe size"),
                    )
                    .changed()
                {
                    crate::settings::set_loupe_size(loupe);
                }
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "Size of the magnifier that follows the cursor while selecting a \
                         region. You can also scroll the wheel during a region capture to \
                         change it.",
                    )
                    .small()
                    .color(crate::theme::t().text_muted),
                );

                ui.add_space(22.0);
                settings_section_header(ui, "Hotkeys");
                hotkey_row(ui, "Fullscreen", crate::settings::HotkeyAction::Full);
                ui.add_space(14.0);
                hotkey_row(ui, "Region", crate::settings::HotkeyAction::Region);
                ui.add_space(14.0);
                hotkey_row(ui, "Record", crate::settings::HotkeyAction::Record);
                ui.add_space(10.0);
                if ui.button("Reset hotkeys to defaults").clicked() {
                    crate::settings::reset_hotkeys();
                    crate::keyhook::reload();
                }
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "Changes apply instantly. If a combo does not work, another app may \
                         already own it.",
                    )
                    .small()
                    .color(crate::theme::t().text_muted),
                );

                ui.add_space(22.0);
                settings_section_header(ui, "Startup");
                settings_toggle(
                    ui,
                    "Start at login",
                    "Launch TrontSnap hidden in the tray when you sign in to Windows.",
                    autostart::is_enabled(),
                    autostart::set,
                );
                ui.add_space(8.0);
            });
        });
    });
}

/// Small accent section header, matching the About tab's "Shortcuts" heading style.
fn settings_section_header(ui: &mut egui::Ui, title: &str) {
    ui.label(egui::RichText::new(title).strong().color(crate::theme::t().accent));
    ui.add_space(8.0);
}

fn settings_toggle(ui: &mut egui::Ui, label: &str, desc: &str, current: bool, set: fn(bool)) {
    let mut value = current;
    if ui.checkbox(&mut value, label).changed() {
        set(value);
    }
    ui.add_space(4.0);
    ui.label(egui::RichText::new(desc).small().color(crate::theme::t().text_muted));
}

/// (label, virtual-key) options for the Hotkeys section's key ComboBox. All three
/// binds default to PrintScreen; the rest are picked because Windows/apps rarely
/// claim them globally, unlike letters/numbers.
const HOTKEY_KEYS: &[(&str, u32)] = &[
    ("PrintScreen", 0x2C),
    ("Pause", 0x13),
    ("Scroll Lock", 0x91),
    ("Insert", 0x2D),
    ("Home", 0x24),
    ("End", 0x23),
    ("Page Up", 0x21),
    ("Page Down", 0x22),
    ("F1", 0x70),
    ("F2", 0x71),
    ("F3", 0x72),
    ("F4", 0x73),
    ("F5", 0x74),
    ("F6", 0x75),
    ("F7", 0x76),
    ("F8", 0x77),
    ("F9", 0x78),
    ("F10", 0x79),
    ("F11", 0x7A),
    ("F12", 0x7B),
];

// Raw HOT_KEY_MODIFIERS bits (see windows::Win32::UI::Input::KeyboardAndMouse),
// duplicated here as plain u32s so this UI code doesn't need the windows crate.
const HK_MOD_ALT: u32 = 0x0001;
const HK_MOD_CONTROL: u32 = 0x0002;
const HK_MOD_SHIFT: u32 = 0x0004;
const HK_MOD_WIN: u32 = 0x0008;

/// One rebindable-hotkey row: modifier checkboxes + a main-key ComboBox, both
/// seeded from the persisted bind every frame. Any change writes the new bind to
/// settings AND tells the hotkey pump thread to re-register (see keyhook::reload);
/// nothing is written back to storage unless the user actually changes something.
fn hotkey_row(ui: &mut egui::Ui, label: &str, action: crate::settings::HotkeyAction) {
    let (mods, vk) = crate::settings::hotkey(action);
    let mut ctrl = mods & HK_MOD_CONTROL != 0;
    let mut shift = mods & HK_MOD_SHIFT != 0;
    let mut alt = mods & HK_MOD_ALT != 0;
    let mut win = mods & HK_MOD_WIN != 0;

    // Fall back to showing "PrintScreen" if the stored vk isn't one of the known
    // combo options (display-only; this never overwrites storage on its own).
    let current_label =
        HOTKEY_KEYS.iter().find(|(_, v)| *v == vk).map(|(name, _)| *name).unwrap_or("PrintScreen");

    let mut changed = false;
    let mut new_vk = vk;

    ui.label(egui::RichText::new(label).strong().color(crate::theme::t().text_primary));
    ui.horizontal(|ui| {
        changed |= ui.checkbox(&mut ctrl, "Ctrl").changed();
        changed |= ui.checkbox(&mut shift, "Shift").changed();
        changed |= ui.checkbox(&mut alt, "Alt").changed();
        changed |= ui.checkbox(&mut win, "Win").changed();

        egui::ComboBox::from_id_salt(("trontsnap-hotkey-key", label))
            .selected_text(current_label)
            .show_ui(ui, |ui| {
                for (name, v) in HOTKEY_KEYS {
                    if ui.selectable_label(*v == vk, *name).clicked() {
                        new_vk = *v;
                        changed = true;
                    }
                }
            });
    });

    if changed {
        let new_mods = (ctrl as u32 * HK_MOD_CONTROL)
            | (shift as u32 * HK_MOD_SHIFT)
            | (alt as u32 * HK_MOD_ALT)
            | (win as u32 * HK_MOD_WIN);
        crate::settings::set_hotkey(action, new_mods, new_vk);
        crate::keyhook::reload();
    }
}

/// One About-tab shortcut line: the key combo in accent, the action in muted, on a
/// single two-tone line. Clean and centered, no keycap chrome.
fn shortcut_row(ui: &mut egui::Ui, key: &str, desc: &str) {
    use egui::text::{LayoutJob, TextFormat};
    let mut job = LayoutJob::default();
    job.append(
        key,
        0.0,
        TextFormat {
            color: crate::theme::t().accent,
            font_id: egui::FontId::proportional(14.0),
            ..Default::default()
        },
    );
    job.append(
        &format!("    {desc}"),
        0.0,
        TextFormat {
            color: crate::theme::t().text_muted,
            font_id: egui::FontId::proportional(14.0),
            ..Default::default()
        },
    );
    ui.label(job);
    ui.add_space(6.0);
}

impl App {
    /// About tab: scrollable + width-clamped so it stays readable at any window
    /// size (the old Boxel-style fixed About window broke under zoom/resize).
    fn about_tab_ui(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.set_max_width(520.0);
                ui.add_space(26.0);
                ui.add(
                    egui::Image::new(&self.icon_tex).fit_to_exact_size(egui::vec2(72.0, 72.0)),
                );
                ui.add_space(10.0);
                ui.heading(
                    egui::RichText::new("TrontSnap").color(crate::theme::t().accent).size(26.0),
                );
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new("Fast screenshots, full history, no cloud.")
                        .size(15.0)
                        .color(crate::theme::t().text_muted),
                );
                ui.add_space(18.0);
                ui.separator();
                ui.add_space(14.0);

                ui.label(egui::RichText::new("Shortcuts").strong().color(crate::theme::t().accent));
                ui.add_space(10.0);
                shortcut_row(ui, "PrtSc", "Grab the full screen");
                shortcut_row(ui, "Ctrl + PrtSc", "Freeze, then click a window or drag a region");
                shortcut_row(ui, "Ctrl + Shift + PrtSc", "Start or stop a screen recording");
                shortcut_row(ui, "Tray icon", "Left-click for an instant region capture");
                ui.add_space(18.0);
                ui.separator();
                ui.add_space(14.0);

                ui.label(egui::RichText::new("Trent Sterling").strong());
                ui.add_space(2.0);
                ui.hyperlink_to("tront.xyz", "https://tront.xyz");
                ui.hyperlink_to(
                    "github.com/TrentSterling/trontsnap",
                    "https://github.com/TrentSterling/trontsnap",
                );
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("Portable single exe. Delete to uninstall.")
                        .small()
                        .color(crate::theme::t().text_muted),
                );
                ui.label(egui::RichText::new("MIT License").small().color(crate::theme::t().text_muted));
                ui.add_space(22.0);

                let mut show_on_launch = crate::settings::show_about_on_launch();
                if ui
                    .checkbox(&mut show_on_launch, "Show this tab when TrontSnap starts")
                    .changed()
                {
                    crate::settings::set_show_about_on_launch(show_on_launch);
                }
                ui.add_space(20.0);
            });
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
