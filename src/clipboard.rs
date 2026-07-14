// Windows clipboard writer: puts a screenshot on the clipboard in every
// format a paste target might look for, in one atomic
// Open/Empty/SetClipboardData*/Close session — the same trick ShareX uses
// to paste everywhere (terminals, Explorer, Discord/Slack, image editors):
//
//   - CF_DIB   : classic 32bpp BGRA bitmap (BI_RGB), bottom-up
//   - CF_DIBV5 : same pixels + an explicit alpha mask (BI_BITFIELDS) for
//                consumers that honor per-pixel alpha
//   - "PNG"    : a registered clipboard format containing the exact PNG
//                bytes written to disk (what Chrome/Firefox/Discord/GIMP/
//                Photoshop look for; ShareX/Greenshot use the same name)
//   - CF_HDROP : the saved .png file itself, via a DROPFILES block — this is
//                what makes pasting into Claude Code CLI / Windows Terminal /
//                Explorer work, since those only accept a dropped/pasted FILE

use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::time::Duration;

use image::RgbaImage;
use windows::core::w;
use windows::Win32::Foundation::{GlobalFree, BOOL, HANDLE, HGLOBAL, HWND, POINT};
use windows::Win32::Graphics::Gdi::{BITMAPINFOHEADER, BITMAPV5HEADER, BI_BITFIELDS, BI_RGB};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GHND};
use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP};
use windows::Win32::UI::Shell::DROPFILES;

/// Put `img` on the clipboard as CF_DIB + CF_DIBV5 + "PNG" + CF_HDROP in one
/// session. `png_bytes` must be the exact PNG encoding of `img`; `file_path`
/// must already exist on disk (CF_HDROP just references it — it doesn't
/// carry the bytes).
pub fn set_all(img: &RgbaImage, png_bytes: &[u8], file_path: &Path) -> anyhow::Result<()> {
    let (w, h) = (img.width(), img.height());
    let bgra = rgba_to_bgra_bottom_up(w, h, img.as_raw());
    let dib = build_dib(w, h, &bgra);
    let dibv5 = build_dibv5(w, h, &bgra);
    let dropfiles = build_dropfiles(file_path);

    unsafe {
        open_clipboard_with_retry()?;
        let result = set_all_formats(&dib, &dibv5, png_bytes, &dropfiles);
        // Always close, even if a Set* call failed partway through.
        let _ = CloseClipboard();
        result
    }
}

unsafe fn set_all_formats(
    dib: &[u8],
    dibv5: &[u8],
    png_bytes: &[u8],
    dropfiles: &[u8],
) -> anyhow::Result<()> {
    EmptyClipboard().map_err(win_err)?;

    let h_dib = alloc_global_copy(dib)?;
    SetClipboardData(CF_DIB.0 as u32, HANDLE(h_dib.0 as isize)).map_err(win_err)?;

    let h_dibv5 = alloc_global_copy(dibv5)?;
    SetClipboardData(CF_DIBV5.0 as u32, HANDLE(h_dibv5.0 as isize)).map_err(win_err)?;

    let png_fmt = RegisterClipboardFormatW(w!("PNG"));
    let h_png = alloc_global_copy(png_bytes)?;
    SetClipboardData(png_fmt, HANDLE(h_png.0 as isize)).map_err(win_err)?;

    let h_hdrop = alloc_global_copy(dropfiles)?;
    SetClipboardData(CF_HDROP.0 as u32, HANDLE(h_hdrop.0 as isize)).map_err(win_err)?;

    Ok(())
}

/// OpenClipboard can transiently fail if another app (browsers are the usual
/// culprit) has it open; retry briefly instead of failing the whole capture.
unsafe fn open_clipboard_with_retry() -> anyhow::Result<()> {
    for attempt in 0..10 {
        if OpenClipboard(HWND(0)).is_ok() {
            return Ok(());
        }
        if attempt < 9 {
            std::thread::sleep(Duration::from_millis(15));
        }
    }
    anyhow::bail!("could not open the clipboard (another app is holding it)")
}

