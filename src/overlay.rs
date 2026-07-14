// Freeze-frame region picker.
//
// The picker is now driven IN-PROCESS by the resident app: on Ctrl+PrintScreen the
// app repurposes its own (already-warm) window into a fullscreen borderless overlay
// and runs `RegionPicker::ui` each frame — no subprocess, no fresh GL context, so
// there is no launch cost and no black flash. `run_region()` keeps the same picker
// as a standalone CLI fallback (`trontsnap region`).
//
// DPI-robust by construction: we never do manual scaling math against the OS. We map
// selection points -> source pixels by the ratio (image size / painted screen size),
// so whatever the pixels-per-point is, the crop lands right.
//
// WINDOW ENUMERATION (smart capture): kept fully decoupled from the frozen frame's
// dimensions and from our own window's fullscreen transition. `enumerate_windows_raw`
// does the Win32/DWM syscalls (no img_w/img_h needed); `clamp_windows` does the pure
// arithmetic clamp + filtering once the frame size is known. Callers on the hotkey
// path (app.rs::start_region) run `enumerate_windows_raw` concurrently with the
// ~90ms xcap grab, on a background thread — never on the UI thread, and never
// depending on whether our own window has become fullscreen yet (self-exclusion is
// PID-based, so that was never actually a race).

use eframe::egui;
use image::RgbaImage;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW, IsIconic,
    IsWindowVisible, GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

use crate::capture;

const ACCENT: egui::Color32 = egui::Color32::from_rgb(90, 209, 255);

/// Window classes that are never a "real" capturable app window, just desktop/
/// shell/overlay plumbing that happens to be a full-screen-sized, visible,
/// non-cloaked top-level window. "Progman" (the desktop) is the big one —
/// without this filter it silently becomes the smart-capture target for the
/// entire desktop background, which reads exactly like "the fullscreen was the
/// rect". Matches ShareX's own ignore list.
const IGNORED_CLASSES: &[&str] = &["Progman", "WorkerW", "Button", "CEF-OSC-WIDGET"];

/// A window's on-screen bounds (physical pixels), before clamping to the captured
/// image. A bare RECT so enumeration has zero dependency on the frozen frame size.
pub type RawRect = RECT;

/// Enumerate visible top-level windows and their *visual* bounds (physical
/// pixels), front-to-back, filtering out anything that isn't a real capturable
/// app window. Pure Win32 + DWM calls, no dependency on the frozen frame's size —
/// safe to run concurrently with (or strictly before) the screen grab and any
/// viewport transition of our own window.
///
/// Uses `DWMWA_EXTENDED_FRAME_BOUNDS` in preference to `GetWindowRect`: modern
/// borderless-styled windows report a GetWindowRect that overshoots the real
/// visual edges (invisible resize-border/shadow margin) and can extend past the
/// monitor bounds — which after clamping degenerates into a whole-screen rect.
/// DWMWA_EXTENDED_FRAME_BOUNDS gives the accurate on-screen rect DWM composites
/// (the same preference ShareX makes). Falls back to GetWindowRect on failure.
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
            // NOTE: our own windows are intentionally INCLUDED now, so the gallery
            // is a selectable rect and you can screenshot TrontSnap itself. The
            // smallest-rect-under-cursor selection keeps the full-screen rect from
            // taking over.
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
            // Skip titleless windows: message-only/utility, and phantom
            // full-screen ApplicationFrameWindow hosts of real UWP content.
            if GetWindowTextLengthW(hwnd) <= 0 {
                continue;
            }
            // Skip non-activatable tool windows (overlays/utilities). Requires
            // BOTH flags (matches ShareX) so genuine floating palettes survive.
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

/// Build the smart-capture rect map: every enumerated window clamped to the frame
/// (whole-monitor-sized ones INCLUDED now), plus one synthetic full-screen rect
/// appended last as the always-available fallback. Selection (`rect_at`) picks the
/// SMALLEST rect under the cursor, so the full-screen rect only wins on empty
/// desktop and never takes over.
pub fn build_rect_map(raw: &[RawRect], img_w: i32, img_h: i32) -> Vec<egui::Rect> {
    let mut out = Vec::with_capacity(raw.len() + 1);
    for r in raw {
        let l = r.left.max(0);
        let t = r.top.max(0);
        let rr = r.right.min(img_w);
        let bb = r.bottom.min(img_h);
        if rr - l < 8 || bb - t < 8 {
            continue; // degenerate / off-screen
        }
        out.push(egui::Rect::from_min_max(
            egui::pos2(l as f32, t as f32),
            egui::pos2(rr as f32, bb as f32),
        ));
    }
    // Full-screen fallback: always present, always last-resort (largest area).
    out.push(egui::Rect::from_min_max(
        egui::pos2(0.0, 0.0),
        egui::pos2(img_w as f32, img_h as f32),
    ));
    out
}

