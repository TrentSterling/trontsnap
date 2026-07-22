// Dedicated Win32 + GDI fullscreen region picker — the ShareX approach.
//
// WHY THIS REPLACED THE OLD IN-PROCESS-REPURPOSE PICKER:
// The previous region picker borrowed the app's own eframe/GL window, blowing it up
// to fullscreen and restoring it afterward. Repurposing a live window meant we had to
// hide/resize/restore/refocus it every capture, and every one of those steps leaked a
// bug: a visible maximize animation, a black close flash, an off-screen restore, and
// "the hotkey behaves differently depending on what window is focused". ShareX avoids
// all of it by never touching its main window: it throws up a SEPARATE overlay window,
// born already fullscreen, with the frozen screenshot painted in. No GL context to warm,
// so it appears instantly. This module is that: a plain WS_POPUP layered-ish window on
// its own thread, GDI double-buffered, blocking-modal, returns the chosen crop.
//
// Everything is in physical pixels: the window covers the primary monitor at (0,0), the
// frozen frame IS the primary monitor, and window rects come back in the same screen
// coordinates, so there is zero scaling math (unlike the egui points version).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use image::RgbaImage;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateDIBSection, CreatePen,
    CreateSolidBrush, DeleteDC, DeleteObject, EndPaint, GetDC, GetStockObject, InvalidateRect,
    ReleaseDC, Rectangle, SelectObject, SetBkMode, SetStretchBltMode, SetTextColor, StretchBlt,
    TextOutW, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, COLORONCOLOR, DIB_RGB_COLORS, HBITMAP, HBRUSH,
    HDC, HGDIOBJ, HPEN, NULL_BRUSH, PAINTSTRUCT, PS_SOLID, SRCCOPY, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetActiveWindow, SetCapture, SetFocus, VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetForegroundWindow,
    GetMessageW, GetWindowLongPtrW, GetWindowThreadProcessId, LoadCursorW, PostQuitMessage,
    RegisterClassW, SetCursor, SetForegroundWindow, SetWindowLongPtrW, ShowWindow, TranslateMessage,
    GWLP_USERDATA, HCURSOR, IDC_CROSS, MSG, SW_SHOW, WM_DESTROY, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_RBUTTONDOWN, WM_SETCURSOR, WNDCLASSW,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
};

use crate::capture;
use crate::overlay::enumerate_windows_raw;
use crate::settings;

/// Below this pointer travel (px) a press+release is a click (smart-rect pick), not a drag.
const CLICK_SLOP: i32 = 5;
const LOUPE_ZOOM: i32 = 8;
const LOUPE_MARGIN: i32 = 26;
// The loupe box size is scrollwheel-adjustable during a pick and persisted via
// settings::loupe_size(). Bounds + per-notch step are multiples of LOUPE_ZOOM so
// src = size / zoom stays exact. Zoom is held constant, so a bigger box shows MORE
// magnified area (a larger viewer), not chunkier pixels.
const LOUPE_MIN: i32 = 96;
const LOUPE_MAX: i32 = 528;
const LOUPE_STEP: i32 = 48;

/// Reads the live theme's accent so the picker outline/loupe match whatever
/// theme is active, not a hardcoded cyan. Runs on the picker's own worker
/// thread; a RwLock read there is fine.
fn accent() -> COLORREF {
    let a = crate::theme::t().accent;
    rgb(a.r(), a.g(), a.b())
}
fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}

