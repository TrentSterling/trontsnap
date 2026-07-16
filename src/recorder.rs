// Screen recording (Ctrl+Shift+PrtSc toggle): DXGI Desktop Duplication -> region crop
// -> GDI cursor composite -> Media Foundation H.264 -> Pictures\TrontSnap\*.mp4.
//
// WHY THIS STACK:
//   - DXGI Desktop Duplication grabs frames on the GPU (~1ms) — a BitBlt loop (what
//     xcap does per shot) burns a whole core at 30fps and tears.
//   - Media Foundation's SinkWriter writes H.264 MP4 natively (NVENC via the hardware
//     MFT when available), so there is no ffmpeg binary and no new crate.
//   - The cursor is NOT in duplicated frames; we composite it per frame with the same
//     GDI DrawIconEx approach as cursor.rs (honors the "Capture cursor" setting), into
//     a persistent DIB whose bits double as the encoder's RGB32 input.
//
// FLOW: toggle() -> pick region (frozen-frame picker, same as screenshots) -> "REC"
// pill appears (own thread; click it or hit Ctrl+Shift+PrtSc again to stop) -> frames
// are paced at a constant 30fps (static screen = duplicated frames, correct CFR) ->
// stop -> Finalize() -> shutter + toast.
//
// Everything runs on dedicated threads; the app's UI thread is never involved.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::{Duration, Instant};

use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HANDLE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE,
    D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    IDXGIAdapter, IDXGIDevice, IDXGIOutput, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
    DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_DESC, DXGI_OUTDUPL_FRAME_INFO,
    DXGI_OUTPUT_DESC,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateCompatibleDC, CreateDIBSection, CreateSolidBrush, DeleteDC, DeleteObject,
    EndPaint, FillRect, GetDC, ReleaseDC, SelectObject, SetBkMode, SetTextColor, TextOutW,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBRUSH, HDC, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFMediaBuffer, IMFMediaType, IMFSample, IMFSinkWriter, MFCreateAttributes,
    MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL, MFShutdown,
    MFStartup, MFVideoFormat_H264, MFVideoFormat_RGB32, MFVideoInterlace_Progressive,
    MFMediaType_Video, MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE,
    MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO,
    MF_MT_SUBTYPE, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, DrawIconEx, GetCursorInfo,
    GetIconInfo, GetMessageW, KillTimer, PostQuitMessage, RegisterClassW, SetTimer,
    TranslateMessage, CURSORINFO, CURSOR_SHOWING, DI_NORMAL, HICON, ICONINFO, MSG, WM_DESTROY,
    WM_LBUTTONUP, WM_PAINT, WM_TIMER, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
};

use crate::capture;
use crate::region_win32;
use crate::settings;

const FPS: u32 = 30;
// windows 0.56 doesn't re-export MF_VERSION; this is MF_SDK_VERSION << 16 | MF_API_VERSION.
const MF_VERSION: u32 = 0x0002_0070;

/// A recording session is live (pick may still be up, or frames are flowing).
static RECORDING: AtomicBool = AtomicBool::new(false);
/// Stop requested (second hotkey press, or pill clicked).
static STOP: AtomicBool = AtomicBool::new(false);

/// Hotkey entry: first press starts a session (region pick -> record), second press
/// stops it. Safe to call from any thread.
pub fn toggle() {
    if RECORDING.load(Ordering::SeqCst) {
        STOP.store(true, Ordering::SeqCst);
    } else {
        std::thread::spawn(record_session);
    }
}

fn record_session() {
    if RECORDING.swap(true, Ordering::SeqCst) {
        return; // already live
    }
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            RECORDING.store(false, Ordering::SeqCst);
            STOP.store(false, Ordering::SeqCst);
        }
    }
    let _guard = Guard;
    STOP.store(false, Ordering::SeqCst);

    // Same frozen-frame picker as screenshots: click a window for its rect, or drag.
    let Some(sel) = region_win32::pick_rect() else {
        return; // cancelled
    };
    // H.264 wants even dimensions; also clamp away degenerate picks.
    let x = sel.left.max(0);
    let y = sel.top.max(0);
    let w = ((sel.right - x).max(0) & !1) as u32;
    let h = ((sel.bottom - y).max(0) & !1) as u32;
    if w < 32 || h < 32 {
        eprintln!("trontsnap: record region too small ({w}x{h}), ignoring");
        return;
    }

    // The REC pill lives on its own thread with its own message loop; it watches
    // RECORDING to know when to close, and clicking it sets STOP.
    let region = RECT { left: x, top: y, right: x + w as i32, bottom: y + h as i32 };
    std::thread::spawn(move || pill_thread(region));

    match capture_encode(x, y, w, h) {
        Ok(path) => {
            // Best-effort: the finished MP4 goes on the clipboard as a file (CF_HDROP),
            // so it pastes straight into Discord — same spirit as screenshots.
            if let Err(e) = crate::clipboard::set_file(&path) {
                eprintln!("trontsnap: recording clipboard set failed: {e:#}");
            }
            crate::sound::play_shutter();
            eprintln!("trontsnap: recorded {w}x{h} -> {}", path.display());
            if let Ok(exe) = std::env::current_exe() {
                let _ = std::process::Command::new(exe).arg("toast").arg(&path).spawn();
            }
        }
        Err(e) => eprintln!("trontsnap: recording failed: {e:#}"),
    }
}