/// Convenience for the standalone CLI path (`run_region`): enumerate + build map.
fn enumerate_windows(img_w: i32, img_h: i32) -> Vec<egui::Rect> {
    build_rect_map(&enumerate_windows_raw(), img_w, img_h)
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let list = &mut *(lparam.0 as *mut Vec<HWND>);
    list.push(hwnd);
    TRUE
}

/// Result of one frame of the picker.
pub enum RegionOutcome {
    Continue,
    /// A capture was delivered, or the user cancelled (Escape / right-click).
    Done,
}

/// The freeze-frame picker. Holds the frozen monitor image + pre-enumerated window
/// rects, drawing/handling one frame per `ui()` call.
pub struct RegionPicker {
    img: RgbaImage,
    tex: Option<egui::TextureHandle>,
    drag_start: Option<egui::Pos2>,
    drag_now: Option<egui::Pos2>,
    windows: Vec<egui::Rect>,
}

impl RegionPicker {
    /// `windows` are pre-enumerated + clamped rects (see enumerate_windows_raw /
    /// clamp_windows). Enumeration is intentionally NOT done here — this runs on
    /// the UI thread right as we paint the first picker frame, so callers do the
    /// 30+ Win32/DWM syscalls off the UI thread instead (app.rs::start_region).
    pub fn new(img: RgbaImage, windows: Vec<egui::Rect>) -> Self {
        Self { img, tex: None, drag_start: None, drag_now: None, windows }
    }

    fn texture(&mut self, ctx: &egui::Context) -> egui::TextureHandle {
        self.tex
            .get_or_insert_with(|| {
                let ci = egui::ColorImage::from_rgba_unmultiplied(
                    [self.img.width() as usize, self.img.height() as usize],
                    self.img.as_raw(),
                );
                ctx.load_texture("frozen", ci, egui::TextureOptions::NEAREST)
            })
            .clone()
    }

    /// The rect to smart-capture at the cursor: the SMALLEST rect containing it
    /// (tie-broken by nearest center), so a small window wins over a big one and
    /// the full-screen fallback only wins on empty desktop.
    fn rect_at(&self, cursor: egui::Pos2, sx: f32, sy: f32) -> Option<egui::Rect> {
        let px = egui::pos2(cursor.x * sx, cursor.y * sy);
        self.windows
            .iter()
            .filter(|r| r.contains(px))
            .min_by(|a, b| {
                let area = |r: &egui::Rect| r.width() * r.height();
                let dist = |r: &egui::Rect| (r.center() - px).length();
                area(a)
                    .partial_cmp(&area(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| {
                        dist(a).partial_cmp(&dist(b)).unwrap_or(std::cmp::Ordering::Equal)
                    })
            })
            .copied()
    }

    /// Convert a source-pixel rect to painted points.
    fn to_points(r: egui::Rect, sx: f32, sy: f32) -> egui::Rect {
        egui::Rect::from_min_max(
            egui::pos2(r.min.x / sx, r.min.y / sy),
            egui::pos2(r.max.x / sx, r.max.y / sy),
        )
    }

    /// Draw + handle one frame. Returns `Done` once a capture is delivered or cancelled.
    pub fn ui(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) -> RegionOutcome {
        let tex = self.texture(ctx);

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            return RegionOutcome::Done;
        }

        let screen = ui.max_rect();
        let full_uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        ui.painter().image(tex.id(), screen, full_uv, egui::Color32::WHITE);

        let sx = self.img.width() as f32 / screen.width();
        let sy = self.img.height() as f32 / screen.height();

        let resp = ui.interact(screen, ui.id().with("canvas"), egui::Sense::click_and_drag());

        if resp.secondary_clicked() {
            return RegionOutcome::Done;
        }

        if resp.drag_started() {
            self.drag_start = resp.interact_pointer_pos();
        }
        if resp.dragged() {
            self.drag_now = resp.interact_pointer_pos();
        }

        let cursor = ctx.input(|i| i.pointer.latest_pos());

        // Smart capture: single click grabs the smallest rect under the cursor.
        if resp.clicked() {
            if let Some(win_px) = cursor.and_then(|c| self.rect_at(c, sx, sy)) {
                deliver_rect_px(&self.img, win_px);
                return RegionOutcome::Done;
            }
        }

        let painter = ui.painter();
        match (self.drag_start, self.drag_now) {
            (Some(a), Some(b)) => {
                let sel = egui::Rect::from_two_pos(a, b);
                dim_outside(painter, screen, sel);
                painter.rect_stroke(sel, 0.0, egui::Stroke::new(1.5, ACCENT));
                draw_dimensions(painter, screen, sel, &self.img);
            }
            _ => {
                // Hover mode: draw the whole rect MAP faintly, then the smallest
                // rect under the cursor bright + dim-outside.
                let sel = cursor.and_then(|c| self.rect_at(c, sx, sy));
                if let Some(sel_px) = sel {
                    dim_outside(painter, screen, Self::to_points(sel_px, sx, sy));
                } else {
                    painter.rect_filled(screen, 0.0, egui::Color32::from_black_alpha(70));
                }
                // Faint outline of every candidate rect (the rect map).
                let faint = egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(90, 209, 255, 80));
                for r in &self.windows {
                    painter.rect_stroke(Self::to_points(*r, sx, sy), 0.0, faint);
                }
                // Bright active selection on top.
                if let Some(sel_px) = sel {
                    let sel_pt = Self::to_points(sel_px, sx, sy);
                    painter.rect_stroke(sel_pt, 0.0, egui::Stroke::new(2.0, ACCENT));
                    draw_dimensions(painter, screen, sel_pt, &self.img);
                }
            }
        }

