// Screen grab + delivery (clipboard + disk).
// Capture is via xcap. Clipboard is our own multi-format writer (see clipboard.rs)
// so screenshots paste everywhere ShareX does (Windows: CF_HDROP also pastes a path
// into terminals; Linux: the clip-serve target set, see LINUX.md).

use std::path::{Path, PathBuf};

use image::RgbaImage;
use xcap::Monitor;

/// Grab the primary monitor as an RGBA image (physical pixels).
/// Primary-monitor only for now; multi-monitor virtual-desktop capture is next.
pub fn grab_primary() -> anyhow::Result<RgbaImage> {
    match grab_xcap() {
        Ok(img) => Ok(img),
        #[cfg(windows)]
        Err(e) => Err(e),
        // Wayland compositors won't all talk to xcap (GNOME's Mutter in
        // particular locks its shell screenshot API down); fall through to the
        // desktop's own CLI grabbers before giving up.
        #[cfg(not(windows))]
        Err(e) => grab_linux_fallback(Some(&e), false),
    }
}

/// `grab_primary`, drawing the pointer when "Capture cursor" is on. Windows composites it afterwards
/// (cursor::maybe_overlay), and so does an X11 session (XFixes). On Wayland nothing but the
/// compositor's own grabber can see the pointer, so there the CLI grabber goes FIRST (xcap can't
/// draw it) and xcap is only the fallback.
#[cfg(not(windows))]
fn grab_primary_pointer() -> anyhow::Result<RgbaImage> {
    if crate::settings::capture_cursor() && crate::cursor::wayland_session() {
        if let Ok(img) = grab_linux_fallback(None, true) {
            return Ok(img);
        }
    }
    grab_primary()
}
#[cfg(windows)]
fn grab_primary_pointer() -> anyhow::Result<RgbaImage> {
    grab_primary()
}

fn grab_xcap() -> anyhow::Result<RgbaImage> {
    let monitors = Monitor::all()?;
    let monitor = monitors
        .iter()
        .find(|m| m.is_primary())
        .or_else(|| monitors.first())
        .ok_or_else(|| anyhow::anyhow!("no monitors found"))?;
    Ok(monitor.capture_image()?)
}

/// A grab takes 1.5-8 s here depending on load; past this the grabber is stuck (portal activation
/// sat for over a minute on a broken bus, and the hotkey just hung with no sign of why).
#[cfg(not(windows))]
const GRAB_TIMEOUT_SECS: u64 = 20;

