//! Annotation editor: `trontsnap edit <path>`, spawned as its own process for
//! the same reason the toast is one — a tray-hidden main window never runs
//! update(), so it cannot open an editor at capture time. An ordinary egui
//! window (NOT GDI like the region picker: an editor needs a text caret, undo
//! and real widgets, and has no "must appear in one frame" constraint).
//!
//! Model: the base image stays untouched in memory; annotations live in a
//! Vec<Shape> and only bake into pixels on save. Undo/redo is an index into
//! that list. The two REDACTION kinds (solid fill, pixelate) are the
//! exception: they are composited into the preview texture immediately, so
//! what you see is exactly what the file will contain — a redaction preview
//! that lies is worse than none. Saving OVERWRITES the original file and puts
//! the result on the clipboard (annotate-then-paste is the whole flow).

use std::path::{Path, PathBuf};

use eframe::egui;
use image::RgbaImage;

/// Toolbar width; the canvas gets everything else.
const TOOLS_W: f32 = 190.0;

/// Spawn the editor subprocess (`trontsnap edit <path>`). Same ShellExecuteW
/// route as the toast (see toast::launch for the uiAccess history), and the
/// same best-effort contract: the file already exists, nothing is lost if the
/// editor fails to appear.
pub fn launch(path: &Path) {
    let Ok(exe) = std::env::current_exe() else { return };
    let params = format!("edit \"{}\"", path.display());
    if !crate::shellexec::run(&exe.to_string_lossy(), &params) {
        eprintln!("trontsnap: editor launch refused by the shell");
    }
}