/// The capture + encode loop. Blocks until STOP; returns the finished file.
fn capture_encode(rx: i32, ry: i32, w: u32, h: u32) -> anyhow::Result<PathBuf> {
    unsafe {
        // COM + MF for this thread/session. RPC_E_CHANGED_MODE just means the thread
        // already had COM in another mode — fine for everything we use.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, 0)?;
        struct MfGuard;
        impl Drop for MfGuard {
            fn drop(&mut self) {
                unsafe {
                    let _ = MFShutdown();
                    CoUninitialize();
                }
            }
        }
        let _mf = MfGuard;

        // --- D3D11 device + duplication of the primary output -------------------
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            windows::Win32::Foundation::HMODULE(0),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
        let device = device.ok_or_else(|| anyhow::anyhow!("no d3d11 device"))?;
        let context = context.ok_or_else(|| anyhow::anyhow!("no d3d11 context"))?;

        let dxgi_dev: IDXGIDevice = device.cast()?;
        let adapter: IDXGIAdapter = dxgi_dev.GetAdapter()?;
        let output = primary_output(&adapter)?;
        let output1: IDXGIOutput1 = output.cast()?;
        let dup: IDXGIOutputDuplication = output1.DuplicateOutput(&device)?;

        let mut dup_desc = DXGI_OUTDUPL_DESC::default();
        dup.GetDesc(&mut dup_desc);
        let mon_w = dup_desc.ModeDesc.Width;
        let mon_h = dup_desc.ModeDesc.Height;
        // The picker rect is primary-monitor px, same space as the duplication.
        let rx = (rx as u32).min(mon_w.saturating_sub(w));
        let ry = (ry as u32).min(mon_h.saturating_sub(h));

        // CPU-readable staging copy of the full monitor frame.
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Width: mon_w,
            Height: mon_h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut staging: Option<ID3D11Texture2D> = None;
        device.CreateTexture2D(&staging_desc, None, Some(&mut staging))?;
        let staging = staging.ok_or_else(|| anyhow::anyhow!("no staging texture"))?;

        // --- persistent compose DIB (frame + cursor), doubles as encoder input ---
        // GDI 32bpp top-down BGRA == duplicated frame layout == MF RGB32 top-down.
        let screen = GetDC(HWND(0));
        let dib_dc = CreateCompatibleDC(screen);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32), // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0 as u32,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(screen, &bmi, DIB_RGB_COLORS, &mut bits, HANDLE(0), 0)?;
        ReleaseDC(HWND(0), screen);
        let old_bmp = SelectObject(dib_dc, dib);
        let frame_len = (w * h * 4) as usize;
        struct DibGuard(HDC, windows::Win32::Graphics::Gdi::HGDIOBJ, windows::Win32::Graphics::Gdi::HBITMAP);
        impl Drop for DibGuard {
            fn drop(&mut self) {
                unsafe {
                    SelectObject(self.0, self.1);
                    let _ = DeleteObject(self.2);
                    let _ = DeleteDC(self.0);
                }
            }
        }
        let _dib_guard = DibGuard(dib_dc, old_bmp, dib);

        // --- MP4 sink writer -----------------------------------------------------
        let path = out_path()?;
        let wide: Vec<u16> = path.to_string_lossy().encode_utf16().chain([0]).collect();
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs = attrs.ok_or_else(|| anyhow::anyhow!("no MF attributes"))?;
        attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?; // NVENC when available
        let writer: IMFSinkWriter =
            MFCreateSinkWriterFromURL(PCWSTR(wide.as_ptr()), None, Some(&attrs))?;

        // ~0.13 bits per pixel per frame lands near screen-recorder norms; clamped.
        let bitrate = ((w as u64 * h as u64 * FPS as u64) / 8).clamp(4_000_000, 24_000_000) as u32;
        let out_type: IMFMediaType = MFCreateMediaType()?;
        out_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        out_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
        out_type.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
        out_type.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64)?;
        out_type.SetUINT64(&MF_MT_FRAME_RATE, ((FPS as u64) << 32) | 1)?;
        out_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
        out_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        let stream = writer.AddStream(&out_type)?;

        let in_type: IMFMediaType = MFCreateMediaType()?;
        in_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        in_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        in_type.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64)?;
        in_type.SetUINT64(&MF_MT_FRAME_RATE, ((FPS as u64) << 32) | 1)?;
        in_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
        in_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        // Positive stride = top-down RGB32, matching the DIB.
        in_type.SetUINT32(&MF_MT_DEFAULT_STRIDE, w * 4)?;
        writer.SetInputMediaType(stream, &in_type, None)?;
        writer.BeginWriting()?;

        // --- 30fps CFR loop ------------------------------------------------------
        // Pace against wall clock; a static screen means AcquireNextFrame times out
        // and we re-encode the previous staging content (constant frame rate).
        let started = Instant::now();
        let mut n: u64 = 0;
        let mut have_frame = false;
        let mut cursor_cache = CursorCache::default();

        while !STOP.load(Ordering::SeqCst) {
            let target = Duration::from_nanos(n * 1_000_000_000 / FPS as u64);
            if let Some(rest) = target.checked_sub(started.elapsed()) {
                std::thread::sleep(rest);
            }

            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut res: Option<IDXGIResource> = None;
            match dup.AcquireNextFrame(0, &mut info, &mut res) {
                Ok(()) => {
                    if let Some(r) = res {
                        let tex: ID3D11Texture2D = r.cast()?;
                        context.CopyResource(&staging, &tex);
                    }
                    let _ = dup.ReleaseFrame();
                    have_frame = true;
                }
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => {} // unchanged screen
                Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => {
                    // Mode switch / secure desktop: end the recording cleanly with
                    // what we have rather than erroring the whole session away.
                    eprintln!("trontsnap: display access lost, finishing recording");
                    break;
                }
                Err(e) => return Err(e.into()),
            }
            if !have_frame {
                continue; // nothing captured yet; don't encode garbage
            }

            // staging -> DIB (crop the record region out of the full-monitor frame)
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let src_base = mapped.pData as *const u8;
            let pitch = mapped.RowPitch as usize;
            let row_bytes = (w * 4) as usize;
            let dst_base = bits as *mut u8;
            for row in 0..h as usize {
                let src = src_base.add((ry as usize + row) * pitch + (rx as usize * 4));
                let dst = dst_base.add(row * row_bytes);
                std::ptr::copy_nonoverlapping(src, dst, row_bytes);
            }
            context.Unmap(&staging, 0);

            // Cursor: duplication frames never include it; composite like cursor.rs.
            if settings::capture_cursor() {
                cursor_cache.draw(dib_dc, rx as i32, ry as i32);
            }

            // DIB bits -> MF sample
            let buffer: IMFMediaBuffer = MFCreateMemoryBuffer(frame_len as u32)?;
            let mut ptr: *mut u8 = std::ptr::null_mut();
            buffer.Lock(&mut ptr, None, None)?;
            std::ptr::copy_nonoverlapping(dst_base as *const u8, ptr, frame_len);
            buffer.Unlock()?;
            buffer.SetCurrentLength(frame_len as u32)?;
            let sample: IMFSample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime((n as i64) * 10_000_000 / FPS as i64)?;
            sample.SetSampleDuration(10_000_000 / FPS as i64)?;
            writer.WriteSample(stream, &sample)?;
            n += 1;
        }

        writer.Finalize()?;
        if n == 0 {
            let _ = std::fs::remove_file(&path);
            anyhow::bail!("no frames captured");
        }
        Ok(path)
    }
}

