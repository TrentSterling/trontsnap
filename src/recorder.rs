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
use std::sync::Arc;
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
    BeginPaint, CombineRgn, CreateCompatibleDC, CreateDIBSection, CreateRectRgn, CreateSolidBrush,
    DeleteDC, DeleteObject, EndPaint, FillRect, GetDC, InvalidateRect, ReleaseDC, SelectObject,
    SetBkMode, SetTextColor, SetWindowRgn, TextOutW, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HBRUSH, HDC, PAINTSTRUCT, RGN_DIFF, RGN_OR, TRANSPARENT,
};
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFMediaBuffer, IMFMediaType, IMFSample, IMFSinkWriter, MFAudioFormat_AAC,
    MFAudioFormat_PCM, MFCreateAttributes, MFCreateMediaType, MFCreateMemoryBuffer,
    MFCreateSample, MFCreateSinkWriterFromURL, MFShutdown, MFStartup, MFVideoFormat_H264,
    MFVideoFormat_RGB32, MFVideoInterlace_Progressive, MFMediaType_Audio, MFMediaType_Video,
    MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT,
    MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_AVG_BITRATE,
    MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE,
    MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE,
    MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, DrawIconEx, GetCursorInfo,
    GetIconInfo, GetMessageW, GetSystemMetrics, GetWindowLongPtrW, KillTimer, LoadCursorW,
    PostQuitMessage, RegisterClassW, SetLayeredWindowAttributes, SetTimer,
    SetWindowDisplayAffinity, SetWindowLongPtrW, TranslateMessage, CURSORINFO, CURSOR_SHOWING, DI_NORMAL, GWLP_USERDATA, HICON, ICONINFO, IDC_ARROW, LWA_COLORKEY, MSG,
    SM_CYSCREEN,
    WDA_EXCLUDEFROMCAPTURE, WM_DESTROY, WM_LBUTTONUP, WM_PAINT, WM_TIMER, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
};

use crate::capture;
use crate::region_win32;
use crate::settings;