        if let Some(c) = cursor {
            draw_loupe(painter, &tex, screen, c);
        }

        let mut outcome = RegionOutcome::Continue;
        if resp.drag_stopped() {
            if let (Some(a), Some(b)) = (self.drag_start, self.drag_now) {
                let sel = egui::Rect::from_two_pos(a, b);
                if sel.width() > 3.0 && sel.height() > 3.0 {
                    deliver_selection(&self.img, screen, sel);
                }
            }
            outcome = RegionOutcome::Done;
        }

        ctx.request_repaint();
        outcome
    }
}

/// Standalone CLI fallback: `trontsnap region`.
pub fn run_region() -> anyhow::Result<()> {
    let img = capture::grab_primary()?;
    let windows = enumerate_windows(img.width() as i32, img.height() as i32);
    let picker = RegionPicker::new(img, windows);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_fullscreen(true)
            .with_decorations(false)
            .with_always_on_top()
            .with_taskbar(false),
        ..Default::default()
    };

    eframe::run_native(
        "TrontSnap",
        options,
        Box::new(move |_cc| Ok(Box::new(StandaloneRegionApp { picker }))),
    )
    .map_err(|e| anyhow::anyhow!("overlay window failed: {e}"))
}

struct StandaloneRegionApp {
    picker: RegionPicker,
}

impl eframe::App for StandaloneRegionApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 1.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| {
            if let RegionOutcome::Done = self.picker.ui(ctx, ui) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }
}

/// Crop the source image to a selection (in painted points) and deliver it.
fn deliver_selection(img: &RgbaImage, screen: egui::Rect, sel: egui::Rect) {
    let sx = img.width() as f32 / screen.width();
    let sy = img.height() as f32 / screen.height();
    let px = egui::Rect::from_min_max(
        egui::pos2(sel.min.x * sx, sel.min.y * sy),
        egui::pos2(sel.max.x * sx, sel.max.y * sy),
    );
    deliver_rect_px(img, px);
}

/// Crop to a rect given in source pixels, then hand delivery (PNG encode +
/// clipboard + disk + shutter) to a BACKGROUND thread — the crop is a cheap
/// memcpy so ui() returns Done this frame and app.rs restores the window
/// immediately instead of waiting on encode/clipboard/disk.
fn deliver_rect_px(img: &RgbaImage, rect: egui::Rect) {
    let iw = img.width() as f32;
    let ih = img.height() as f32;
    let x = rect.min.x.round().clamp(0.0, iw) as u32;
    let y = rect.min.y.round().clamp(0.0, ih) as u32;
    let w = (rect.width().round() as u32).min(img.width().saturating_sub(x));
    let h = (rect.height().round() as u32).min(img.height().saturating_sub(y));
    if w == 0 || h == 0 {
        return;
    }

    let cropped = image::imageops::crop_imm(img, x, y, w, h).to_image();
    std::thread::spawn(move || match capture::deliver(&cropped) {
        Ok(path) => println!("captured {w}x{h} -> clipboard + {}", path.display()),
        Err(e) => eprintln!("trontsnap: deliver failed: {e:#}"),
    });
}

