// Screen grab + delivery (clipboard + disk).
// Capture is via xcap. Clipboard is our own multi-format writer (see clipboard.rs)
// so screenshots paste everywhere ShareX does, including as a file into terminals.

use std::path::{Path, PathBuf};

use image::RgbaImage;
use xcap::Monitor;

/// Grab the primary monitor as an RGBA image (physical pixels).
/// Primary-monitor only for now; multi-monitor virtual-desktop capture is next.
pub fn grab_primary() -> anyhow::Result<RgbaImage> {
    let monitors = Monitor::all()?;
    let monitor = monitors
        .iter()
        .find(|m| m.is_primary())
        .or_else(|| monitors.first())
        .ok_or_else(|| anyhow::anyhow!("no monitors found"))?;
    Ok(monitor.capture_image()?)
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
    if let Err(e) = crate::clipboard::set_all(img, &png_bytes, &path) {
        eprintln!("trontsnap: clipboard set failed: {e:#}");
    }
    crate::sound::play_shutter();
    // Corner toast (ShareX-style) as its own tiny process, so it never touches the main
    // app's window/loop. A plain std::process::Command spawn (inside toast::launch) is all
    // it takes now that TrontSnap is a portable, non-uiAccess, Medium-integrity exe.
    crate::toast::launch(&path);
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
