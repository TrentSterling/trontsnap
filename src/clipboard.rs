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
/// Append a line to `%LOCALAPPDATA%\TrontSnap\trontsnap.log`.
///
/// Release builds are windows-subsystem, so `eprintln!` goes to nowhere: a
/// clipboard failure left literally no trace anywhere on the machine, which is
/// why "it just didn't copy" was impossible to chase. Failures are rare, the file
/// is tiny, and having one real error message beats another round of guessing.
pub fn log_line(msg: &str) {
    let Ok(base) = std::env::var("LOCALAPPDATA") else { return };
    let dir = std::path::Path::new(&base).join("TrontSnap");
    let _ = std::fs::create_dir_all(&dir);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("trontsnap.log"))
    {
        let _ = writeln!(f, "[{stamp}] {msg}");
    }
}

/// Everything needed to perform one clipboard write, built once so a retry
/// costs nothing but the Win32 calls.
enum Payload {
    All { dib: Vec<u8>, dibv5: Vec<u8>, png: Vec<u8>, dropfiles: Vec<u8> },
    File { dropfiles: Vec<u8> },
}

impl Payload {
    unsafe fn write(&self) -> anyhow::Result<()> {
        match self {
            Payload::All { dib, dibv5, png, dropfiles } => {
                set_all_formats(dib, dibv5, png, dropfiles)
            }
            Payload::File { dropfiles } => {
                EmptyClipboard().map_err(win_err)?;
                let h = alloc_global_copy(dropfiles)?;
                SetClipboardData(CF_HDROP.0 as u32, HANDLE(h.0 as isize)).map_err(win_err)?;
                Ok(())
            }
        }
    }
}

/// The one outstanding write, plus the file it came from (for the late-success
/// toast). NEWEST WINS: a fresh capture replaces a pending one rather than
/// queueing behind it, because nobody wants an old screenshot to land on their
/// clipboard after they have already taken another.
static PENDING: std::sync::Mutex<Option<(Payload, std::path::PathBuf)>> =
    std::sync::Mutex::new(None);
static PENDING_WAKE: std::sync::Condvar = std::sync::Condvar::new();

/// How long the background retrier keeps trying before giving up on a payload.
/// Generous on purpose: the whole point is that a clipboard held by a browser,
/// a terminal with an active selection, or a remote-desktop session is a
/// TRANSIENT condition, and the capture is already safely on disk meanwhile.
const RETRY_BUDGET: Duration = Duration::from_secs(90);

/// Hand a payload to the background retrier, replacing whatever was pending.
fn enqueue(payload: Payload, path: &Path) {
    start_retry_thread();
    if let Ok(mut slot) = PENDING.lock() {
        let superseded = slot.is_some();
        *slot = Some((payload, path.to_path_buf()));
        if superseded {
            log_line("clipboard: newer capture superseded the pending retry");
        }
    }
    PENDING_WAKE.notify_all();
}

/// Background retrier. Started once, on first need.
///
/// WHY THIS EXISTS: the synchronous attempt gives up after about a second, and
/// when it lost, the capture was on disk but never on the clipboard, with the
/// only feedback being a toast saying so. That is the "CLIPBOARD WAS BUSY"
/// failure, and it is infuriating precisely because the condition clears on its
/// own a moment later. Now the write is queued and keeps trying.
fn start_retry_thread() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        log_line("clipboard: writer thread starting");
        std::thread::spawn(|| {
        // Created HERE so it belongs to this thread, and never handed out. Every
        // OpenClipboard in the process happens on this thread with this window.
        let owner = unsafe { create_owner_window() };
        log_line(&format!("clipboard: writer owns hwnd=0x{:X}", owner.0));
        loop {
            // Wait for work.
            let (payload, path) = {
                let Ok(mut slot) = PENDING.lock() else { return };
                while slot.is_none() {
                    let Ok(next) = PENDING_WAKE.wait(slot) else { return };
                    slot = next;
                }
                slot.take().expect("checked above")
            };

            let started = std::time::Instant::now();
            let mut attempt = 0u32;
            loop {
                // A newer capture replaces this one; abandon it immediately.
                if PENDING.lock().map(|s| s.is_some()).unwrap_or(false) {
                    break;
                }
                let ok = unsafe {
                    if OpenClipboard(owner).is_ok() {
                        let r = payload.write();
                        let _ = CloseClipboard();
                        r.is_ok()
                    } else {
                        false
                    }
                };
                if ok {
                    if attempt > 0 {
                        log_line(&format!(
                            "clipboard: landed after {} retries ({:.1}s)",
                            attempt,
                            started.elapsed().as_secs_f32()
                        ));
                    }
                    let _ = &path;
                    break;
                }
                if started.elapsed() >= RETRY_BUDGET {
                    log_line(&format!(
                        "clipboard: gave up after {attempt} retries; the file is still on disk"
                    ));
                    break;
                }
                attempt += 1;
                if attempt % 10 == 0 {
                    log_line(&format!("clipboard: retry {attempt} still blocked"));
                }
                // Ramp 50ms -> 500ms. Fast enough to catch a browser letting go
                // within a frame or two, slow enough to be free while a terminal
                // sits on a selection for a minute.
                let backoff = (50 * (attempt.min(10) as u64)).min(500);
                std::thread::sleep(Duration::from_millis(backoff));
            }
        }
        });
    });
}