/// The DXGI output whose desktop rect contains (0,0) — the primary monitor, the same
/// one the picker and xcap operate on. Falls back to output 0.
unsafe fn primary_output(adapter: &IDXGIAdapter) -> anyhow::Result<IDXGIOutput> {
    let mut i = 0u32;
    let mut first: Option<IDXGIOutput> = None;
    while let Ok(out) = adapter.EnumOutputs(i) {
        let mut desc = DXGI_OUTPUT_DESC::default();
        if out.GetDesc(&mut desc).is_ok() {
            let r = desc.DesktopCoordinates;
            if r.left <= 0 && r.top <= 0 && r.right > 0 && r.bottom > 0 {
                return Ok(out);
            }
        }
        if first.is_none() {
            first = Some(out);
        }
        i += 1;
    }
    first.ok_or_else(|| anyhow::anyhow!("no DXGI outputs"))
}

fn out_path() -> anyhow::Result<PathBuf> {
    let dir = capture::trontsnap_dir()?;
    std::fs::create_dir_all(&dir)?;
    let name = format!(
        "TrontSnap_{}.mp4",
        chrono::Local::now().format("%Y-%m-%d_%H-%M-%S")
    );
    Ok(dir.join(name))
}

/// Per-frame cursor compositor. GetIconInfo allocates two bitmaps per call, so the
/// hotspot is cached per cursor handle instead of re-queried 30x a second.
#[derive(Default)]
struct CursorCache {
    handle: isize,
    hx: i32,
    hy: i32,
}