/// Darken everything outside the selection with four border rects.
fn dim_outside(painter: &egui::Painter, screen: egui::Rect, sel: egui::Rect) {
    let shade = egui::Color32::from_black_alpha(110);
    let sel = sel.intersect(screen);
    painter.rect_filled(
        egui::Rect::from_min_max(screen.min, egui::pos2(screen.max.x, sel.min.y)),
        0.0,
        shade,
    );
    painter.rect_filled(
        egui::Rect::from_min_max(egui::pos2(screen.min.x, sel.max.y), screen.max),
        0.0,
        shade,
    );
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(screen.min.x, sel.min.y),
            egui::pos2(sel.min.x, sel.max.y),
        ),
        0.0,
        shade,
    );
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(sel.max.x, sel.min.y),
            egui::pos2(screen.max.x, sel.max.y),
        ),
        0.0,
        shade,
    );
}

/// Live pixel dimensions of the selection, drawn just outside its top-left.
fn draw_dimensions(painter: &egui::Painter, screen: egui::Rect, sel: egui::Rect, img: &RgbaImage) {
    let sx = img.width() as f32 / screen.width();
    let sy = img.height() as f32 / screen.height();
    let pw = (sel.width() * sx).round().max(0.0) as u32;
    let ph = (sel.height() * sy).round().max(0.0) as u32;
    let text = format!("{pw} x {ph}");

    let mut anchor = egui::pos2(sel.min.x, sel.min.y - 22.0);
    if anchor.y < screen.min.y + 2.0 {
        anchor.y = sel.min.y + 4.0;
    }

    let galley = painter.layout_no_wrap(text, egui::FontId::proportional(13.0), egui::Color32::WHITE);
    let bg = egui::Rect::from_min_size(anchor, galley.size() + egui::vec2(10.0, 6.0));
    painter.rect_filled(bg, 3.0, egui::Color32::from_rgba_unmultiplied(10, 12, 16, 220));
    painter.galley(anchor + egui::vec2(5.0, 3.0), galley, egui::Color32::WHITE);
}

/// Pixel-crisp magnifier at the cursor. Uses the frozen texture with a zoomed UV rect.
fn draw_loupe(painter: &egui::Painter, tex: &egui::TextureHandle, screen: egui::Rect, cursor: egui::Pos2) {
    const SIZE: f32 = 132.0;
    const ZOOM: f32 = 8.0;
    const MARGIN: f32 = 26.0;

    let mut origin = cursor + egui::vec2(MARGIN, MARGIN);
    if origin.x + SIZE > screen.max.x {
        origin.x = cursor.x - MARGIN - SIZE;
    }
    if origin.y + SIZE > screen.max.y {
        origin.y = cursor.y - MARGIN - SIZE;
    }
    let box_rect = egui::Rect::from_min_size(origin, egui::vec2(SIZE, SIZE));

    let cu = cursor.x / screen.width();
    let cv = cursor.y / screen.height();
    let half_u = (SIZE / ZOOM / 2.0) / screen.width();
    let half_v = (SIZE / ZOOM / 2.0) / screen.height();
    let uv = egui::Rect::from_min_max(
        egui::pos2(cu - half_u, cv - half_v),
        egui::pos2(cu + half_u, cv + half_v),
    );

    painter.rect_filled(box_rect, 4.0, egui::Color32::BLACK);
    painter.image(tex.id(), box_rect, uv, egui::Color32::WHITE);
    painter.rect_stroke(box_rect, 4.0, egui::Stroke::new(1.0, ACCENT));

    let ctr = box_rect.center();
    let cross = egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(90, 209, 255, 160));
    painter.line_segment(
        [egui::pos2(box_rect.min.x, ctr.y), egui::pos2(box_rect.max.x, ctr.y)],
        cross,
    );
    painter.line_segment(
        [egui::pos2(ctr.x, box_rect.min.y), egui::pos2(ctr.x, box_rect.max.y)],
        cross,
    );
}