const FPS: u32 = 30;
// windows 0.56 doesn't re-export MF_VERSION; this is MF_SDK_VERSION << 16 | MF_API_VERSION.
// Shared with videothumb.rs (the Source Reader side).
pub(crate) const MF_VERSION: u32 = 0x0002_0070;

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

    // The recording HUD (blinking red outline + REC tab) lives on its own thread with
    // its own message loop; it watches RECORDING to know when to close, and clicking
    // its red parts sets STOP.
    let region = RECT { left: x, top: y, right: x + w as i32, bottom: y + h as i32 };
    std::thread::spawn(move || hud_thread(region));

    match capture_encode(x, y, w, h) {
        Ok(path) => {
            // Best-effort: the finished MP4 goes on the clipboard as a file (CF_HDROP),
            // so it pastes straight into Discord — same spirit as screenshots.
            if let Err(e) = crate::clipboard::set_file(&path) {
                eprintln!("trontsnap: recording clipboard set failed: {e:#}");
            }
            crate::sound::play_shutter();
            eprintln!("trontsnap: recorded {w}x{h} -> {}", path.display());
            crate::toast::launch(&path);
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

        // --- audio track: WASAPI loopback -> AAC (optional, best-effort) ---------
        // Shares the recording epoch so the block timestamps line up with video.
        // Any failure (odd device rate, no endpoint) logs and records video-only.
        let epoch = Instant::now();
        let audio_stop = Arc::new(AtomicBool::new(false));
        let mut audio: Option<(u32, crossbeam_channel::Receiver<crate::audio::AudioBlock>)> = None;
        if crate::settings::record_audio() {
            match crate::audio::start(epoch, audio_stop.clone()) {
                Ok((fmt, rx)) => {
                    let out_a: IMFMediaType = MFCreateMediaType()?;
                    out_a.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                    out_a.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
                    out_a.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, fmt.rate)?;
                    out_a.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, fmt.channels as u32)?;
                    out_a.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
                    // 24000 B/s = 192kbps, the top rate the MF AAC encoder accepts.
                    out_a.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24_000)?;
                    let a_stream = writer.AddStream(&out_a)?;

                    let in_a: IMFMediaType = MFCreateMediaType()?;
                    let block = 2 * fmt.channels as u32;
                    in_a.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                    in_a.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
                    in_a.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, fmt.rate)?;
                    in_a.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, fmt.channels as u32)?;
                    in_a.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
                    in_a.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, block)?;
                    in_a.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, fmt.rate * block)?;
                    writer.SetInputMediaType(a_stream, &in_a, None)?;
                    audio = Some((a_stream, rx));
                }
                Err(e) => eprintln!("trontsnap: recording without audio: {e:#}"),
            }
        }
        // Recorder threads must never outlive the session, whatever exit path runs.
        struct AudioStopGuard(Arc<AtomicBool>);
        impl Drop for AudioStopGuard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let _audio_guard = AudioStopGuard(audio_stop.clone());

        writer.BeginWriting()?;

        // --- 30fps CFR loop ------------------------------------------------------
        // Pace against wall clock; a static screen means AcquireNextFrame times out
        // and we re-encode the previous staging content (constant frame rate).
        let mut n: u64 = 0;
        let mut have_frame = false;
        let mut cursor_cache = CursorCache::default();

        while !STOP.load(Ordering::SeqCst) {
            let target = Duration::from_nanos(n * 1_000_000_000 / FPS as u64);
            if let Some(rest) = target.checked_sub(epoch.elapsed()) {
                std::thread::sleep(rest);
            }

            // Interleave any audio that arrived since the last tick (timestamps come
            // from the capture thread's sample clock, same epoch as the video).
            if let Some((a_stream, rx)) = &audio {
                for block in rx.try_iter() {
                    write_sample(&writer, *a_stream, &block.pcm, block.t100ns, block.dur100ns)?;
                }
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
            let frame = std::slice::from_raw_parts(dst_base as *const u8, frame_len);
            write_sample(
                &writer,
                stream,
                frame,
                (n as i64) * 10_000_000 / FPS as i64,
                10_000_000 / FPS as i64,
            )?;
            n += 1;
        }

        // Wind down audio: stop the capture thread, then flush whatever it already
        // produced so the track doesn't end a beat before the video.
        audio_stop.store(true, Ordering::SeqCst);
        if let Some((a_stream, rx)) = &audio {
            std::thread::sleep(Duration::from_millis(40));
            for block in rx.try_iter() {
                write_sample(&writer, *a_stream, &block.pcm, block.t100ns, block.dur100ns)?;
            }
        }

        writer.Finalize()?;
        if n == 0 {
            let _ = std::fs::remove_file(&path);
            anyhow::bail!("no frames captured");
        }
        Ok(path)
    }
}