/// Entry point (run this on a dedicated thread — it blocks in a modal message loop):
/// grab the primary monitor, enumerate smart-capture targets, run the picker, and
/// deliver the crop (clipboard + disk + shutter). No involvement of the app's UI thread.
pub fn capture_region() {
    let Some((img, sel)) = run_picker() else { return };
    let w = img.width() as i32;
    let x = sel.left.clamp(0, w) as u32;
    let y = sel.top.clamp(0, img.height() as i32) as u32;
    let rw = ((sel.right - sel.left).max(0) as u32).min(img.width().saturating_sub(x));
    let rh = ((sel.bottom - sel.top).max(0) as u32).min(img.height().saturating_sub(y));
    if rw == 0 || rh == 0 {
        return;
    }
    let cropped = image::imageops::crop_imm(&img, x, y, rw, rh).to_image();
    match capture::deliver(&cropped) {
        Ok(path) => println!("captured {rw}x{rh} -> clipboard + {}", path.display()),
        Err(e) => eprintln!("trontsnap: region deliver failed: {e:#}"),
    }
}

/// Interactive rect selection only (for the video recorder): freeze the screen, run
/// the same modal picker, return the chosen rect in physical primary-monitor px.
/// Nothing is saved or copied. Blocks; run on a dedicated thread.
pub fn pick_rect() -> Option<RECT> {
    run_picker().map(|(_, sel)| sel)
}

/// Shared picker session: one at a time (rapid repeat hotkey presses while an overlay
/// is already up are ignored), grab the frozen frame, enumerate targets, run the modal
/// overlay. Returns the frame and the raw (unclamped) selection.
fn run_picker() -> Option<(RgbaImage, RECT)> {
    static ACTIVE: AtomicBool = AtomicBool::new(false);
    if ACTIVE.swap(true, Ordering::SeqCst) {
        return None;
    }
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            ACTIVE.store(false, Ordering::SeqCst);
        }
    }
    let _guard = Guard;

    // Freeze the frame WITH the cursor composited in (if enabled), so the pointer is
    // frozen where it was at Ctrl+PrtSc time and lands in the crop — ShareX behaviour.
    let img = match capture::grab_for_shot() {
        Ok(i) => i,
        Err(e) => {
            eprintln!("trontsnap: region grab failed: {e:#}");
            return None;
        }
    };
    let w = img.width() as i32;
    let h = img.height() as i32;
    let targets = rect_map(&enumerate_windows_raw(), w, h);
    let sel = pick(&img, w, h, &targets)?;
    Some((img, sel))
}

/// Build the smart-capture rect map: every enumerated window clamped to the frame, plus
/// a full-screen fallback appended last. Selection picks the SMALLEST rect under the
/// cursor, so the full-screen rect only wins on empty desktop.
fn rect_map(raw: &[RECT], w: i32, h: i32) -> Vec<RECT> {
    let mut out = Vec::with_capacity(raw.len() + 1);
    for r in raw {
        let l = r.left.max(0);
        let t = r.top.max(0);
        let rr = r.right.min(w);
        let bb = r.bottom.min(h);
        if rr - l >= 8 && bb - t >= 8 {
            out.push(RECT { left: l, top: t, right: rr, bottom: bb });
        }
    }
    out.push(RECT { left: 0, top: 0, right: w, bottom: h });
    out
}

/// Per-session picker state, reached from the window proc via GWLP_USERDATA.
struct Picker {
    w: i32,
    h: i32,
    bright_dc: HDC, // frozen frame at full brightness
    base_dc: HDC,   // pre-composited dim backdrop + faint rect map (static; built once)
    back_dc: HDC,   // double-buffer
    targets: Vec<RECT>,
    cursor: POINT,
    loupe_size: i32, // scrollwheel-adjustable on-screen loupe box (px); persisted
    dragging: bool,
    drag_start: POINT,
    drag_now: POINT,
    result: Option<RECT>,
    accent_pen: HPEN,
    hollow: HGDIOBJ,
    label_bg: HBRUSH,
    cross: HCURSOR,
}

