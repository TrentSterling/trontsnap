//! ShareX-style corner toast: a small themed always-on-top window in the
//! bottom-right showing the capture thumbnail + "Copied to clipboard", auto-
//! dismissing after ~2.6s (click it to open the file). Spawned as its own tiny
//! process (`trontsnap toast <path>`) per capture so it never touches the main
//! app's window/loop.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;
use image::RgbaImage;

const W: f32 = 330.0;
const H: f32 = 90.0;

pub fn show(path: PathBuf) -> anyhow::Result<()> {
    // Small thumbnail of the capture (aspect-preserving fit in 76px).
    let thumb = image::open(&path).ok().map(|i| i.thumbnail(76, 76).to_rgba8());

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([W, H])
            .with_decorations(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_resizable(false),
        ..Default::default()
    };

    eframe::run_native(
        "TrontSnap Toast",
        options,
        Box::new(move |cc| {
            crate::theme::apply(&cc.egui_ctx);
            Ok(Box::new(Toast::new(path, thumb)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("toast failed: {e}"))
}

struct Toast {
    path: PathBuf,
    thumb: Option<RgbaImage>,
    tex: Option<egui::TextureHandle>,
    born: Option<Instant>,
    positioned: bool,
}

impl Toast {
    fn new(path: PathBuf, thumb: Option<RgbaImage>) -> Self {
        Self { path, thumb, tex: None, born: None, positioned: false }
    }
}

impl eframe::App for Toast {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Dock bottom-right of the work area once, in physical pixels via Win32
        // (deterministic — avoids eframe OuterPosition point/pixel ambiguity).
        if !self.positioned {
            position_bottom_right(frame);
            self.positioned = true;
        }

        // Auto-dismiss.
        let born = *self.born.get_or_insert_with(Instant::now);
        if born.elapsed() > Duration::from_millis(2600) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // Upload the thumbnail once.
        if self.tex.is_none() {
            if let Some(img) = &self.thumb {
                let ci = egui::ColorImage::from_rgba_unmultiplied(
                    [img.width() as usize, img.height() as usize],
                    img.as_raw(),
                );
                self.tex = Some(ctx.load_texture("toast-thumb", ci, egui::TextureOptions::LINEAR));
            }
        }

        let name = self
            .path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();

        let frame = egui::Frame::none()
            .fill(crate::theme::T.panel_bg)
            .stroke(egui::Stroke::new(1.0, crate::theme::T.accent))
            .rounding(8.0)
            .inner_margin(10.0);

        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
            let resp = ui.interact(ui.max_rect(), ui.id().with("toast"), egui::Sense::click());
            ui.horizontal_centered(|ui| {
                if let Some(tex) = &self.tex {
                    let sz = tex.size_vec2();
                    let scale = (64.0 / sz.x).min(64.0 / sz.y);
                    let (rect, _) = ui.allocate_exact_size(sz * scale, egui::Sense::hover());
                    ui.painter().image(
                        tex.id(),
                        rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                    ui.painter().rect_stroke(rect, 3.0, egui::Stroke::new(1.0, crate::theme::T.stroke));
                }
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("Copied to clipboard")
                            .color(crate::theme::T.accent)
                            .strong(),
                    );
                    ui.label(egui::RichText::new(name).color(crate::theme::T.text_muted).small());
                    ui.label(
                        egui::RichText::new("click to open")
                            .color(crate::theme::T.text_muted)
                            .small(),
                    );
                });
            });
            if resp.clicked() {
                let _ = opener::open(&self.path);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });

        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

/// Move the toast to the bottom-right of the primary work area (excludes the
/// taskbar), in physical pixels.
fn position_bottom_right(frame: &eframe::Frame) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowRect, SetWindowPos, SystemParametersInfoW, HWND_TOPMOST, SPI_GETWORKAREA,
        SWP_NOACTIVATE, SWP_NOSIZE, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    };

    let hwnd = match frame.window_handle().ok().map(|h| h.as_raw()) {
        Some(RawWindowHandle::Win32(h)) => HWND(h.hwnd.get()),
        _ => return,
    };
    unsafe {
        let mut wa = RECT::default();
        if SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut wa as *mut _ as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .is_err()
        {
            return;
        }
        let mut wr = RECT::default();
        if GetWindowRect(hwnd, &mut wr).is_err() {
            return;
        }
        let w = wr.right - wr.left;
        let h = wr.bottom - wr.top;
        let x = wa.right - w - 24;
        let y = wa.bottom - h - 24;
        let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
    }
}