pub fn set_all(img: &RgbaImage, png_bytes: &[u8], file_path: &Path) -> anyhow::Result<()> {
    let (w, h) = (img.width(), img.height());
    let bgra = rgba_to_bgra_bottom_up(w, h, img.as_raw());
    let payload = Payload::All {
        dib: build_dib(w, h, &bgra),
        dibv5: build_dibv5(w, h, &bgra),
        png: png_bytes.to_vec(),
        dropfiles: build_dropfiles(file_path),
    };

    // ALWAYS go through the writer thread; never touch the clipboard from here.
    // This function runs on a fresh thread per capture, and the clipboard owner
    // window belongs to exactly one thread, so a write attempted here would fail.
    enqueue(payload, file_path);
    Ok(())
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

/// Put just a FILE on the clipboard (CF_HDROP only) — used for video recordings,
/// which have no pixel formats. Pasting still works everywhere that accepts a
/// dropped/pasted file (Discord, Explorer, terminals). Same atomic session +
/// open-retry as `set_all`.
pub fn set_file(file_path: &Path) -> anyhow::Result<()> {
    enqueue(Payload::File { dropfiles: build_dropfiles(file_path) }, file_path);
    Ok(())
}

/// OpenClipboard can transiently fail if another app (browsers are the usual
/// culprit) has it open; retry briefly instead of failing the whole capture.
/// A message-only window to OWN the clipboard, created once per process.
///
/// THE TRAP: `OpenClipboard(HWND(0))` leaves the clipboard OWNER null, and
/// Microsoft documents that this makes `EmptyClipboard` null the owner and
/// `SetClipboardData` unreliable. The write can report success and the contents
/// still evaporate, because they are tied to a thread rather than a window — and
/// the one-shot capture modes (`trontsnap full` / `region`) exit within seconds,
/// while the gallery's Copy runs on a thread that exits immediately. Either way
/// the data was gone before it could be pasted.
///
/// Owning the clipboard with a real (hidden, message-only) window fixes that: the
/// window outlives the operation and the OS has a valid owner to point at.
/// Create the message-only window that owns the clipboard.
///
/// MUST be called on the thread that will call OpenClipboard with it, and that
/// handle must not be shared with other threads. A window belongs to the thread
/// that created it, and OpenClipboard against another thread's window fails.
///
/// This used to be a process-wide `OnceLock<isize>`, created by whichever thread
/// captured first and then reused everywhere. `do_full` spawns a fresh thread per
/// capture, so every capture after the first passed a foreign window and
/// OpenClipboard refused it. The error surfaces as "could not open the clipboard
/// (another app is holding it)", which sent us hunting for a culprit that did not
/// exist: measured 2026-07-27 with `GetOpenClipboardWindow()` returning NULL
/// while 185 consecutive retries all failed.
///
/// Hence the single writer thread below: one window, one thread, all writes.
unsafe fn create_owner_window() -> HWND {
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE,
    };
    HWND({
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!("TrontSnapClipboard"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            None,
            None,
            None,
        )
        .0
    })
}

// NOTE: the old `open_clipboard_with_retry` lived here. It is gone because
// retrying on the CALLING thread could never work: the owner window belongs to
// the writer thread, so a caller retrying 28 times just failed 28 times. The
// retry now lives in the writer thread's loop, where the window is valid.

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
