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

const W: f32 = 340.0;
const H: f32 = 188.0;
/// How long the toast stays up once the cursor is NOT on it.
const DWELL: Duration = Duration::from_millis(2600);
/// Pointer travel (physical px) that turns a press into a drag rather than a click.
const DRAG_SLOP: i32 = 6;

pub fn show(path: PathBuf, clipboard_ok: bool) -> anyhow::Result<()> {
    // Small thumbnail of the capture (aspect-preserving fit in 76px). Videos decode
    // their first frame via Media Foundation (file is finalized by the time the toast
    // spawns); failures just mean a text-only toast.
    // Decoded at ~2x the toast so the preview stays crisp on a HiDPI display. The
    // old 76px thumbnail was sized for a tiny icon beside a wall of text; the
    // preview is now the toast, so it needs real pixels.
    const THUMB: u32 = 720;
    let thumb = if crate::index::is_video(&path) {
        crate::videothumb::first_frame(&path)
            .ok()
            .map(|i| image::DynamicImage::ImageRgba8(i).thumbnail(THUMB, THUMB).to_rgba8())
    } else {
        image::open(&path).ok().map(|i| i.thumbnail(THUMB, THUMB).to_rgba8())
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
    /// When the dwell countdown last (re)started. Hovering keeps pushing this
    /// forward, so the toast stays up as long as the cursor is on it and gets a
    /// full fresh countdown the moment the cursor leaves.
    born: Option<Instant>,
    positioned: bool,
    was_pressed: bool,
    /// Cursor position (physical px) when the left button went down on the toast.
    /// Some(..) means a press is in progress and has not yet become a drag.
    press_at: Option<(i32, i32)>,
    /// Set once a press has travelled far enough to count as a drag, so the
    /// release does not then also open the file.
    dragging: bool,
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
            press_at: None,
            dragging: false,
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

        // All pointer handling is Win32 polling, not egui: the toast is a
        // non-activating always-on-top tool window, so egui never receives the
        // input events it would normally use for clicks and drags.
        let over = cursor_over_window(frame);
        let btn = left_button_down();

        // HOVER HOLDS THE TOAST OPEN. Pushing `born` forward every hovered frame
        // means the countdown is effectively paused while the cursor is on it,
        // and restarts from full the moment the cursor leaves. That is the
        // ShareX behaviour, and it is what makes a drag-out physically possible:
        // otherwise the toast can vanish out from under the pointer mid-gesture.
        let born = *self.born.get_or_insert_with(Instant::now);
        if over || self.press_at.is_some() {
            self.born = Some(Instant::now());
        } else if born.elapsed() > DWELL {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // Press starts a gesture that is not yet known to be a click or a drag.
        if btn && !self.was_pressed && over {
            self.press_at = cursor_pos();
            self.dragging = false;
        }

        // Travel far enough while held and it becomes a real OLE file drag, so the
        // capture can be dropped straight into Discord, a terminal or an editor
        // without going near the gallery. start_file_drag is modal and blocks
        // until the drop resolves, which is why the toast closes right after.
        if btn && !self.dragging {
            if let (Some((sx, sy)), Some((cx, cy))) = (self.press_at, cursor_pos()) {
                if (cx - sx).abs() > DRAG_SLOP || (cy - sy).abs() > DRAG_SLOP {
                    self.dragging = true;
                    crate::app::start_file_drag(window_hwnd(frame), &self.path);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

        // Release without travel is a click: open the capture. Moved from the
        // press edge so a drag no longer also opens the file in the viewer.
        if !btn && self.was_pressed {
            let was_press = self.press_at.take().is_some();
            if was_press && !self.dragging && over {
                if let Err(e) = opener::open(&self.path) {
                    eprintln!("trontsnap: toast open failed: {e:#}");
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            self.dragging = false;
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

        // THE PREVIEW IS THE TOAST. It used to be a 64px thumbnail beside three
        // lines of text, which spent most of a 330x90 window on words describing
        // a picture it was showing too small to read. Now the capture fills the
        // whole surface and the text sits on top of it, over a scrim so it stays
        // legible against any screenshot.
        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
            let rect = ui.available_rect_before_wrap();
            let painter = ui.painter();

            if let Some(tex) = &self.tex {
                // COVER, not contain: scale to fill and centre-crop via UV, so
                // there are no letterbox bars whatever the capture's aspect is.
                let sz = tex.size_vec2();
                let img_ar = sz.x / sz.y;
                let box_ar = rect.width() / rect.height();
                let (u, v) = if img_ar > box_ar {
                    (box_ar / img_ar, 1.0) // image wider: trim left/right
                } else {
                    (1.0, img_ar / box_ar) // image taller: trim top/bottom
                };
                let uv = egui::Rect::from_min_max(
                    egui::pos2((1.0 - u) * 0.5, (1.0 - v) * 0.5),
                    egui::pos2(1.0 - (1.0 - u) * 0.5, 1.0 - (1.0 - v) * 0.5),
                );
                painter.image(tex.id(), rect, uv, egui::Color32::WHITE);
            } else {
                painter.rect_filled(rect, 0.0, crate::theme::t().panel_bg);
            }

            // Scrim under the text. Two stacked bands approximate a gradient
            // without needing a mesh, and keep white text readable over a bright
            // screenshot.
            let band_h = 56.0;
            let band = egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.bottom() - band_h),
                rect.max,
            );
            painter.rect_filled(band, 0.0, egui::Color32::from_black_alpha(210));
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(rect.left(), band.top() - 18.0),
                    egui::pos2(rect.right(), band.top()),
                ),
                0.0,
                egui::Color32::from_black_alpha(110),
            );

            let x = rect.left() + 10.0;
            painter.text(
                egui::pos2(x, band.top() + 8.0),
                egui::Align2::LEFT_TOP,
                title,
                egui::FontId::proportional(14.0),
                crate::theme::t().accent,
            );
            painter.text(
                egui::pos2(x, band.top() + 26.0),
                egui::Align2::LEFT_TOP,
                &name,
                egui::FontId::proportional(11.0),
                egui::Color32::from_white_alpha(190),
            );
            painter.text(
                egui::pos2(rect.right() - 10.0, band.top() + 26.0),
                egui::Align2::RIGHT_TOP,
                "click to open  .  drag to send",
                egui::FontId::proportional(11.0),
                egui::Color32::from_white_alpha(140),
            );
        });

        // Poll fairly often so the Win32 click detection above catches quick clicks.
        ctx.request_repaint_after(Duration::from_millis(30));
    }
}

/// Cursor position in physical screen pixels, or None if Win32 refuses.
fn cursor_pos() -> Option<(i32, i32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    unsafe {
        let mut pt = POINT::default();
        GetCursorPos(&mut pt).ok()?;
        Some((pt.x, pt.y))
    }
}

/// The toast's own HWND as an isize, for handing to the OLE drag helper.
fn window_hwnd(frame: &eframe::Frame) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match frame.window_handle().ok().map(|h| h.as_raw()) {
        Some(RawWindowHandle::Win32(h)) => Some(h.hwnd.get()),
        _ => None,
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