/// Wrap `data` in an IMFSample with the given timestamp/duration (100ns units) and
/// hand it to the writer. Used for both video frames and PCM audio blocks.
unsafe fn write_sample(
    writer: &IMFSinkWriter,
    stream: u32,
    data: &[u8],
    t100ns: i64,
    dur100ns: i64,
) -> anyhow::Result<()> {
    let buffer: IMFMediaBuffer = MFCreateMemoryBuffer(data.len() as u32)?;
    let mut ptr: *mut u8 = std::ptr::null_mut();
    buffer.Lock(&mut ptr, None, None)?;
    std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
    buffer.Unlock()?;
    buffer.SetCurrentLength(data.len() as u32)?;
    let sample: IMFSample = MFCreateSample()?;
    sample.AddBuffer(&buffer)?;
    sample.SetSampleTime(t100ns)?;
    sample.SetSampleDuration(dur100ns)?;
    writer.WriteSample(stream, &sample)?;
    Ok(())
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
// The recording HUD: ONE topmost window drawing a blinking red outline around the
// region plus an attached "● REC" tab. Its WINDOW REGION (SetWindowRgn) is exactly
// the frame ring + the tab, so the interior is not merely transparent — it is NOT
// PART OF THE WINDOW. That matters for hit-testing: Windows routes hover and the
// "scroll the window under the cursor" wheel by hit-test, and a layered color-key
// window still hit-tests as solid over its keyed pixels on Win11 (found live: the
// HUD ate scroll/hover over the recorded area). With a region there is no surface
// over the interior at all. The red frame and the tab ARE clickable — click to stop.
//
// The whole window is marked WDA_EXCLUDEFROMCAPTURE (the OBS trick), so the outline is
// visible on the monitor but NEVER appears in the recorded MP4 (DXGI duplication skips
// excluded windows) — which is also why the tab can sit inside the region when a
// fullscreen record leaves no room outside.

const FRAME_PX: i32 = 3; // outline thickness, drawn just OUTSIDE the recorded region
const TAB_W: i32 = 86;
const TAB_H: i32 = 28;
/// Improbable color used as the transparency key — everything painted in it is
/// invisible AND click-through.
const KEY: COLORREF = COLORREF(0x0003_0201);

static HUD_BLINK: AtomicI32 = AtomicI32::new(0);

/// Window-local geometry, reached from the proc via GWLP_USERDATA (same pattern as
/// the region picker; the HUD is modal to this thread so a stack ref is safe).
struct HudGeo {
    outline: RECT, // the region border, window-local
    tab: RECT,     // the REC tab, window-local
}

fn hud_thread(region: RECT) {
    unsafe {
        let hinstance = GetModuleHandleW(None).unwrap_or_default();
        let class = hud_class(hinstance.into());

        // Outline sits just outside the region. Tab: above the top-right corner if
        // there's room, below the bottom-right otherwise, inside as a last resort
        // (fullscreen records — invisible to the capture anyway).
        let out = RECT {
            left: region.left - FRAME_PX,
            top: region.top - FRAME_PX,
            right: region.right + FRAME_PX,
            bottom: region.bottom + FRAME_PX,
        };
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        let tab_left = (out.right - TAB_W).max(0);
        let tab = if out.top - TAB_H - 2 >= 0 {
            RECT { left: tab_left, top: out.top - TAB_H - 2, right: tab_left + TAB_W, bottom: out.top - 2 }
        } else if out.bottom + TAB_H + 2 <= screen_h {
            RECT { left: tab_left, top: out.bottom + 2, right: tab_left + TAB_W, bottom: out.bottom + TAB_H + 2 }
        } else {
            let t = region.top + 6;
            RECT { left: region.right - 6 - TAB_W, top: t, right: region.right - 6, bottom: t + TAB_H }
        };

        // Window = union of outline and tab, in screen px.
        let wx = out.left.min(tab.left);
        let wy = out.top.min(tab.top);
        let ww = out.right.max(tab.right) - wx;
        let wh = out.bottom.max(tab.bottom) - wy;
        let mut geo = HudGeo {
            outline: RECT {
                left: out.left - wx,
                top: out.top - wy,
                right: out.right - wx,
                bottom: out.bottom - wy,
            },
            tab: RECT {
                left: tab.left - wx,
                top: tab.top - wy,
                right: tab.right - wx,
                bottom: tab.bottom - wy,
            },
        };

        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
            PCWSTR(class.as_ptr()),
            PCWSTR::null(),
            WS_POPUP | WS_VISIBLE,
            wx,
            wy,
            ww,
            wh,
            HWND(0),
            None,
            hinstance,
            None,
        );
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, &mut geo as *mut HudGeo as isize);
        // Key-colored pixels render transparent (visual only — see the region below
        // for why hit-testing can't rely on this).
        let _ = SetLayeredWindowAttributes(hwnd, KEY, 0, LWA_COLORKEY);
        // Shape the window to frame-ring + tab so the interior structurally does not
        // exist: mouse hover / wheel / clicks inside the recorded area go straight to
        // the app being recorded. SetWindowRgn takes ownership of `shape`; the
        // temporaries must be deleted by us.
        let shape = CreateRectRgn(
            geo.outline.left,
            geo.outline.top,
            geo.outline.right,
            geo.outline.bottom,
        );
        let inner = CreateRectRgn(
            geo.outline.left + FRAME_PX,
            geo.outline.top + FRAME_PX,
            geo.outline.right - FRAME_PX,
            geo.outline.bottom - FRAME_PX,
        );
        let tab_rgn = CreateRectRgn(geo.tab.left, geo.tab.top, geo.tab.right, geo.tab.bottom);
        CombineRgn(shape, shape, inner, RGN_DIFF);
        CombineRgn(shape, shape, tab_rgn, RGN_OR);
        let _ = DeleteObject(inner);
        let _ = DeleteObject(tab_rgn);
        SetWindowRgn(hwnd, shape, true);
        // Exclude the HUD from ALL capture (our DXGI recording and screenshots both):
        // on failure the HUD still works, it just shows up in the video.
        if SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE).is_err() {
            eprintln!("trontsnap: HUD capture-exclusion unavailable (will appear in the video)");
        }
        SetTimer(hwnd, 1, 500, None); // blink + session-end watchdog
        let _ = InvalidateRect(hwnd, None, true);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND(0), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn hud_class(hinstance: windows::Win32::Foundation::HINSTANCE) -> &'static Vec<u16> {
    static CLASS: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
    CLASS.get_or_init(|| {
        let name: Vec<u16> = "TrontSnapRecHud\0".encode_utf16().collect();
        unsafe {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(hud_proc),
                hInstance: hinstance,
                lpszClassName: PCWSTR(name.as_ptr()),
                // Class background = the key color, so every un-painted pixel is
                // transparent by default and erase never flashes a solid rect.
                hbrBackground: CreateSolidBrush(KEY),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                ..Default::default()
            };
            RegisterClassW(&wc);
        }
        name
    })
}