impl CursorCache {
    unsafe fn draw(&mut self, dc: HDC, origin_x: i32, origin_y: i32) {
        let mut ci = CURSORINFO {
            cbSize: std::mem::size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        if GetCursorInfo(&mut ci).is_err()
            || (ci.flags.0 & CURSOR_SHOWING.0) == 0
            || ci.hCursor.0 == 0
        {
            return;
        }
        let hicon = HICON(ci.hCursor.0);
        if ci.hCursor.0 != self.handle {
            let mut ii = ICONINFO::default();
            if GetIconInfo(hicon, &mut ii).is_ok() {
                if ii.hbmColor.0 != 0 {
                    let _ = DeleteObject(ii.hbmColor);
                }
                if ii.hbmMask.0 != 0 {
                    let _ = DeleteObject(ii.hbmMask);
                }
                self.handle = ci.hCursor.0;
                self.hx = ii.xHotspot as i32;
                self.hy = ii.yHotspot as i32;
            }
        }
        let x = ci.ptScreenPos.x - origin_x - self.hx;
        let y = ci.ptScreenPos.y - origin_y - self.hy;
        let _ = DrawIconEx(dc, x, y, hicon, 0, 0, 0, HBRUSH(0), DI_NORMAL);
    }
}

// ---------------------------------------------------------------------------------
// The "REC" pill: a tiny topmost, non-activating window near the region's top-right.
// Clicking it stops the recording. It closes itself when the session ends. It lives
// on its own thread because the recorder thread is busy pacing frames.

const PILL_W: i32 = 78;
const PILL_H: i32 = 30;
static PILL_BLINK: AtomicI32 = AtomicI32::new(0);

fn pill_thread(region: RECT) {
    unsafe {
        let hinstance = GetModuleHandleW(None).unwrap_or_default();
        let class = pill_class(hinstance.into());

        // Prefer just above the region (outside the recorded pixels); below if the
        // region touches the top edge; inside top-right as a last resort.
        let x = (region.right - PILL_W).max(0);
        let y = if region.top >= PILL_H + 6 {
            region.top - PILL_H - 4
        } else {
            region.bottom + 4
        };

        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            PCWSTR(class.as_ptr()),
            PCWSTR::null(),
            WS_POPUP | WS_VISIBLE,
            x,
            y,
            PILL_W,
            PILL_H,
            HWND(0),
            None,
            hinstance,
            None,
        );
        SetTimer(hwnd, 1, 400, None); // blink + session-end watchdog

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND(0), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn pill_class(hinstance: windows::Win32::Foundation::HINSTANCE) -> &'static Vec<u16> {
    static CLASS: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
    CLASS.get_or_init(|| {
        let name: Vec<u16> = "TrontSnapRecPill\0".encode_utf16().collect();
        unsafe {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(pill_proc),
                hInstance: hinstance,
                lpszClassName: PCWSTR(name.as_ptr()),
                hbrBackground: HBRUSH(0),
                ..Default::default()
            };
            RegisterClassW(&wc);
        }
        name
    })
}

unsafe extern "system" fn pill_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_LBUTTONUP => {
            STOP.store(true, Ordering::SeqCst);
            LRESULT(0)
        }
        WM_TIMER => {
            if !RECORDING.load(Ordering::SeqCst) {
                let _ = KillTimer(hwnd, 1);
                let _ = DestroyWindow(hwnd);
            } else {
                PILL_BLINK.fetch_add(1, Ordering::Relaxed);
                let _ = windows::Win32::Graphics::Gdi::InvalidateRect(hwnd, None, false);
            }
            LRESULT(0)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let bg = CreateSolidBrush(COLORREF(0x00100c0a)); // dark, BGR
            let full = RECT { left: 0, top: 0, right: PILL_W, bottom: PILL_H };
            FillRect(hdc, &full, bg);
            let _ = DeleteObject(bg);
            // Blinking red dot.
            if PILL_BLINK.load(Ordering::Relaxed) % 2 == 0 {
                let dot = CreateSolidBrush(COLORREF(0x002020e8)); // red-ish, BGR
                let r = RECT { left: 9, top: 10, right: 19, bottom: 20 };
                FillRect(hdc, &r, dot);
                let _ = DeleteObject(dot);
            }
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, COLORREF(0x00f5faff));
            let text: Vec<u16> = "REC".encode_utf16().collect();
            let _ = TextOutW(hdc, 26, 7, &text);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
