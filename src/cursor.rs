// Composite the live mouse cursor onto a captured frame.
//
// xcap grabs the screen WITHOUT the pointer, so when "capture cursor" is on we draw the
// current system cursor into the RGBA frame ourselves. We hand it to GDI (DrawIconEx)
// rather than blending pixels by hand, so every cursor style renders the way Windows
// itself would: color arrows with per-pixel alpha, the monochrome I-beam (AND/XOR mask),
// link hands, resize arrows — DrawIconEx does the masking for us.
//
// Everything is physical pixels. `origin` is the captured region's top-left in screen
// space; the primary monitor is (0,0), which is all TrontSnap captures today.

use image::RgbaImage;
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HBRUSH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DrawIconEx, GetCursorInfo, GetIconInfo, CURSORINFO, CURSOR_SHOWING, DI_NORMAL, HICON, ICONINFO,
};

/// If the "capture cursor" setting is on, draw the current cursor onto `img`.
pub fn maybe_overlay(img: &mut RgbaImage, origin: (i32, i32)) {
    if !crate::settings::capture_cursor() {
        return;
    }
    if let Err(e) = overlay(img, origin) {
        // Non-fatal: a missing cursor should never lose the screenshot.
        eprintln!("trontsnap: cursor overlay skipped: {e:#}");
    }
}

fn overlay(img: &mut RgbaImage, origin: (i32, i32)) -> anyhow::Result<()> {
    let w = img.width() as i32;
    let h = img.height() as i32;
    unsafe {
        // 1. Which cursor, and where is its hotspot pinned?
        let mut ci = CURSORINFO {
            cbSize: std::mem::size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        GetCursorInfo(&mut ci)?;
        // Cursor hidden (e.g. suppressed by a fullscreen app) -> nothing to draw.
        if (ci.flags.0 & CURSOR_SHOWING.0) == 0 || ci.hCursor.0 == 0 {
            return Ok(());
        }
        let hicon = HICON(ci.hCursor.0);
        // The cursor bitmap is offset so its hotspot lands on the pointer position.
        let mut ii = ICONINFO::default();
        GetIconInfo(hicon, &mut ii)?;
        // GetIconInfo hands back freshly-created bitmaps; free them (we only need the hotspot).
        if ii.hbmColor.0 != 0 {
            let _ = DeleteObject(ii.hbmColor);
        }
        if ii.hbmMask.0 != 0 {
            let _ = DeleteObject(ii.hbmMask);
        }
        let x = ci.ptScreenPos.x - origin.0 - ii.xHotspot as i32;
        let y = ci.ptScreenPos.y - origin.1 - ii.yHotspot as i32;

        // 2. Wrap the frame in a top-down 32bpp DIB we can draw onto.
        let screen = GetDC(HWND(0));
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
        let hbm: HBITMAP = CreateDIBSection(screen, &bmi, DIB_RGB_COLORS, &mut bits, HANDLE(0), 0)?;
        ReleaseDC(HWND(0), screen);
        let old = SelectObject(dc, hbm);

        // image is R,G,B,A; a GDI 32bpp DIB is B,G,R,A — copy in with the swap.
        let dst = std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);
        for (i, px) in img.pixels().enumerate() {
            let [r, g, b, a] = px.0;
            let o = i * 4;
            dst[o] = b;
            dst[o + 1] = g;
            dst[o + 2] = r;
            dst[o + 3] = a;
        }

        // 3. Let GDI composite the cursor (mask / alpha / monochrome all handled here).
        let _ = DrawIconEx(dc, x, y, hicon, 0, 0, 0, HBRUSH(0), DI_NORMAL);

        // 4. Read it back (BGRA -> RGBA), forcing opaque alpha: a screenshot has no
        //    transparency, and DrawIconEx can leave the DIB's alpha bytes unset.
        for (i, px) in img.pixels_mut().enumerate() {
            let o = i * 4;
            px.0 = [dst[o + 2], dst[o + 1], dst[o], 255];
        }

        SelectObject(dc, old);
        let _ = DeleteObject(hbm);
        let _ = DeleteDC(dc);
    }
    Ok(())
}
