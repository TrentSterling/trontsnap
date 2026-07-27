//! ShareX-style corner toast: a small themed always-on-top window in the
//! bottom-right showing the capture thumbnail + "Copied to clipboard", auto-
//! dismissing after ~2.6s (click it to open the file). Spawned as its own tiny
//! process (`trontsnap toast <path>`) per capture so it never touches the main
//! app's window/loop.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui;
use image::RgbaImage;

/// Spawn the corner-toast subprocess (`trontsnap toast <path>`).
///
/// Must go through ShellExecuteW, NOT `std::process::Command`: the installed
/// build carries the uiAccess token flag, and a uiAccess process cannot start
/// anything with a bare CreateProcess (ERROR_ELEVATION_REQUIRED, 740). Using
/// Command here is exactly why toasts silently stopped appearing after the
/// v0.14.0 install. See shellexec.rs.
///
/// Best-effort: the capture is already saved and on the clipboard by now, so a
/// failed toast never loses anything.
pub fn launch(path: &Path, clipboard_ok: bool) {
    let Ok(exe) = std::env::current_exe() else { return };
    let mut params = format!("toast \"{}\"", path.display());
    if !clipboard_ok {
        params.push_str(" --clipboard-failed");
    }
    if !crate::shellexec::run(&exe.to_string_lossy(), &params) {
        eprintln!("trontsnap: toast launch refused by the shell");
    }
}

const W: f32 = 330.0;
const H: f32 = 90.0;

pub fn show(path: PathBuf, clipboard_ok: bool) -> anyhow::Result<()> {
    // Small thumbnail of the capture (aspect-preserving fit in 76px). Videos decode
    // their first frame via Media Foundation (file is finalized by the time the toast
    // spawns); failures just mean a text-only toast.
    let thumb = if crate::index::is_video(&path) {
        crate::videothumb::first_frame(&path)
            .ok()
            .map(|i| image::DynamicImage::ImageRgba8(i).thumbnail(76, 76).to_rgba8())
    } else {
        image::open(&path).ok().map(|i| i.thumbnail(76, 76).to_rgba8())
    };

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
            Ok(Box::new(Toast::new(path, thumb, clipboard_ok)))
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
    was_pressed: bool,
    /// Whether the clipboard write actually succeeded, so the toast can say so.
    clipboard_ok: bool,
}

impl Toast {
    fn new(path: PathBuf, thumb: Option<RgbaImage>, clipboard_ok: bool) -> Self {
        Self {
            path,
            thumb,
            tex: None,
            born: None,
            positioned: false,
            was_pressed: false,
            clipboard_ok,
        }
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

        // Click-to-open, detected via Win32 (poll the physical mouse button + cursor
        // position). The toast is a non-activating always-on-top tool window, so egui's
        // own click detection doesn't fire on it. A rising edge of the left button while
        // the cursor is over the toast opens the capture in the default viewer.
        let btn = left_button_down();
        if btn && !self.was_pressed && cursor_over_window(frame) {
            if let Err(e) = opener::open(&self.path) {
                eprintln!("trontsnap: toast open failed: {e:#}");
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        self.was_pressed = btn;

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
            .fill(crate::theme::t().panel_bg)
            .stroke(egui::Stroke::new(1.0, crate::theme::t().accent))
            .rounding(8.0)
            .inner_margin(10.0);

        // Tell the truth about the clipboard. This used to say "Copied to clipboard"
        // unconditionally, so when the write lost a race with a browser holding the
        // clipboard the shot silently never arrived and the toast still claimed it
        // had -- the failure only went to eprintln!, which goes nowhere in a
        // windowed exe.
        let title = if crate::index::is_video(&self.path) {
            "Recording saved"
        } else if self.clipboard_ok {
            "Copied to clipboard"
        } else {
            "Saved (clipboard was busy)"
        };

        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
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
                    ui.painter().rect_stroke(rect, 3.0, egui::Stroke::new(1.0, crate::theme::t().stroke));
                }
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(title).color(crate::theme::t().accent).strong());
                    ui.label(egui::RichText::new(name).color(crate::theme::t().text_muted).small());
                    ui.label(
                        egui::RichText::new("click to open")
                            .color(crate::theme::t().text_muted)
                            .small(),
                    );
                });
            });
        });

        // Poll fairly often so the Win32 click detection above catches quick clicks.
        ctx.request_repaint_after(Duration::from_millis(30));
    }
}

/// Is the physical left mouse button currently down? (Global, focus-independent.)
fn left_button_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    unsafe { (GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000) != 0 }
}

/// Is the mouse cursor currently over the toast window? Win32 rect test in physical px.
fn cursor_over_window(frame: &eframe::Frame) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::{HWND, POINT, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetWindowRect};

    let hwnd = match frame.window_handle().ok().map(|h| h.as_raw()) {
        Some(RawWindowHandle::Win32(h)) => HWND(h.hwnd.get()),
        _ => return false,
    };
    unsafe {
        let mut pt = POINT::default();
        if GetCursorPos(&mut pt).is_err() {
            return false;
        }
        let mut wr = RECT::default();
        if GetWindowRect(hwnd, &mut wr).is_err() {
            return false;
        }
        pt.x >= wr.left && pt.x < wr.right && pt.y >= wr.top && pt.y < wr.bottom
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
