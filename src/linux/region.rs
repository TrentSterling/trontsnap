// Freeze-frame region picker, Linux side: an egui/eframe fullscreen overlay
// born with the frozen screenshot painted in — same UX as the Win32/GDI picker
// (drag a region; click a window for its rect where enumeration works; Esc or
// right-click cancels; release delivers).
//
// PROCESS MODEL, and why: eframe/winit allows exactly ONE event loop per
// process, and it must run on the main thread. The one-shot CLI modes
// (`trontsnap region` / `ocr`) satisfy both, so they run the picker inline.
// The persistent app does NOT (its event loop is the gallery's, and hotkey
// dispatch happens on worker threads) — so when these entry points are called
// from anywhere that isn't the matching one-shot mode, they respawn the binary
// as `trontsnap region`/`ocr` and return. Toasts already use the same
// own-process trick on Windows for the same reason.

use std::sync::{Arc, Mutex};

use eframe::egui;
use image::RgbaImage;

use crate::capture;
use crate::overlay::{enumerate_windows_raw, RawRect};

/// Below this pointer travel (physical px) a press+release is a click
/// (smart-rect pick), not a drag.
const CLICK_SLOP: f32 = 5.0;

/// One-shot CLI mode this process was started in, if any.
fn cli_mode() -> Option<String> {
    std::env::args().nth(1)
}

fn respawn_as(mode: &str) {
    let Ok(exe) = std::env::current_exe() else { return };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg(mode);
    if !crate::shellexec::spawn_detached(cmd) {
        eprintln!("trontsnap: could not spawn the {mode} picker process");
    }
}

/// Region capture entry point: picker, crop, deliver (clipboard + disk + shutter).
pub fn capture_region() {
    if cli_mode().as_deref() != Some("region") {
        respawn_as("region");
        return;
    }
    let Some(cropped) = picked_crop() else { return };
    let (rw, rh) = (cropped.width(), cropped.height());
    match capture::deliver(&cropped) {
        Ok(path) => println!("captured {rw}x{rh} -> clipboard + {}", path.display()),
        Err(e) => eprintln!("trontsnap: region deliver failed: {e:#}"),
    }
}

/// Picker for the recorder: the chosen rectangle in physical pixels, clamped to the
/// frozen frame (same clamp as a still's crop). `None` = Esc / right-click / empty drag.
/// Needs the main thread, so only `trontsnap record region` calls it.
pub fn pick_rect() -> Option<crate::recorder::Region> {
    let (img, sel) = run_picker()?;
    let x = sel.left.clamp(0, img.width() as i32);
    let y = sel.top.clamp(0, img.height() as i32);
    let w = (sel.right.clamp(0, img.width() as i32) - x).max(0);
    let h = (sel.bottom.clamp(0, img.height() as i32) - y).max(0);
    // Below the recorder's minimum a "region" is a mis-click, not a recording.
    (w >= 16 && h >= 16).then_some(crate::recorder::Region { x, y, w: w as u32, h: h as u32 })
}

/// Same picker, OCR delivery: the crop's TEXT lands on the clipboard. Nothing
/// is saved to disk; this is a read, not a capture.
pub fn capture_region_ocr() {
    if cli_mode().as_deref() != Some("ocr") {
        respawn_as("ocr");
        return;
    }
    // HARD-WON: this runs from a hotkey, where stderr goes nowhere. With tesseract
    // missing (not preinstalled on Ubuntu) the user used to get a picker, a drag,
    // and then silence plus the OLD clipboard. Say so BEFORE the picker.
    if !crate::ocr::available() {
        let msg = "OCR needs tesseract: sudo apt install tesseract-ocr";
        crate::clipboard::log_line(&format!("ocr: {msg}"));
        eprintln!("trontsnap: {msg}");
        notify("OCR unavailable", msg);
        return;
    }
    let Some(cropped) = picked_crop() else { return };
    let (rw, rh) = (cropped.width(), cropped.height());
    match crate::ocr::recognize(&cropped) {
        Ok(text) if text.trim().is_empty() => {
            crate::clipboard::log_line(&format!("ocr: no text found in {rw}x{rh} region"));
            notify("No text found", &format!("OCR saw nothing in the {rw}x{rh} region"));
        }
        Ok(text) => {
            let lines = text.lines().count();
            let _ = crate::clipboard::set_text(&text);
            crate::sound::play_shutter();
            println!("ocr {rw}x{rh} -> {lines} lines to clipboard");
        }
        Err(e) => {
            eprintln!("trontsnap: ocr failed: {e:#}");
            crate::clipboard::log_line(&format!("ocr failed: {e:#}"));
            notify("OCR failed", &format!("{e:#}"));
        }
    }
}