/// Run `cmd` to completion and say whether it succeeded; kill it when it outlives `secs`.
#[cfg(not(windows))]
fn run_with_timeout(cmd: &mut std::process::Command, secs: u64) -> bool {
    let Ok(mut child) = cmd.spawn() else { return false };
    let t0 = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => return st.success(),
            Ok(None) if t0.elapsed().as_secs() < secs => std::thread::sleep(std::time::Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// Run the first CLI grabber that works and return its PNG bytes. gnome-screenshot
/// covers GNOME Wayland (portal path, no prompt), spectacle covers KDE, grim
/// covers wlroots. Each writes a PNG to a temp path we read back.
#[cfg(not(windows))]
fn run_cli_grabber(xcap_err: Option<&anyhow::Error>, pointer: bool) -> anyhow::Result<Vec<u8>> {
    let dir = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir);
    let tmp = dir.join(format!("trontsnap-grab-{}.png", std::process::id()));
    let tmp_s = tmp.display().to_string();
    // HARD-WON: "Capture cursor" did nothing for screenshots on Linux (cursor::maybe_overlay is a
    // no-op and a Wayland client can't read the pointer), while the Settings text promised it.
    // The grabbers can draw the pointer themselves, so ask them to.
    let attempts: [(&str, Vec<&str>); 3] = if pointer {
        [
            ("gnome-screenshot", vec!["-p", "-f", &tmp_s]),
            ("spectacle", vec!["-p", "-b", "-n", "-o", &tmp_s]),
            ("grim", vec!["-c", &tmp_s]),
        ]
    } else {
        [
            ("gnome-screenshot", vec!["-f", &tmp_s]),
            ("spectacle", vec!["-b", "-n", "-o", &tmp_s]),
            ("grim", vec![&tmp_s]),
        ]
    };
    for (cmd, args) in attempts {
        let mut c = std::process::Command::new(cmd);
        c.args(&args).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        let ok = run_with_timeout(&mut c, GRAB_TIMEOUT_SECS);
        if ok && tmp.exists() {
            let bytes = std::fs::read(&tmp);
            let _ = std::fs::remove_file(&tmp);
            return Ok(bytes?);
        }
    }
    let xcap = xcap_err.map_or_else(|| "not tried".to_string(), |e| format!("{e:#}"));
    anyhow::bail!("no screen grabber worked (xcap: {xcap}); install gnome-screenshot, spectacle or grim")
}

/// The primary monitor's rect inside a multi-monitor grab, or None when the grab
/// is already exactly the primary (or the monitor list isn't available). These
/// tools shoot ALL monitors; everything downstream (picker geometry, cursor math,
/// smart rects) assumes the primary.
#[cfg(not(windows))]
fn primary_crop_rect(grab_w: u32, grab_h: u32) -> Option<(u32, u32, u32, u32)> {
    let monitors = Monitor::all().ok()?;
    let m = monitors.iter().find(|m| m.is_primary()).or(monitors.first())?;
    let (x, y) = (m.x().max(0) as u32, m.y().max(0) as u32);
    let (w, h) = (m.width(), m.height());
    (x + w <= grab_w && y + h <= grab_h && (w, h) != (grab_w, grab_h)).then_some((x, y, w, h))
}

/// CLI grabber -> decoded primary-monitor image (region picker, anything that
/// needs pixels).
#[cfg(not(windows))]
fn grab_linux_fallback(xcap_err: Option<&anyhow::Error>, pointer: bool) -> anyhow::Result<RgbaImage> {
    let bytes = run_cli_grabber(xcap_err, pointer)?;
    let mut img = image::load_from_memory(&bytes)?.to_rgba8();
    if let Some((x, y, w, h)) = primary_crop_rect(img.width(), img.height()) {
        img = image::imageops::crop_imm(&img, x, y, w, h).to_image();
    }
    Ok(img)
}

/// Width/height out of a PNG's IHDR without decoding (sig 8 + len 4 + "IHDR" 4 + w + h).
#[cfg(not(windows))]
fn png_dims(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 24 || b[..8] != *b"\x89PNG\r\n\x1a\n" || b[12..16] != *b"IHDR" {
        return None;
    }
    Some((u32::from_be_bytes(b[16..20].try_into().ok()?), u32::from_be_bytes(b[20..24].try_into().ok()?)))
}

/// Full-screen grab as finished PNG bytes. HARD-WON: when gnome-screenshot's PNG
/// already is the primary monitor (single-screen box, the common case) its bytes
/// go to the clipboard and disk untouched; the old decode -> overlay -> re-encode
/// round trip was ~3 s of a ~4.8 s capture in the debug build the hotkeys run,
/// and the clipboard is only set at the end, so a quick paste got the previous
/// one. A grabber's own PNG already has the pointer when it was asked for it; only the xcap path
/// (X11) needs the XFixes overlay, and that image is encoded here anyway.
#[cfg(not(windows))]
fn grab_png_linux() -> anyhow::Result<Vec<u8>> {
    // On Wayland xcap can't draw the pointer: with it wanted, the CLI grabber goes first.
    let pointer = crate::settings::capture_cursor() && crate::cursor::wayland_session();
    let xcap_err = if pointer {
        None
    } else {
        match grab_xcap() {
            Ok(mut img) => {
                crate::cursor::maybe_overlay(&mut img, (0, 0));
                return encode_png(&img);
            }
            Err(e) => Some(e),
        }
    };
    // (CLI-grabbed shots on an X11 session, e.g. xcap failing: the pointer flag is passed only on
    // Wayland, see `pointer`; there the grabber's output is used as is.)
    let bytes = match run_cli_grabber(xcap_err.as_ref(), pointer) {
        Ok(b) => b,
        // Pointer wanted but no grabber ran: a pointer-less shot beats none.
        Err(_) if pointer => return encode_png(&grab_xcap()?),
        Err(e) => return Err(e),
    };
    let needs_crop = png_dims(&bytes).map_or(true, |(w, h)| primary_crop_rect(w, h).is_some());
    if !needs_crop {
        return Ok(bytes);
    }
    let mut img = image::load_from_memory(&bytes)?.to_rgba8();
    if let Some((x, y, w, h)) = primary_crop_rect(img.width(), img.height()) {
        img = image::imageops::crop_imm(&img, x, y, w, h).to_image();
    }
    encode_png(&img)
}

/// Grab the primary monitor for a screenshot, compositing in the mouse cursor when the
/// "capture cursor" setting is on. Every capture path (full, region, one-shot CLI) goes
/// through this; `grab_primary` stays pointer-free for callers that don't want it.
pub fn grab_for_shot() -> anyhow::Result<RgbaImage> {
    let mut img = grab_primary_pointer()?;
    crate::cursor::maybe_overlay(&mut img, (0, 0));
    Ok(img)
}

/// Full-screen capture path: grab primary, deliver.
#[cfg(windows)]
pub fn capture_full() -> anyhow::Result<()> {
    let img = grab_for_shot()?;
    let path = deliver(&img)?;
    println!("captured {}x{} -> clipboard + {}", img.width(), img.height(), path.display());
    Ok(())
}

/// Full-screen capture path, Linux: PNG bytes straight through (see grab_png_linux).
#[cfg(not(windows))]
pub fn capture_full() -> anyhow::Result<()> {
    let png_bytes = grab_png_linux()?;
    let (w, h) = png_dims(&png_bytes).unwrap_or((0, 0));
    let path = save_png_bytes(&png_bytes)?;
    finish_delivery(&path, crate::clipboard::set_png(&png_bytes, &path));
    println!("captured {w}x{h} -> clipboard + {}", path.display());
    Ok(())
}

/// Encode, save, put on the clipboard (CF_DIB+CF_DIBV5+"PNG"+CF_HDROP), and play
/// the shutter. Saves to disk BEFORE touching the clipboard so CF_HDROP never
/// points at a file that doesn't exist yet; the clipboard write is best-effort
/// (logged, not fatal) so a transient clipboard failure never loses the shot.
pub fn deliver(img: &RgbaImage) -> anyhow::Result<PathBuf> {
    let png_bytes = encode_png(img)?;
    let path = save_png_bytes(&png_bytes)?;
    finish_delivery(&path, crate::clipboard::set_all(img, &png_bytes, &path));
    Ok(path)
}

/// Shared tail of every capture: report the clipboard outcome, shutter, toast.
fn finish_delivery(path: &Path, clipboard: anyhow::Result<()>) {
    // Best-effort, but NOT silent: the toast reports the real outcome. Previously
    // this only logged to stderr, which goes nowhere in a windowed exe, so a lost
    // clipboard race looked identical to success.
    let clipboard_ok = match clipboard {
        Ok(()) => true,
        Err(e) => {
            eprintln!("trontsnap: clipboard set failed: {e:#}");
            crate::clipboard::log_line(&format!("clipboard set_all failed: {e:#}"));
            false
        }
    };
    crate::sound::play_shutter();
    // Corner toast (ShareX-style) as its own tiny process, so it never touches the main
    // app's window/loop. A plain std::process::Command spawn (inside toast::launch) is all
    // it takes now that TrontSnap is a portable, non-uiAccess, Medium-integrity exe.
    crate::toast::launch(path, clipboard_ok);
}

/// Load a PNG/JPG off disk and put it on the clipboard (used by the gallery).
pub fn copy_path(path: &Path) -> anyhow::Result<()> {
    let img = image::open(path)?.to_rgba8();
    let is_png = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("png"));
    let png_bytes = if is_png {
        std::fs::read(path)? // byte-identical to what's on disk
    } else {
        encode_png(&img)? // e.g. ShareX .jpg archive entries: re-encode for the "PNG" format
    };
    crate::clipboard::set_all(&img, &png_bytes, path)
}