unsafe extern "system" fn hud_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_LBUTTONUP => {
            // Only reachable via the red frame or the tab (key pixels are click-through).
            STOP.store(true, Ordering::SeqCst);
            LRESULT(0)
        }
        WM_TIMER => {
            if !RECORDING.load(Ordering::SeqCst) {
                let _ = KillTimer(hwnd, 1);
                let _ = DestroyWindow(hwnd);
            } else {
                HUD_BLINK.fetch_add(1, Ordering::Relaxed);
                let _ = InvalidateRect(hwnd, None, true);
            }
            LRESULT(0)
        }
        WM_PAINT => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const HudGeo;
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            if !ptr.is_null() {
                paint_hud(hdc, &*ptr);
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::PCWSTR;

    /// Headless A/V mux smoke: verifies the EXACT SinkWriter configuration the
    /// recorder uses (H.264 video out/RGB32 in + AAC audio out/PCM in) is accepted,
    /// interleaves, finalizes, and reads back with BOTH tracks — no screen grab, no
    /// audio device. Run manually: cargo test av_mux_smoke -- --ignored
    #[test]
    #[ignore]
    fn av_mux_smoke() {
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, 0).unwrap();

            let path = std::env::temp_dir().join("trontsnap-avmux-smoke.mp4");
            let wide: Vec<u16> = path.to_string_lossy().encode_utf16().chain([0]).collect();
            let writer: IMFSinkWriter =
                MFCreateSinkWriterFromURL(PCWSTR(wide.as_ptr()), None, None).unwrap();

            let (w, h, fps) = (64u32, 64u32, 30u32);
            let out_v: IMFMediaType = MFCreateMediaType().unwrap();
            out_v.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).unwrap();
            out_v.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).unwrap();
            out_v.SetUINT32(&MF_MT_AVG_BITRATE, 1_000_000).unwrap();
            out_v.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64).unwrap();
            out_v.SetUINT64(&MF_MT_FRAME_RATE, ((fps as u64) << 32) | 1).unwrap();
            out_v.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1).unwrap();
            out_v
                .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .unwrap();
            let vs = writer.AddStream(&out_v).unwrap();
            let in_v: IMFMediaType = MFCreateMediaType().unwrap();
            in_v.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).unwrap();
            in_v.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).unwrap();
            in_v.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64).unwrap();
            in_v.SetUINT64(&MF_MT_FRAME_RATE, ((fps as u64) << 32) | 1).unwrap();
            in_v.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1).unwrap();
            in_v.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .unwrap();
            in_v.SetUINT32(&MF_MT_DEFAULT_STRIDE, w * 4).unwrap();
            writer.SetInputMediaType(vs, &in_v, None).unwrap();

            let (rate, ch) = (48_000u32, 2u32);
            let out_a: IMFMediaType = MFCreateMediaType().unwrap();
            out_a.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).unwrap();
            out_a.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC).unwrap();
            out_a.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate).unwrap();
            out_a.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, ch).unwrap();
            out_a.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).unwrap();
            out_a.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24_000).unwrap();
            let astream = writer.AddStream(&out_a).unwrap();
            let in_a: IMFMediaType = MFCreateMediaType().unwrap();
            in_a.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).unwrap();
            in_a.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM).unwrap();
            in_a.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate).unwrap();
            in_a.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, ch).unwrap();
            in_a.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).unwrap();
            in_a.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, ch * 2).unwrap();
            in_a.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, rate * ch * 2).unwrap();
            writer.SetInputMediaType(astream, &in_a, None).unwrap();

            writer.BeginWriting().unwrap();
            // 0.5s of black video + 0.5s of 440Hz sine.
            let frame = vec![0u8; (w * h * 4) as usize];
            let samples = (rate / 2) as usize;
            let mut sine = vec![0u8; samples * ch as usize * 2];
            for i in 0..samples {
                let v = ((i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin()
                    * 8000.0) as i16;
                let b = v.to_le_bytes();
                for c in 0..ch as usize {
                    let o = (i * ch as usize + c) * 2;
                    sine[o..o + 2].copy_from_slice(&b);
                }
            }
            for n in 0..15i64 {
                write_sample(&writer, vs, &frame, n * 10_000_000 / fps as i64, 10_000_000 / fps as i64)
                    .unwrap();
            }
            write_sample(&writer, astream, &sine, 0, 5_000_000).unwrap();
            writer.Finalize().unwrap();

            // Read back: the video track decodes, and an audio track exists.
            let img = crate::videothumb::first_frame(&path).expect("video track readable");
            assert_eq!((img.width(), img.height()), (w, h));
            let reader = MFCreateSinkReaderCheck(&wide);
            reader
                .GetCurrentMediaType(0xFFFF_FFFD) // MF_SOURCE_READER_FIRST_AUDIO_STREAM
                .expect("audio track present");
            drop(reader);
            let _ = std::fs::remove_file(&path);
        }
    }

    #[allow(non_snake_case)]
    unsafe fn MFCreateSinkReaderCheck(
        wide: &[u16],
    ) -> windows::Win32::Media::MediaFoundation::IMFSourceReader {
        windows::Win32::Media::MediaFoundation::MFCreateSourceReaderFromURL(
            PCWSTR(wide.as_ptr()),
            None,
        )
        .unwrap()
    }
}