pub fn run(path: PathBuf) -> anyhow::Result<()> {
    let base = image::open(&path)
        .map_err(|e| anyhow::anyhow!("could not open {}: {e}", path.display()))?
        .to_rgba8();

    // Window sized to the image plus chrome, clamped to something sane; the
    // canvas letterboxes whatever doesn't fit.
    let w = (base.width() as f32 + TOOLS_W + 40.0).clamp(860.0, 1560.0);
    let h = (base.height() as f32 + 90.0).clamp(560.0, 960.0);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("TrontSnap Edit")
            .with_inner_size([w, h])
            .with_min_inner_size([760.0, 480.0]),
        ..Default::default()
    };

    eframe::run_native(
        "TrontSnap Edit",
        options,
        Box::new(move |cc| {
            crate::theme::apply(&cc.egui_ctx);
            Ok(Box::new(Editor::new(path, base)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("editor failed: {e}"))
}

#[derive(Clone, Copy, PartialEq)]
enum Tool {
    Arrow,
    Box,
    Redact,
    Pixelate,
    Text,
    Crop,
}

#[derive(Clone, Copy, PartialEq)]
enum ShapeKind {
    Arrow,
    Box,
    /// Solid fill, always black. The DEFAULT redaction: a gaussian blur over
    /// text can be reversed, so soft options must be chosen, never defaulted.
    Redact,
    /// Chunky mosaic; block size derives from the region so it can't be dialed
    /// down to something reversible by accident.
    Pixelate,
    Text,
}

/// One annotation, in IMAGE pixel coordinates (display scale never touches the
/// stored geometry, so a resized window can't move existing shapes).
struct Shape {
    kind: ShapeKind,
    a: egui::Pos2,
    b: egui::Pos2,
    color: egui::Color32,
    width: f32,
    text: String,
    font_px: f32,
}

struct Editor {
    path: PathBuf,
    base: RgbaImage,
    /// base + all redaction shapes, i.e. exactly what save() starts from.
    /// Rebuilt (and re-uploaded) only when the redaction set changes.
    tex: Option<egui::TextureHandle>,
    tex_dirty: bool,
    shapes: Vec<Shape>,
    redo: Vec<Shape>,
    tool: Tool,
    color: egui::Color32,
    stroke_w: f32,
    font_px: f32,
    /// Index of the Text shape the side-panel TextEdit is editing.
    active_text: Option<usize>,
    /// Image-coord anchor of an in-progress drag.
    drag_from: Option<egui::Pos2>,
    /// Pending crop, image coords, applied at save time. Not in the undo
    /// stack; it has its own Clear button.
    crop: Option<(egui::Pos2, egui::Pos2)>,
    dirty: bool,
    confirm_close: bool,
    status: Option<(String, u32)>,
}

impl Editor {
    fn new(path: PathBuf, base: RgbaImage) -> Self {
        Self {
            path,
            base,
            tex: None,
            tex_dirty: true,
            shapes: Vec::new(),
            redo: Vec::new(),
            tool: Tool::Arrow,
            color: egui::Color32::from_rgb(255, 64, 64),
            stroke_w: 4.0,
            font_px: 28.0,
            active_text: None,
            drag_from: None,
            crop: None,
            dirty: false,
            confirm_close: false,
            status: None,
        }
    }

    fn set_status(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), 150));
    }

    fn push_shape(&mut self, s: Shape) {
        if matches!(s.kind, ShapeKind::Redact | ShapeKind::Pixelate) {
            self.tex_dirty = true;
        }
        self.shapes.push(s);
        self.redo.clear();
        self.dirty = true;
    }

    fn undo(&mut self) {
        if let Some(s) = self.shapes.pop() {
            if matches!(s.kind, ShapeKind::Redact | ShapeKind::Pixelate) {
                self.tex_dirty = true;
            }
            self.redo.push(s);
            self.active_text = None;
            self.dirty = true;
        }
    }

    fn redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            if matches!(s.kind, ShapeKind::Redact | ShapeKind::Pixelate) {
                self.tex_dirty = true;
            }
            self.shapes.push(s);
            self.dirty = true;
        }
    }

    /// base + redactions: the truth the canvas shows and save() builds on.
    fn composite(&self) -> RgbaImage {
        let mut img = self.base.clone();
        for s in &self.shapes {
            let r = norm_rect_px(s.a, s.b, &img);
            match s.kind {
                ShapeKind::Redact => fill_rect_px(&mut img, r, [0, 0, 0, 255]),
                ShapeKind::Pixelate => pixelate_px(&mut img, r),
                _ => {}
            }
        }
        img
    }

    /// Bake everything and overwrite the original, then put the result on the
    /// clipboard (the usual flow is annotate, then paste it somewhere).
    fn save(&mut self) {
        let mut img = self.composite();
        for s in &self.shapes {
            match s.kind {
                ShapeKind::Arrow => {
                    for (a, b) in arrow_segments(s.a, s.b) {
                        thick_line_px(&mut img, a, b, s.width, s.color);
                    }
                }
                ShapeKind::Box => box_outline_px(&mut img, s.a, s.b, s.width, s.color),
                ShapeKind::Text => draw_text_px(&mut img, s.a, &s.text, s.font_px, s.color),
                ShapeKind::Redact | ShapeKind::Pixelate => {}
            }
        }
        if let Some((a, b)) = self.crop {
            let r = norm_rect_px(a, b, &img);
            if r.2 >= 2 && r.3 >= 2 {
                img = image::imageops::crop_imm(&img, r.0, r.1, r.2, r.3).to_image();
            }
        }

        let png = match encode_png(&img) {
            Ok(p) => p,
            Err(e) => {
                self.set_status(format!("Save failed: {e}"));
                return;
            }
        };
        if let Err(e) = std::fs::write(&self.path, &png) {
            self.set_status(format!("Save failed: {e}"));
            return;
        }
        // Best-effort: the file is safe on disk; the writer thread retries.
        let _ = crate::clipboard::set_all(&img, &png, &self.path);
        self.dirty = false;
        self.set_status("Saved + copied to clipboard");
    }
}