/// A GMEM_MOVEABLE + zero-initialized global block containing a copy of
/// `bytes`, ready to hand to `SetClipboardData`. Ownership of the handle passes
/// to the OS once `SetClipboardData` succeeds — do not GlobalFree it after that.
unsafe fn alloc_global_copy(bytes: &[u8]) -> anyhow::Result<HGLOBAL> {
    let hglobal = GlobalAlloc(GHND, bytes.len().max(1)).map_err(win_err)?;
    let ptr = GlobalLock(hglobal);
    if ptr.is_null() {
        let _ = GlobalFree(hglobal); // this fn's Result is inverted in 0.56; discard it
        anyhow::bail!("GlobalLock failed");
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
    let _ = GlobalUnlock(hglobal); // spuriously "errors" on the ordinary case; discard it
    Ok(hglobal)
}

fn win_err(e: windows::core::Error) -> anyhow::Error {
    anyhow::anyhow!("{e}")
}

/// image::RgbaImage is top-down RGBA; CF_DIB/CF_DIBV5 want bottom-up BGRA.
fn rgba_to_bgra_bottom_up(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let stride = w * 4;
    let mut out = vec![0u8; stride * h];
    for y in 0..h {
        let src = &rgba[y * stride..y * stride + stride];
        let dst_y = h - 1 - y;
        let dst = &mut out[dst_y * stride..dst_y * stride + stride];
        for x in 0..w {
            let s = &src[x * 4..x * 4 + 4];
            dst[x * 4] = s[2]; // B
            dst[x * 4 + 1] = s[1]; // G
            dst[x * 4 + 2] = s[0]; // R
            dst[x * 4 + 3] = s[3]; // A
        }
    }
    out
}

fn build_dib(w: u32, h: u32, bgra_bottom_up: &[u8]) -> Vec<u8> {
    let header = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w as i32,
        biHeight: h as i32, // positive => bottom-up
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0 as u32,
        biSizeImage: w * h * 4,
        biXPelsPerMeter: 0,
        biYPelsPerMeter: 0,
        biClrUsed: 0,
        biClrImportant: 0,
    };
    let mut buf = Vec::with_capacity(std::mem::size_of::<BITMAPINFOHEADER>() + bgra_bottom_up.len());
    buf.extend_from_slice(unsafe { struct_bytes(&header) });
    buf.extend_from_slice(bgra_bottom_up);
    buf
}

fn build_dibv5(w: u32, h: u32, bgra_bottom_up: &[u8]) -> Vec<u8> {
    let mut header: BITMAPV5HEADER = unsafe { std::mem::zeroed() };
    header.bV5Size = std::mem::size_of::<BITMAPV5HEADER>() as u32;
    header.bV5Width = w as i32;
    header.bV5Height = h as i32; // positive => bottom-up
    header.bV5Planes = 1;
    header.bV5BitCount = 32;
    header.bV5Compression = BI_BITFIELDS;
    header.bV5SizeImage = w * h * 4;
    header.bV5RedMask = 0x00FF_0000;
    header.bV5GreenMask = 0x0000_FF00;
    header.bV5BlueMask = 0x0000_00FF;
    header.bV5AlphaMask = 0xFF00_0000;
    header.bV5CSType = 0x7352_4742; // 'sRGB' (LCS_sRGB)
    header.bV5Intent = 4; // LCS_GM_IMAGES

    let mut buf = Vec::with_capacity(std::mem::size_of::<BITMAPV5HEADER>() + bgra_bottom_up.len());
    buf.extend_from_slice(unsafe { struct_bytes(&header) });
    buf.extend_from_slice(bgra_bottom_up);
    buf
}

/// DROPFILES + a double-null-terminated wide string list (one entry: the
/// saved PNG's path) — exactly what CF_HDROP consumers expect.
fn build_dropfiles(path: &Path) -> Vec<u8> {
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0); // terminate this path
    wide.push(0); // terminate the list (double-null)

    let header = DROPFILES {
        pFiles: std::mem::size_of::<DROPFILES>() as u32, // offset to the file list
        pt: POINT { x: 0, y: 0 },
        fNC: BOOL(0),
        fWide: BOOL(1), // the list is UTF-16, not ANSI
    };
    let mut buf = Vec::with_capacity(std::mem::size_of::<DROPFILES>() + wide.len() * 2);
    buf.extend_from_slice(unsafe { struct_bytes(&header) });
    buf.extend_from_slice(unsafe {
        std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * 2)
    });
    buf
}

unsafe fn struct_bytes<T>(value: &T) -> &[u8] {
    std::slice::from_raw_parts((value as *const T).cast::<u8>(), std::mem::size_of::<T>())
}