/// Desktop notification for the one-shot picker modes (nothing else is visible
/// when they fail). Transient so it doesn't pile up in the tray.
fn notify(summary: &str, body: &str) {
    let mut cmd = std::process::Command::new("notify-send");
    cmd.args(["-a", "TrontSnap", "-t", "5000", "-h", "int:transient:1", summary, body])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let _ = crate::shellexec::spawn_detached(cmd);
}

/// Run the picker and return the cropped selection, clamped to the frame.
fn picked_crop() -> Option<RgbaImage> {
    let (img, sel) = run_picker()?;
    let w = img.width() as i32;
    let x = sel.left.clamp(0, w) as u32;
    let y = sel.top.clamp(0, img.height() as i32) as u32;
    let rw = ((sel.right - sel.left).max(0) as u32).min(img.width().saturating_sub(x));
    let rh = ((sel.bottom - sel.top).max(0) as u32).min(img.height().saturating_sub(y));
    if rw == 0 || rh == 0 {
        return None;
    }
    Some(image::imageops::crop_imm(&img, x, y, rw, rh).to_image())
}

/// Grab the frozen frame, enumerate smart targets, run the modal overlay.
/// Returns the frame and the raw (unclamped) selection in physical pixels.
fn run_picker() -> Option<(RgbaImage, RawRect)> {
    let frame = match capture::grab_for_shot() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("trontsnap: screen grab failed: {e:#}");
            return None;
        }
    };
    let windows = enumerate_windows_raw();
    let frame = Arc::new(frame);
    let result: Arc<Mutex<Option<RawRect>>> = Arc::new(Mutex::new(None));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("TrontSnap Region")
            .with_decorations(false)
            .with_fullscreen(true)
            .with_always_on_top(),
        ..Default::default()
    };
    let app_frame = frame.clone();
    let app_result = result.clone();
    let run = eframe::run_native(
        "TrontSnap Region",
        options,
        Box::new(move |_cc| Ok(Box::new(Picker::new(app_frame, windows, app_result)))),
    );
    if let Err(e) = run {
        eprintln!("trontsnap: picker failed: {e}");
        return None;
    }
    let sel = result.lock().ok()?.take()?;
    let frame = Arc::try_unwrap(frame).unwrap_or_else(|a| (*a).clone());
    Some((frame, sel))
}

struct Picker {
    frame: Arc<RgbaImage>,
    windows: Vec<RawRect>,
    result: Arc<Mutex<Option<RawRect>>>,
    tex: Option<egui::TextureHandle>,
    /// Press origin in PHYSICAL pixels; Some while a gesture is in progress.
    press: Option<egui::Pos2>,
    /// Current pointer in physical pixels.
    cursor: egui::Pos2,
    done: bool,
}

impl Picker {
    fn new(
        frame: Arc<RgbaImage>,
        windows: Vec<RawRect>,
        result: Arc<Mutex<Option<RawRect>>>,
    ) -> Self {
        Self { frame, windows, result, tex: None, press: None, cursor: egui::Pos2::ZERO, done: false }
    }

    /// Topmost enumerated window containing the point, else the full frame.
    fn smart_rect(&self, p: egui::Pos2) -> RawRect {
        for w in &self.windows {
            if p.x >= w.left as f32 && p.x < w.right as f32 && p.y >= w.top as f32 && p.y < w.bottom as f32
            {
                return *w;
            }
        }
        RawRect { left: 0, top: 0, right: self.frame.width() as i32, bottom: self.frame.height() as i32 }
    }