unsafe fn paint_hud(hdc: HDC, geo: &HudGeo) {
    // Flash the outline bright <-> dim red (never fully off, so the recorded area
    // always reads at a glance).
    let bright = HUD_BLINK.load(Ordering::Relaxed) % 2 == 0;
    let red = if bright { COLORREF(0x0040_30ff) } else { COLORREF(0x0028_2080) }; // BGR
    let frame = CreateSolidBrush(red);
    let o = geo.outline;
    let strips = [
        RECT { left: o.left, top: o.top, right: o.right, bottom: o.top + FRAME_PX },
        RECT { left: o.left, top: o.bottom - FRAME_PX, right: o.right, bottom: o.bottom },
        RECT { left: o.left, top: o.top, right: o.left + FRAME_PX, bottom: o.bottom },
        RECT { left: o.right - FRAME_PX, top: o.top, right: o.right, bottom: o.bottom },
    ];
    for s in &strips {
        FillRect(hdc, s, frame);
    }
    let _ = DeleteObject(frame);

    // The REC tab: dark plate, blinking red dot, label.
    let t = geo.tab;
    let plate = CreateSolidBrush(COLORREF(0x0010_0c0a));
    FillRect(hdc, &t, plate);
    let _ = DeleteObject(plate);
    if bright {
        let dot = CreateSolidBrush(COLORREF(0x0020_20e8));
        let r = RECT { left: t.left + 10, top: t.top + 9, right: t.left + 20, bottom: t.top + 19 };
        FillRect(hdc, &r, dot);
        let _ = DeleteObject(dot);
    }
    SetBkMode(hdc, TRANSPARENT);
    SetTextColor(hdc, COLORREF(0x00f5_faff));
    let text: Vec<u16> = "REC · stop".encode_utf16().collect();
    let _ = TextOutW(hdc, t.left + 26, t.top + 6, &text);
}
