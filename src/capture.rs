// Screen grab + delivery (clipboard + disk).
// Capture is via xcap. Clipboard is our own multi-format writer (see clipboard.rs)
// so screenshots paste everywhere ShareX does, including as a file into terminals.

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
        Err(e) => grab_linux_fallback(&e),
    }
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

/// CLI grabber chain for desktops xcap can't capture: each writes a PNG to a
/// temp path we then load. gnome-screenshot covers GNOME Wayland (portal path,
/// no prompt), spectacle covers KDE, grim covers wlroots.
#[cfg(not(windows))]
fn grab_linux_fallback(xcap_err: &anyhow::Error) -> anyhow::Result<RgbaImage> {
    let dir = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir);
    let tmp = dir.join(format!("trontsnap-grab-{}.png", std::process::id()));
    let tmp_s = tmp.display().to_string();
    let attempts: [(&str, Vec<&str>); 3] = [
        ("gnome-screenshot", vec!["-f", &tmp_s]),
        ("spectacle", vec!["-b", "-n", "-o", &tmp_s]),
        ("grim", vec![&tmp_s]),
    ];
    for (cmd, args) in attempts {
        let ok = std::process::Command::new(cmd)
            .args(&args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok && tmp.exists() {
            let mut img = image::open(&tmp)?.to_rgba8();
            let _ = std::fs::remove_file(&tmp);
            // These tools shoot ALL monitors; everything downstream (picker
            // geometry, cursor math, smart rects) assumes the primary. Crop to
            // the primary's bounds when the monitor list is available.
            if let Ok(monitors) = Monitor::all() {
                if let Some(m) = monitors.iter().find(|m| m.is_primary()).or(monitors.first()) {
                    let (x, y) = (m.x().max(0) as u32, m.y().max(0) as u32);
                    let (w, h) = (m.width(), m.height());
                    if x + w <= img.width() && y + h <= img.height() && (w, h) != img.dimensions() {
                        img = image::imageops::crop_imm(&img, x, y, w, h).to_image();
                    }
                }
            }
            return Ok(img);
        }
    }
    anyhow::bail!(
        "no screen grabber worked (xcap: {xcap_err:#}; also tried gnome-screenshot/spectacle/grim)"
    )
}

/// Grab the primary monitor for a screenshot, compositing in the mouse cursor when the
/// "capture cursor" setting is on. Every capture path (full, region, one-shot CLI) goes
/// through this; `grab_primary` stays pointer-free for callers that don't want it.
pub fn grab_for_shot() -> anyhow::Result<RgbaImage> {
    let mut img = grab_primary()?;
    crate::cursor::maybe_overlay(&mut img, (0, 0));
    Ok(img)
}

/// Full-screen capture path: grab primary, deliver.
pub fn capture_full() -> anyhow::Result<()> {
    let img = grab_for_shot()?;
    let path = deliver(&img)?;
    println!("captured {}x{} -> clipboard + {}", img.width(), img.height(), path.display());
    Ok(())
}

/// Encode, save, put on the clipboard (CF_DIB+CF_DIBV5+"PNG"+CF_HDROP), and play
/// the shutter. Saves to disk BEFORE touching the clipboard so CF_HDROP never
/// points at a file that doesn't exist yet; the clipboard write is best-effort
/// (logged, not fatal) so a transient clipboard failure never loses the shot.
pub fn deliver(img: &RgbaImage) -> anyhow::Result<PathBuf> {
    let png_bytes = encode_png(img)?;
    let path = save_png_bytes(&png_bytes)?;
    // Best-effort, but NOT silent: the toast reports the real outcome. Previously
    // this only logged to stderr, which goes nowhere in a windowed exe, so a lost
    // clipboard race looked identical to success.
    let clipboard_ok = match crate::clipboard::set_all(img, &png_bytes, &path) {
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
    crate::toast::launch(&path, clipboard_ok);
    Ok(path)
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