/// Run the modal picker. Returns the chosen rect in physical screen px, or None on cancel.
fn pick(img: &RgbaImage, w: i32, h: i32, targets: &[RECT]) -> Option<RECT> {
    unsafe {
        let screen = GetDC(HWND(0));
        let (bright_dc, bright_bmp) = make_frame_dc(screen, img, w, h, false);
        let (dark_dc, dark_bmp) = make_frame_dc(screen, img, w, h, true);
        let back_dc = CreateCompatibleDC(screen);
        let back_bmp = CreateCompatibleBitmap(screen, w, h);
        SelectObject(back_dc, back_bmp);

        // Pre-compose the STATIC layer once: the dim backdrop plus the faint rect map.
        // None of that changes during a pick, so building it up front (instead of
        // re-darkening the whole screen and re-stroking every window rect on every
        // mouse move) keeps each frame cheap — a busy paint thread is exactly what
        // makes Windows flash the wait cursor over the overlay.
        let base_dc = CreateCompatibleDC(screen);
        let base_bmp = CreateCompatibleBitmap(screen, w, h);
        SelectObject(base_dc, base_bmp);
        let _ = BitBlt(base_dc, 0, 0, w, h, dark_dc, 0, 0, SRCCOPY);
        let faint = CreatePen(PS_SOLID, 1, rgb(60, 140, 175));
        SelectObject(base_dc, GetStockObject(NULL_BRUSH));
        SelectObject(base_dc, faint);
        for r in targets {
            let _ = Rectangle(base_dc, r.left, r.top, r.right, r.bottom);
        }
        ReleaseDC(HWND(0), screen);
        // The dark frame was only needed to build the base; drop it now.
        let _ = DeleteDC(dark_dc);
        let _ = DeleteObject(dark_bmp);

        let mut picker = Picker {
            w,
            h,
            bright_dc,
            base_dc,
            back_dc,
            targets: targets.to_vec(),
            cursor: POINT { x: -1, y: -1 },
            loupe_size: settings::loupe_size().clamp(LOUPE_MIN, LOUPE_MAX),
            dragging: false,
            drag_start: POINT { x: 0, y: 0 },
            drag_now: POINT { x: 0, y: 0 },
            result: None,
            accent_pen: CreatePen(PS_SOLID, 2, accent()),
            hollow: GetStockObject(NULL_BRUSH),
            label_bg: CreateSolidBrush(rgb(10, 12, 16)),
            cross: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
        };

        let hinstance = GetModuleHandleW(None).unwrap_or_default();
        let class_name = ensure_class(hinstance.into());

        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            PCWSTR(class_name.as_ptr()),
            PCWSTR::null(),
            WS_POPUP | WS_VISIBLE,
            0,
            0,
            w,
            h,
            HWND(0),
            None,
            hinstance,
            None,
        );

        SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut picker as *mut Picker as isize);
        let _ = ShowWindow(hwnd, SW_SHOW);
        force_foreground(hwnd);
        SetCapture(hwnd);
        // With capture held, Windows stops sending WM_SETCURSOR, so the class cursor
        // never applies and the app-starting/wait spinner sticks. Force it here and on
        // every WM_MOUSEMOVE (see wndproc) so it's always the crosshair.
        SetCursor(picker.cross);
        let _ = InvalidateRect(hwnd, None, false);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND(0), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // Teardown (window already destroyed by the proc). DeleteDC first so the
        // selected pen/bitmap are released before we DeleteObject them.
        let _ = DeleteDC(back_dc);
        let _ = DeleteObject(back_bmp);
        let _ = DeleteDC(bright_dc);
        let _ = DeleteObject(bright_bmp);
        let _ = DeleteDC(base_dc);
        let _ = DeleteObject(base_bmp);
        let _ = DeleteObject(faint);
        let _ = DeleteObject(picker.accent_pen);
        let _ = DeleteObject(picker.label_bg);

        picker.result
    }
}