fn encode_png(img: &RgbaImage) -> anyhow::Result<Vec<u8>> {
    use image::ImageEncoder;
    let mut buf = Vec::new();
    image::codecs::png::PngEncoder::new(&mut buf).write_image(
        img.as_raw(),
        img.width(),
        img.height(),
        image::ColorType::Rgba8,
    )?;
    Ok(buf)
}

fn save_png_bytes(bytes: &[u8]) -> anyhow::Result<PathBuf> {
    let dir = trontsnap_dir()?;
    std::fs::create_dir_all(&dir)?;
    let name = format!(
        "TrontSnap_{}.png",
        chrono::Local::now().format("%Y-%m-%d_%H-%M-%S")
    );
    let path = dir.join(name);
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// Where new captures land: `Pictures\TrontSnap`.
pub fn trontsnap_dir() -> anyhow::Result<PathBuf> {
    let base = dirs::picture_dir()
        .or_else(dirs::home_dir)
        .ok_or_else(|| anyhow::anyhow!("could not resolve a save directory"))?;
    Ok(base.join("TrontSnap"))
}

/// The legacy ShareX archive, browsed read-only in the gallery timeline.
pub fn sharex_dir() -> Option<PathBuf> {
    dirs::document_dir().map(|d| d.join("ShareX").join("Screenshots"))
}