    fn finish(&mut self, ctx: &egui::Context, sel: Option<RawRect>) {
        if self.done {
            return;
        }
        self.done = true;
        if let Ok(mut slot) = self.result.lock() {
            *slot = sel;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

impl eframe::App for Picker {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let ppp = ctx.pixels_per_point();

        if ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.pointer.secondary_clicked()) {
            self.finish(ctx, None);
            return;
        }

        // Upload the frozen frame once.
        if self.tex.is_none() {
            let ci = egui::ColorImage::from_rgba_unmultiplied(
                [self.frame.width() as usize, self.frame.height() as usize],
                self.frame.as_raw(),
            );
            self.tex = Some(ctx.load_texture("frozen-frame", ci, egui::TextureOptions::LINEAR));
        }

        // Pointer state in PHYSICAL pixels (the frame's coordinate space).
        let (pressed, released, hover) = ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_released(),
                i.pointer.hover_pos().or_else(|| i.pointer.interact_pos()),
            )
        });
        if let Some(p) = hover {
            self.cursor = egui::pos2(p.x * ppp, p.y * ppp);
        }
        if pressed {
            self.press = Some(self.cursor);
        }
        if released {
            if let Some(start) = self.press.take() {
                let travel = (self.cursor - start).abs();
                let sel = if travel.x < CLICK_SLOP && travel.y < CLICK_SLOP {
                    self.smart_rect(self.cursor)
                } else {
                    RawRect {
                        left: start.x.min(self.cursor.x) as i32,
                        top: start.y.min(self.cursor.y) as i32,
                        right: start.x.max(self.cursor.x) as i32,
                        bottom: start.y.max(self.cursor.y) as i32,
                    }
                };
                self.finish(ctx, Some(sel));
                return;
            }
        }

        let accent = crate::theme::accent_over_media();
        egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| {
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::Crosshair);
            let screen = ui.max_rect();
            let painter = ui.painter();

            if let Some(tex) = &self.tex {
                painter.image(
                    tex.id(),
                    screen,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }

            // Live selection (logical coords for painting), scrim everywhere else.
            let sel_logical = self.press.map(|start| {
                egui::Rect::from_two_pos(
                    egui::pos2(start.x / ppp, start.y / ppp),
                    egui::pos2(self.cursor.x / ppp, self.cursor.y / ppp),
                )
            });
            let scrim = egui::Color32::from_black_alpha(110);
            match sel_logical {
                Some(sel) => {
                    // Four bands around the selection so the chosen pixels stay bright.
                    let s = screen;
                    painter.rect_filled(
                        egui::Rect::from_min_max(s.min, egui::pos2(s.right(), sel.top())),
                        0.0,
                        scrim,
                    );
                    painter.rect_filled(
                        egui::Rect::from_min_max(egui::pos2(s.left(), sel.bottom()), s.max),
                        0.0,
                        scrim,
                    );
                    painter.rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(s.left(), sel.top()),
                            egui::pos2(sel.left(), sel.bottom()),
                        ),
                        0.0,
                        scrim,
                    );
                    painter.rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(sel.right(), sel.top()),
                            egui::pos2(s.right(), sel.bottom()),
                        ),
                        0.0,
                        scrim,
                    );
                    painter.rect_stroke(sel, 0.0, egui::Stroke::new(1.5, accent));
                    // Dimensions readout in PHYSICAL px (what actually gets cropped).
                    let dims = format!(
                        "{} x {}",
                        (sel.width() * ppp).round() as i32,
                        (sel.height() * ppp).round() as i32
                    );
                    painter.text(
                        sel.left_top() + egui::vec2(4.0, -18.0),
                        egui::Align2::LEFT_TOP,
                        dims,
                        egui::FontId::proportional(13.0),
                        accent,
                    );
                }
                None => {
                    painter.rect_filled(screen, 0.0, scrim);
                    painter.text(
                        egui::pos2(screen.center().x, screen.top() + 42.0),
                        egui::Align2::CENTER_TOP,
                        "drag to select a region  ·  click for a window  ·  Esc to cancel",
                        egui::FontId::proportional(15.0),
                        egui::Color32::from_white_alpha(220),
                    );
                }
            }
        });

        // Repaint continuously while interacting so the rubber band stays live.
        ctx.request_repaint();
    }
}