/// Create a memory DC holding the frozen frame as a top-down 32bpp DIB (optionally
/// darkened for the dim backdrop). Returns (dc, bitmap) — caller deletes both.
unsafe fn make_frame_dc(screen: HDC, img: &RgbaImage, w: i32, h: i32, darken: bool) -> (HDC, HBITMAP) {
    let dc = CreateCompatibleDC(screen);
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // negative = top-down, matches the image crate's row order
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0 as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let hbm = CreateDIBSection(screen, &bmi, DIB_RGB_COLORS, &mut bits, HANDLE(0), 0)
        .expect("CreateDIBSection");
    // GDI 32bpp DIB is byte order B,G,R,X; the image is R,G,B,A — swap R/B.
    let dst = std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);
    for (i, px) in img.pixels().enumerate() {
        let [r, g, b, _a] = px.0;
        let (r, g, b) = if darken {
            ((r as u32 * 2 / 5) as u8, (g as u32 * 2 / 5) as u8, (b as u32 * 2 / 5) as u8)
        } else {
            (r, g, b)
        };
        let o = i * 4;
        dst[o] = b;
        dst[o + 1] = g;
        dst[o + 2] = r;
        dst[o + 3] = 255;
    }
    SelectObject(dc, hbm);
    (dc, hbm)
}

/// Register the window class once per process; returns the (null-terminated) class name.
fn ensure_class(hinstance: windows::Win32::Foundation::HINSTANCE) -> &'static Vec<u16> {
    static CLASS: OnceLock<Vec<u16>> = OnceLock::new();
    CLASS.get_or_init(|| {
        let name: Vec<u16> = "TrontSnapRegionOverlay\0".encode_utf16().collect();
        unsafe {
            let cursor: HCURSOR = LoadCursorW(None, IDC_CROSS).unwrap_or_default();
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinstance,
                lpszClassName: PCWSTR(name.as_ptr()),
                hCursor: cursor,
                hbrBackground: HBRUSH(0), // no bg erase (we double-buffer)
                ..Default::default()
            };
            RegisterClassW(&wc);
        }
        name
    })
}

/// Force our overlay to the foreground + keyboard focus regardless of which app was
/// active (attach-thread-input trick), so Esc works and clicks land immediately.
unsafe fn force_foreground(hwnd: HWND) {
    let fg = GetForegroundWindow();
    let fg_thread = GetWindowThreadProcessId(fg, None);
    let our_thread = GetCurrentThreadId();
    if fg_thread != 0 && fg_thread != our_thread {
        let _ = AttachThreadInput(our_thread, fg_thread, true);
        let _ = SetForegroundWindow(hwnd);
        SetFocus(hwnd);
        SetActiveWindow(hwnd);
        let _ = AttachThreadInput(our_thread, fg_thread, false);
    } else {
        let _ = SetForegroundWindow(hwnd);
        SetFocus(hwnd);
    }
}

fn lp_point(lp: LPARAM) -> POINT {
    let x = (lp.0 & 0xFFFF) as i16 as i32;
    let y = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
    POINT { x, y }
}

/// Normalized selection rect for the current frame: the drag rect while dragging, else
/// the smallest smart-capture target under the cursor.
fn current_sel(p: &Picker) -> Option<RECT> {
    if p.dragging {
        Some(norm_rect(p.drag_start, p.drag_now))
    } else {
        smallest_at(&p.targets, p.cursor)
    }
}

fn norm_rect(a: POINT, b: POINT) -> RECT {
    RECT {
        left: a.x.min(b.x),
        top: a.y.min(b.y),
        right: a.x.max(b.x),
        bottom: a.y.max(b.y),
    }
}

fn contains(r: &RECT, pt: POINT) -> bool {
    pt.x >= r.left && pt.x < r.right && pt.y >= r.top && pt.y < r.bottom
}