impl eframe::App for Editor {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // ---- keyboard --------------------------------------------------
        let text_focused = ctx.wants_keyboard_input();
        let (undo_k, redo_k, save_k, esc) = ctx.input(|i| {
            (
                i.modifiers.command && i.key_pressed(egui::Key::Z) && !i.modifiers.shift,
                i.modifiers.command
                    && (i.key_pressed(egui::Key::Y)
                        || (i.modifiers.shift && i.key_pressed(egui::Key::Z))),
                i.modifiers.command && i.key_pressed(egui::Key::S),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if save_k {
            self.save();
        }
        // Ctrl+Z must keep working inside the text box for the SHAPES (egui's
        // TextEdit has no history of its own worth protecting here).
        if undo_k {
            self.undo();
        }
        if redo_k {
            self.redo();
        }
        if esc && !text_focused && !self.confirm_close {
            if self.dirty {
                self.confirm_close = true;
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        // The window X follows the same rule as Esc.
        if ctx.input(|i| i.viewport().close_requested()) && self.dirty && !self.confirm_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_close = true;
        }

        if self.confirm_close {
            let mut decision: Option<u8> = None; // 1 save+close, 2 discard, 3 stay
            egui::Window::new("Unsaved changes")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.label("Save before closing? Saving overwrites the original.");
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save and close").clicked() {
                            decision = Some(1);
                        }
                        if ui.button("Discard").clicked() {
                            decision = Some(2);
                        }
                        if ui.button("Cancel").clicked() {
                            decision = Some(3);
                        }
                    });
                });
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                decision = Some(3);
            }
            match decision {
                Some(1) => {
                    self.save();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                Some(2) => {
                    self.dirty = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                Some(3) => self.confirm_close = false,
                _ => {}
            }
        }

        // ---- composite texture ----------------------------------------
        if self.tex_dirty || self.tex.is_none() {
            let img = self.composite();
            let ci = egui::ColorImage::from_rgba_unmultiplied(
                [img.width() as usize, img.height() as usize],
                img.as_raw(),
            );
            match &mut self.tex {
                Some(t) => t.set(ci, egui::TextureOptions::LINEAR),
                None => {
                    self.tex = Some(ctx.load_texture("edit-canvas", ci, egui::TextureOptions::LINEAR))
                }
            }
            self.tex_dirty = false;
        }

        // ---- toolbar ---------------------------------------------------
        egui::SidePanel::left("edit-tools").exact_width(TOOLS_W).show(ctx, |ui| {
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Tools").strong().color(crate::theme::t().accent));
            ui.add_space(4.0);
            for (tool, label, hint) in [
                (Tool::Arrow, "Arrow", "Drag from tail to tip"),
                (Tool::Box, "Box", "Drag a rectangle outline"),
                (Tool::Redact, "Redact", "Solid black fill. The safe default: blur/pixelation of text can be reversed"),
                (Tool::Pixelate, "Pixelate", "Chunky mosaic for de-emphasis; use Redact for secrets"),
                (Tool::Text, "Text", "Click to place, then type in the box below"),
                (Tool::Crop, "Crop", "Drag the area to keep; applied on save"),
            ] {
                if ui.selectable_label(self.tool == tool, label).on_hover_text(hint).clicked() {
                    self.tool = tool;
                }
            }

            ui.add_space(10.0);
            ui.separator();
            ui.label("Color");
            ui.horizontal_wrapped(|ui| {
                for c in [
                    egui::Color32::from_rgb(255, 64, 64),
                    egui::Color32::from_rgb(255, 200, 40),
                    egui::Color32::from_rgb(80, 220, 120),
                    crate::theme::t().accent,
                    egui::Color32::WHITE,
                    egui::Color32::BLACK,
                ] {
                    let (r, resp) =
                        ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
                    ui.painter().rect_filled(r, 4.0, c);
                    if self.color == c {
                        ui.painter().rect_stroke(
                            r,
                            4.0,
                            egui::Stroke::new(2.0, crate::theme::t().text_primary),
                        );
                    }
                    if resp.clicked() {
                        self.color = c;
                    }
                }
                let mut c = self.color;
                ui.scope(|ui| {
                    ui.spacing_mut().interact_size = egui::vec2(30.0, 22.0);
                    if ui.color_edit_button_srgba(&mut c).changed() {
                        self.color = c;
                    }
                });
            });
            ui.add_space(6.0);
            ui.add(egui::Slider::new(&mut self.stroke_w, 1.0..=12.0).text("Width"));

            if self.tool == Tool::Text || self.active_text.is_some() {
                ui.add_space(10.0);
                ui.separator();
                ui.label("Text");
                let mut font_px = self.font_px;
                if ui.add(egui::Slider::new(&mut font_px, 12.0..=72.0).text("Size")).changed() {
                    self.font_px = font_px;
                    if let Some(i) = self.active_text {
                        if let Some(s) = self.shapes.get_mut(i) {
                            s.font_px = font_px;
                            self.dirty = true;
                        }
                    }
                }
                if let Some(i) = self.active_text {
                    if let Some(s) = self.shapes.get_mut(i) {
                        if ui
                            .add(egui::TextEdit::multiline(&mut s.text).desired_rows(2))
                            .changed()
                        {
                            self.dirty = true;
                        }
                    }
                } else {
                    ui.label(
                        egui::RichText::new("Click the image to place text.")
                            .small()
                            .color(crate::theme::t().text_muted),
                    );
                }
            }

            ui.add_space(10.0);
            ui.separator();
            ui.horizontal(|ui| {
                if ui.add_enabled(!self.shapes.is_empty(), egui::Button::new("Undo")).clicked() {
                    self.undo();
                }
                if ui.add_enabled(!self.redo.is_empty(), egui::Button::new("Redo")).clicked() {
                    self.redo();
                }
            });
            if self.crop.is_some() && ui.button("Clear crop").clicked() {
                self.crop = None;
                self.dirty = true;
            }

            ui.add_space(14.0);
            ui.separator();
            if ui
                .button(egui::RichText::new("Save").strong())
                .on_hover_text("Ctrl+S. Overwrites the original and copies it to the clipboard.")
                .clicked()
            {
                self.save();
            }
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("Saving overwrites the original\nand copies it to the clipboard.")
                    .small()
                    .color(crate::theme::t().text_muted),
            );
        });

        // ---- status bar ------------------------------------------------
        egui::TopBottomPanel::bottom("edit-status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let name = self
                    .path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                ui.label(
                    egui::RichText::new(format!(
                        "{name}  .  {}x{}{}",
                        self.base.width(),
                        self.base.height(),
                        if self.dirty { "  .  edited" } else { "" }
                    ))
                    .small()
                    .color(crate::theme::t().text_muted),
                );
                if let Some((msg, _)) = &self.status {
                    ui.separator();
                    ui.colored_label(crate::theme::t().accent, msg.clone());
                }
            });
        });
        if let Some((_, n)) = &mut self.status {
            if *n == 0 {
                self.status = None;
            } else {
                *n -= 1;
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
        }

        // ---- canvas ----------------------------------------------------
        egui::CentralPanel::default().show(ctx, |ui| {
            let avail = ui.available_rect_before_wrap();
            let (iw, ih) = (self.base.width() as f32, self.base.height() as f32);
            // Fit, never upscale: a zoomed-in screenshot reads as blur, and the
            // typical capture is near screen-sized anyway.
            let scale = ((avail.width() - 8.0) / iw).min((avail.height() - 8.0) / ih).min(1.0);
            let disp = egui::Rect::from_center_size(
                avail.center(),
                egui::vec2(iw * scale, ih * scale),
            );
            let to_screen = |p: egui::Pos2| disp.min + p.to_vec2() * scale;
            let to_img = |p: egui::Pos2| {
                egui::pos2(
                    ((p.x - disp.min.x) / scale).clamp(0.0, iw),
                    ((p.y - disp.min.y) / scale).clamp(0.0, ih),
                )
            };

            let resp = ui.allocate_rect(disp, egui::Sense::click_and_drag());
            let painter = ui.painter_at(avail);

            if let Some(tex) = &self.tex {
                painter.image(
                    tex.id(),
                    disp,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            painter.rect_stroke(disp, 0.0, egui::Stroke::new(1.0, crate::theme::t().stroke));

            // Committed vector shapes (redactions are already in the texture).
            for (i, s) in self.shapes.iter().enumerate() {
                let sw = (s.width * scale).max(1.0);
                match s.kind {
                    ShapeKind::Arrow => {
                        for (a, b) in arrow_segments(s.a, s.b) {
                            painter.line_segment(
                                [to_screen(a), to_screen(b)],
                                egui::Stroke::new(sw, s.color),
                            );
                        }
                    }
                    ShapeKind::Box => {
                        painter.rect_stroke(
                            egui::Rect::from_two_pos(to_screen(s.a), to_screen(s.b)),
                            2.0,
                            egui::Stroke::new(sw, s.color),
                        );
                    }
                    ShapeKind::Text => {
                        painter.text(
                            to_screen(s.a),
                            egui::Align2::LEFT_TOP,
                            &s.text,
                            egui::FontId::proportional(s.font_px * scale),
                            s.color,
                        );
                        if self.active_text == Some(i) {
                            let r = painter.text(
                                to_screen(s.a),
                                egui::Align2::LEFT_TOP,
                                &s.text,
                                egui::FontId::proportional(s.font_px * scale),
                                egui::Color32::TRANSPARENT,
                            );
                            painter.rect_stroke(
                                r.expand(3.0),
                                2.0,
                                egui::Stroke::new(1.0, crate::theme::t().text_muted),
                            );
                        }
                    }
                    ShapeKind::Redact | ShapeKind::Pixelate => {}
                }
            }

            // Crop overlay: darken everything OUTSIDE the kept area.
            if let Some((a, b)) = self.crop {
                let keep = egui::Rect::from_two_pos(to_screen(a), to_screen(b));
                let shade = egui::Color32::from_black_alpha(140);
                let top = egui::Rect::from_min_max(disp.min, egui::pos2(disp.max.x, keep.min.y));
                let bottom =
                    egui::Rect::from_min_max(egui::pos2(disp.min.x, keep.max.y), disp.max);
                let left = egui::Rect::from_min_max(
                    egui::pos2(disp.min.x, keep.min.y),
                    egui::pos2(keep.min.x, keep.max.y),
                );
                let right = egui::Rect::from_min_max(
                    egui::pos2(keep.max.x, keep.min.y),
                    egui::pos2(disp.max.x, keep.max.y),
                );
                for r in [top, bottom, left, right] {
                    if r.width() > 0.0 && r.height() > 0.0 {
                        painter.rect_filled(r, 0.0, shade);
                    }
                }
                painter.rect_stroke(keep, 0.0, egui::Stroke::new(1.5, crate::theme::t().accent));
            }

            // ---- interaction ------------------------------------------
            if resp.drag_started() {
                if let Some(p) = resp.interact_pointer_pos() {
                    self.drag_from = Some(to_img(p));
                }
            }

            // In-progress preview.
            if let (Some(from), Some(cur)) = (
                self.drag_from,
                resp.interact_pointer_pos().map(to_img).filter(|_| resp.dragged()),
            ) {
                let sw = (self.stroke_w * scale).max(1.0);
                match self.tool {
                    Tool::Arrow => {
                        for (a, b) in arrow_segments(from, cur) {
                            painter.line_segment(
                                [to_screen(a), to_screen(b)],
                                egui::Stroke::new(sw, self.color),
                            );
                        }
                    }
                    Tool::Box => {
                        painter.rect_stroke(
                            egui::Rect::from_two_pos(to_screen(from), to_screen(cur)),
                            2.0,
                            egui::Stroke::new(sw, self.color),
                        );
                    }
                    Tool::Redact => {
                        painter.rect_filled(
                            egui::Rect::from_two_pos(to_screen(from), to_screen(cur)),
                            0.0,
                            egui::Color32::from_black_alpha(230),
                        );
                    }
                    Tool::Pixelate => {
                        painter.rect_filled(
                            egui::Rect::from_two_pos(to_screen(from), to_screen(cur)),
                            0.0,
                            egui::Color32::from_black_alpha(120),
                        );
                    }
                    Tool::Crop => {
                        painter.rect_stroke(
                            egui::Rect::from_two_pos(to_screen(from), to_screen(cur)),
                            0.0,
                            egui::Stroke::new(1.5, crate::theme::t().accent),
                        );
                    }
                    Tool::Text => {}
                }
            }

            // Commit on release.
            if resp.drag_stopped() {
                if let (Some(from), Some(p)) = (self.drag_from.take(), resp.interact_pointer_pos())
                {
                    let to = to_img(p);
                    let big_enough = (to.x - from.x).abs() >= 3.0 || (to.y - from.y).abs() >= 3.0;
                    if big_enough {
                        match self.tool {
                            Tool::Arrow | Tool::Box | Tool::Redact | Tool::Pixelate => {
                                let kind = match self.tool {
                                    Tool::Arrow => ShapeKind::Arrow,
                                    Tool::Box => ShapeKind::Box,
                                    Tool::Redact => ShapeKind::Redact,
                                    _ => ShapeKind::Pixelate,
                                };
                                self.push_shape(Shape {
                                    kind,
                                    a: from,
                                    b: to,
                                    color: self.color,
                                    width: self.stroke_w,
                                    text: String::new(),
                                    font_px: self.font_px,
                                });
                            }
                            Tool::Crop => {
                                self.crop = Some((from, to));
                                self.dirty = true;
                            }
                            Tool::Text => {}
                        }
                    }
                }
            }

            // Text placement is a click, not a drag.
            if self.tool == Tool::Text && resp.clicked() {
                if let Some(p) = resp.interact_pointer_pos() {
                    self.push_shape(Shape {
                        kind: ShapeKind::Text,
                        a: to_img(p),
                        b: to_img(p),
                        color: self.color,
                        width: self.stroke_w,
                        text: "Text".into(),
                        font_px: self.font_px,
                    });
                    self.active_text = Some(self.shapes.len() - 1);
                }
            }
        });
    }
}

// ---- pixel-space rasterizers (image coords, used only at bake time / for
// ---- the redaction composite) ----------------------------------------------

/// Clamp two corners to the image and return (x, y, w, h).
fn norm_rect_px(a: egui::Pos2, b: egui::Pos2, img: &RgbaImage) -> (u32, u32, u32, u32) {
    let (iw, ih) = (img.width() as f32, img.height() as f32);
    let x0 = a.x.min(b.x).clamp(0.0, iw);
    let y0 = a.y.min(b.y).clamp(0.0, ih);
    let x1 = a.x.max(b.x).clamp(0.0, iw);
    let y1 = a.y.max(b.y).clamp(0.0, ih);
    (x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32)
}

fn fill_rect_px(img: &mut RgbaImage, r: (u32, u32, u32, u32), color: [u8; 4]) {
    let (x, y, w, h) = r;
    for py in y..(y + h).min(img.height()) {
        for px in x..(x + w).min(img.width()) {
            img.put_pixel(px, py, image::Rgba(color));
        }
    }
}

/// Mosaic: average each block, then flood it. Block size derives from the
/// region (clamped chunky) so a tiny region can't produce a fine, reversible
/// pixelation of a password.
fn pixelate_px(img: &mut RgbaImage, r: (u32, u32, u32, u32)) {
    let (x, y, w, h) = r;
    if w == 0 || h == 0 {
        return;
    }
    let block = (w.min(h) / 10).clamp(10, 64);
    let mut by = y;
    while by < (y + h).min(img.height()) {
        let mut bx = x;
        let bh = block.min((y + h).min(img.height()) - by);
        while bx < (x + w).min(img.width()) {
            let bw = block.min((x + w).min(img.width()) - bx);
            let (mut sr, mut sg, mut sb, mut n) = (0u64, 0u64, 0u64, 0u64);
            for py in by..by + bh {
                for px in bx..bx + bw {
                    let p = img.get_pixel(px, py).0;
                    sr += p[0] as u64;
                    sg += p[1] as u64;
                    sb += p[2] as u64;
                    n += 1;
                }
            }
            if n > 0 {
                let avg = [(sr / n) as u8, (sg / n) as u8, (sb / n) as u8, 255];
                for py in by..by + bh {
                    for px in bx..bx + bw {
                        img.put_pixel(px, py, image::Rgba(avg));
                    }
                }
            }
            bx += bw;
        }
        by += bh;
    }
}

/// Alpha-blend `color` onto one pixel at coverage `cov` (0..=1).
fn blend_px(img: &mut RgbaImage, x: i32, y: i32, color: egui::Color32, cov: f32) {
    if x < 0 || y < 0 || x >= img.width() as i32 || y >= img.height() as i32 {
        return;
    }
    let a = (color.a() as f32 / 255.0) * cov.clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    let p = img.get_pixel(x as u32, y as u32).0;
    let mix = |src: u8, dst: u8| (src as f32 * a + dst as f32 * (1.0 - a)).round() as u8;
    img.put_pixel(
        x as u32,
        y as u32,
        image::Rgba([
            mix(color.r(), p[0]),
            mix(color.g(), p[1]),
            mix(color.b(), p[2]),
            255,
        ]),
    );
}

/// Filled capsule from a to b (a line with thickness + round caps): distance
/// test over the bounding box, with a 1px soft edge so strokes aren't jagged.
fn thick_line_px(img: &mut RgbaImage, a: egui::Pos2, b: egui::Pos2, width: f32, color: egui::Color32) {
    let r = width * 0.5;
    let (minx, maxx) = ((a.x.min(b.x) - r - 1.0) as i32, (a.x.max(b.x) + r + 1.0) as i32);
    let (miny, maxy) = ((a.y.min(b.y) - r - 1.0) as i32, (a.y.max(b.y) + r + 1.0) as i32);
    let ab = egui::vec2(b.x - a.x, b.y - a.y);
    let len2 = ab.length_sq().max(1e-6);
    for y in miny..=maxy {
        for x in minx..=maxx {
            let p = egui::vec2(x as f32 + 0.5 - a.x, y as f32 + 0.5 - a.y);
            let t = (p.dot(ab) / len2).clamp(0.0, 1.0);
            let d = (p - ab * t).length();
            let cov = (r + 0.5 - d).clamp(0.0, 1.0);
            if cov > 0.0 {
                blend_px(img, x, y, color, cov);
            }
        }
    }
}

/// Arrow = shaft + two head strokes. Shared by the live preview and the bake
/// so they can never disagree about where the head points.
fn arrow_segments(a: egui::Pos2, b: egui::Pos2) -> Vec<(egui::Pos2, egui::Pos2)> {
    let dir = egui::vec2(b.x - a.x, b.y - a.y);
    let len = dir.length();
    if len < 1.0 {
        return vec![(a, b)];
    }
    let d = dir / len;
    let head = (len * 0.25).clamp(8.0, 26.0);
    let rot = |v: egui::Vec2, ang: f32| {
        egui::vec2(v.x * ang.cos() - v.y * ang.sin(), v.x * ang.sin() + v.y * ang.cos())
    };
    let h1 = b - rot(d, 0.45) * head;
    let h2 = b - rot(d, -0.45) * head;
    vec![(a, b), (b, h1), (b, h2)]
}

fn box_outline_px(img: &mut RgbaImage, a: egui::Pos2, b: egui::Pos2, width: f32, color: egui::Color32) {
    let tl = egui::pos2(a.x.min(b.x), a.y.min(b.y));
    let br = egui::pos2(a.x.max(b.x), a.y.max(b.y));
    let tr = egui::pos2(br.x, tl.y);
    let bl = egui::pos2(tl.x, br.y);
    for (p, q) in [(tl, tr), (tr, br), (br, bl), (bl, tl)] {
        thick_line_px(img, p, q, width, color);
    }
}

/// Rasterize text with the bundled Rajdhani (the same face egui shows), one
/// glyph at a time via ab_glyph coverage. Supports newlines; no wrapping.
fn draw_text_px(img: &mut RgbaImage, anchor: egui::Pos2, text: &str, px: f32, color: egui::Color32) {
    use ab_glyph::{Font, ScaleFont};
    static FONT: std::sync::OnceLock<ab_glyph::FontRef<'static>> = std::sync::OnceLock::new();
    let font = FONT.get_or_init(|| {
        ab_glyph::FontRef::try_from_slice(include_bytes!("../assets/fonts/Rajdhani-SemiBold.ttf"))
            .expect("bundled Rajdhani parses")
    });
    let scaled = font.as_scaled(ab_glyph::PxScale::from(px));
    let mut y = anchor.y + scaled.ascent();
    for line in text.lines() {
        let mut x = anchor.x;
        let mut prev: Option<ab_glyph::GlyphId> = None;
        for ch in line.chars() {
            let id = scaled.glyph_id(ch);
            if let Some(p) = prev {
                x += scaled.kern(p, id);
            }
            let glyph = id.with_scale_and_position(
                ab_glyph::PxScale::from(px),
                ab_glyph::point(x, y),
            );
            if let Some(outline) = font.outline_glyph(glyph) {
                let bounds = outline.px_bounds();
                outline.draw(|gx, gy, cov| {
                    blend_px(
                        img,
                        bounds.min.x as i32 + gx as i32,
                        bounds.min.y as i32 + gy as i32,
                        color,
                        cov,
                    );
                });
            }
            x += scaled.h_advance(id);
            prev = Some(id);
        }
        y += scaled.height();
    }
}

fn encode_png(img: &RgbaImage) -> anyhow::Result<Vec<u8>> {
    use image::codecs::png::PngEncoder;
    use image::ImageEncoder as _;
    let mut out = Vec::new();
    PngEncoder::new(&mut out).write_image(
        img.as_raw(),
        img.width(),
        img.height(),
        image::ColorType::Rgba8,
    )?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_pixel(w, h, image::Rgba([200, 200, 200, 255]))
    }

    #[test]
    fn redact_is_pure_black_over_the_whole_rect() {
        let mut i = img(64, 64);
        fill_rect_px(&mut i, (8, 8, 16, 16), [0, 0, 0, 255]);
        for y in 8..24 {
            for x in 8..24 {
                assert_eq!(i.get_pixel(x, y).0, [0, 0, 0, 255]);
            }
        }
        assert_eq!(i.get_pixel(7, 7).0, [200, 200, 200, 255]);
    }

    #[test]
    fn pixelate_flattens_blocks() {
        let mut i = img(128, 128);
        // Two-tone region so the average is distinct from both inputs.
        for y in 0..64 {
            for x in 0..64 {
                let v = if x < 32 { 0 } else { 200 };
                i.put_pixel(x, y, image::Rgba([v, v, v, 255]));
            }
        }
        pixelate_px(&mut i, (0, 0, 64, 64));
        // Every pixel inside one block must be identical (that is the point).
        let first = i.get_pixel(1, 1).0;
        assert_eq!(i.get_pixel(5, 5).0, first);
    }

    #[test]
    fn crop_rect_clamps_to_image() {
        let i = img(100, 50);
        let r = norm_rect_px(egui::pos2(-20.0, -20.0), egui::pos2(500.0, 500.0), &i);
        assert_eq!(r, (0, 0, 100, 50));
    }

    #[test]
    fn text_marks_pixels() {
        let mut i = img(300, 80);
        draw_text_px(&mut i, egui::pos2(4.0, 4.0), "Hi", 40.0, egui::Color32::BLACK);
        let touched = i.pixels().filter(|p| p.0 != [200, 200, 200, 255]).count();
        assert!(touched > 50, "expected glyph coverage, got {touched} px");
    }

    #[test]
    fn arrow_head_flanks_the_tip() {
        let segs = arrow_segments(egui::pos2(0.0, 0.0), egui::pos2(100.0, 0.0));
        assert_eq!(segs.len(), 3);
        // Head strokes start at the tip and point backwards on both sides.
        assert!(segs[1].1.x < 100.0 && segs[2].1.x < 100.0);
        assert!(segs[1].1.y != segs[2].1.y);
    }
}