/// Smallest-area target rect containing the cursor (tie-break nearest center).
fn smallest_at(targets: &[RECT], pt: POINT) -> Option<RECT> {
    targets
        .iter()
        .filter(|r| contains(r, pt))
        .min_by(|a, b| {
            let area = |r: &RECT| (r.right - r.left) as i64 * (r.bottom - r.top) as i64;
            let dist = |r: &RECT| {
                let cx = (r.left + r.right) / 2 - pt.x;
                let cy = (r.top + r.bottom) / 2 - pt.y;
                (cx as i64 * cx as i64) + (cy as i64 * cy as i64)
            };
            area(a).cmp(&area(b)).then_with(|| dist(a).cmp(&dist(b)))
        })
        .copied()
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Picker;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let p = &mut *ptr;

    match msg {
        WM_MOUSEMOVE => {
            SetCursor(p.cross); // capture suppresses WM_SETCURSOR; force crosshair here
            p.cursor = lp_point(lp);
            if p.dragging {
                p.drag_now = p.cursor;
            }
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            p.dragging = true;
            p.drag_start = lp_point(lp);
            p.drag_now = p.drag_start;
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let up = lp_point(lp);
            let travel = (up.x - p.drag_start.x).abs().max((up.y - p.drag_start.y).abs());
            p.dragging = false;
            p.result = if travel < CLICK_SLOP {
                // A click: smart-capture the smallest target under the cursor.
                smallest_at(&p.targets, up)
            } else {
                Some(norm_rect(p.drag_start, up))
            };
            let _ = ReleaseCapture();
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_RBUTTONDOWN => {
            p.result = None;
            let _ = ReleaseCapture();
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_KEYDOWN if wp.0 as u16 == VK_ESCAPE.0 => {
            p.result = None;
            let _ = ReleaseCapture();
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            // Grow/shrink the loupe box a notch at a time; persist so it sticks across
            // captures and restarts. Zoom is constant, so this changes how much magnified
            // area the viewer shows, not the pixel scale.
            let delta = ((wp.0 >> 16) & 0xFFFF) as i16 as i32;
            let notches = delta / 120;
            if notches != 0 {
                let new = (p.loupe_size + notches * LOUPE_STEP).clamp(LOUPE_MIN, LOUPE_MAX);
                if new != p.loupe_size {
                    p.loupe_size = new;
                    settings::set_loupe_size(new);
                    let _ = InvalidateRect(hwnd, None, false);
                }
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            SetCursor(LoadCursorW(None, IDC_CROSS).unwrap_or_default());
            LRESULT(1)
        }
        WM_PAINT => {
            paint(p, hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe fn paint(p: &Picker, hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);

    // 1. Static base: dim backdrop + faint rect map, pre-composited once.
    let _ = BitBlt(p.back_dc, 0, 0, p.w, p.h, p.base_dc, 0, 0, SRCCOPY);

    let sel = current_sel(p);

    // 2. Restore bright pixels inside the selection.
    if let Some(r) = sel {
        let (rw, rh) = (r.right - r.left, r.bottom - r.top);
        if rw > 0 && rh > 0 {
            let _ = BitBlt(p.back_dc, r.left, r.top, rw, rh, p.bright_dc, r.left, r.top, SRCCOPY);
        }
    }

    // 3. Bright selection outline + live dimensions.
    if let Some(r) = sel {
        SelectObject(p.back_dc, p.hollow);
        SelectObject(p.back_dc, p.accent_pen);
        let _ = Rectangle(p.back_dc, r.left, r.top, r.right, r.bottom);
        draw_dimensions(p, r);
    }

    // 4. Pixel-crisp loupe at the cursor.
    if p.cursor.x >= 0 {
        draw_loupe(p);
    }

    let _ = BitBlt(hdc, 0, 0, p.w, p.h, p.back_dc, 0, 0, SRCCOPY);
    let _ = EndPaint(hwnd, &ps);
}

unsafe fn draw_dimensions(p: &Picker, r: RECT) {
    let text: Vec<u16> = format!("{} x {}", r.right - r.left, r.bottom - r.top)
        .encode_utf16()
        .collect();
    let mut x = r.left;
    let mut y = r.top - 22;
    if y < 2 {
        y = r.top + 4;
    }
    if x < 2 {
        x = 2;
    }
    let bg = RECT { left: x, top: y, right: x + (text.len() as i32) * 8 + 10, bottom: y + 20 };
    fill_rect(p.back_dc, &bg, p.label_bg);
    SetBkMode(p.back_dc, TRANSPARENT);
    SetTextColor(p.back_dc, rgb(245, 250, 255));
    let _ = TextOutW(p.back_dc, x + 5, y + 3, &text);
}

unsafe fn draw_loupe(p: &Picker) {
    let size = p.loupe_size;
    let mut ox = p.cursor.x + LOUPE_MARGIN;
    let mut oy = p.cursor.y + LOUPE_MARGIN;
    if ox + size > p.w {
        ox = p.cursor.x - LOUPE_MARGIN - size;
    }
    if oy + size > p.h {
        oy = p.cursor.y - LOUPE_MARGIN - size;
    }
    let src = size / LOUPE_ZOOM;
    SetStretchBltMode(p.back_dc, COLORONCOLOR);
    let _ = StretchBlt(
        p.back_dc,
        ox,
        oy,
        size,
        size,
        p.bright_dc,
        p.cursor.x - src / 2,
        p.cursor.y - src / 2,
        src,
        src,
        SRCCOPY,
    );
    let box_r = RECT { left: ox, top: oy, right: ox + size, bottom: oy + size };
    SelectObject(p.back_dc, p.hollow);
    SelectObject(p.back_dc, p.accent_pen);
    let _ = Rectangle(p.back_dc, box_r.left, box_r.top, box_r.right, box_r.bottom);
    // Crosshair.
    let cx = ox + size / 2;
    let cy = oy + size / 2;
    let _ = Rectangle(p.back_dc, cx - 1, box_r.top, cx + 1, box_r.bottom);
    let _ = Rectangle(p.back_dc, box_r.left, cy - 1, box_r.right, cy + 1);
}

#[cfg(test)]
mod tests {
    use super::{norm_rect, rect_map, smallest_at};
    use windows::Win32::Foundation::{POINT, RECT};

    fn r(l: i32, t: i32, ri: i32, b: i32) -> RECT {
        RECT { left: l, top: t, right: ri, bottom: b }
    }
    fn p(x: i32, y: i32) -> POINT {
        POINT { x, y }
    }

    #[test]
    fn smallest_rect_under_cursor_wins() {
        // A small window sitting inside a big one: the cursor is in both, small wins.
        let big = r(0, 0, 1000, 1000);
        let small = r(100, 100, 300, 300);
        let got = smallest_at(&[big, small], p(200, 200)).unwrap();
        assert_eq!((got.left, got.top, got.right, got.bottom), (100, 100, 300, 300));
    }

    #[test]
    fn cursor_outside_all_targets_is_none() {
        assert!(smallest_at(&[r(0, 0, 10, 10)], p(500, 500)).is_none());
    }

    #[test]
    fn rect_map_clamps_drops_slivers_and_appends_fullscreen() {
        // One rect overshoots the frame (clamped), one is a sliver (dropped).
        let raw = [r(-50, -50, 400, 400), r(10, 10, 12, 12)];
        let map = rect_map(&raw, 1920, 1080);
        // clamped rect + fullscreen fallback == 2; the 2x2 sliver is gone.
        assert_eq!(map.len(), 2);
        let clamped = map[0];
        assert_eq!((clamped.left, clamped.top), (0, 0));
        let full = *map.last().unwrap();
        assert_eq!((full.left, full.top, full.right, full.bottom), (0, 0, 1920, 1080));
    }

    #[test]
    fn norm_rect_orders_corners() {
        let n = norm_rect(p(300, 200), p(100, 50));
        assert_eq!((n.left, n.top, n.right, n.bottom), (100, 50, 300, 200));
    }
}

unsafe fn fill_rect(dc: HDC, r: &RECT, brush: HBRUSH) {
    windows::Win32::Graphics::Gdi::FillRect(dc, r, brush);
}
