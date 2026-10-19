// Screen grab + delivery (clipboard + disk).
// Capture is via xcap. Clipboard is our own multi-format writer (see clipboard.rs)
// so screenshots paste everywhere ShareX does, including as a file into terminals.

use std::path::{Path, PathBuf};

use image::RgbaImage;
use xcap::Monitor;

/// One physical display in virtual-desktop coordinates (physical pixels; the process
/// is per-monitor DPI aware, so a 150%-scaled 4K panel reports 3840x2160, and a monitor
/// above or left of the primary has negative x/y).
#[derive(Clone, Debug)]
pub struct Mon {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub primary: bool,
    pub scale: f32,
}

impl Mon {
    fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w as i32 && py >= self.y && py < self.y + self.h as i32
    }
}

/// All attached monitors, primary first, then left-to-right / top-to-bottom.
pub fn monitors() -> anyhow::Result<Vec<Mon>> {
    let mut v: Vec<Mon> = Monitor::all()?
        .iter()
        .map(|m| Mon {
            x: m.x(),
            y: m.y(),
            w: m.width(),
            h: m.height(),
            primary: m.is_primary(),
            scale: m.scale_factor(),
        })
        .collect();
    if v.is_empty() {
        anyhow::bail!("no monitors found");
    }
    v.sort_by_key(|m| (!m.primary, m.x, m.y));
    Ok(v)
}

fn grab_mon(mon: &Mon) -> anyhow::Result<RgbaImage> {
    let all = Monitor::all()?;
    let m = all
        .iter()
        .find(|m| m.x() == mon.x && m.y() == mon.y)
        .ok_or_else(|| anyhow::anyhow!("monitor at ({}, {}) went away", mon.x, mon.y))?;
    Ok(m.capture_image()?)
}

fn cursor_pos() -> (i32, i32) {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

/// The monitor holding the mouse pointer (falls back to the primary).
pub fn cursor_monitor() -> anyhow::Result<Mon> {
    let mons = monitors()?;
    let (cx, cy) = cursor_pos();
    Ok(mons
        .iter()
        .find(|m| m.contains(cx, cy))
        .or_else(|| mons.first())
        .cloned()
        .expect("monitors() is non-empty"))
}

/// Grab the primary monitor as an RGBA image (physical pixels), pointer-free.
pub fn grab_primary() -> anyhow::Result<RgbaImage> {
    let mons = monitors()?;
    grab_mon(&mons[0])
}

/// Bounding box (left, top, right, bottom) of all monitors in screen coordinates.
fn virtual_bounds(mons: &[Mon]) -> (i32, i32, i32, i32) {
    (
        mons.iter().map(|m| m.x).min().unwrap(),
        mons.iter().map(|m| m.y).min().unwrap(),
        mons.iter().map(|m| m.x + m.w as i32).max().unwrap(),
        mons.iter().map(|m| m.y + m.h as i32).max().unwrap(),
    )
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    fn mon(x: i32, y: i32, w: u32, h: u32, primary: bool) -> Mon {
        Mon { x, y, w, h, primary, scale: 1.0 }
    }
    #[test]
    fn second_monitor_above_primary() {
        let m = [mon(0, 0, 3840, 2160, true), mon(0, -1080, 1920, 1080, false)];
        assert_eq!(virtual_bounds(&m), (0, -1080, 3840, 2160));
        assert!(m[1].contains(100, -50) && !m[0].contains(100, -50));
    }
    #[test]
    fn second_monitor_below_and_offset() {
        let m = [mon(0, 0, 3840, 2160, true), mon(960, 2160, 1920, 1080, false)];
        assert_eq!(virtual_bounds(&m), (0, 0, 3840, 3240));
        assert!(m[1].contains(1000, 2200));
    }
    #[test]
    fn left_monitor_negative_x() {
        let m = [mon(0, 0, 3840, 2160, true), mon(-3840, 0, 3840, 2160, false)];
        assert_eq!(virtual_bounds(&m), (-3840, 0, 3840, 2160));
    }
}

/// Stitch every monitor into one image of the whole virtual desktop. Gaps between
/// monitors of different sizes or offsets stay black. Returns the image and its
/// top-left in screen coordinates.
pub fn grab_all() -> anyhow::Result<(RgbaImage, (i32, i32))> {
    let mons = monitors()?;
    let (x0, y0, x1, y1) = virtual_bounds(&mons);
    let mut out = RgbaImage::from_pixel((x1 - x0) as u32, (y1 - y0) as u32, image::Rgba([0, 0, 0, 255]));
    for m in &mons {
        let img = grab_mon(m)?;
        image::imageops::replace(&mut out, &img, (m.x - x0) as i64, (m.y - y0) as i64);
    }
    Ok((out, (x0, y0)))
}

/// What the Fullscreen hotkey grabs. Persisted in settings.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FullTarget {
    /// The monitor the mouse is on (default: you are looking where you point).
    Cursor,
    Primary,
    /// Every monitor stitched into one picture of the whole desktop.
    All,
    /// Every monitor as its own file; the pointer's monitor goes on the clipboard.
    Each,
}

impl FullTarget {
    pub const ALL_TARGETS: [FullTarget; 4] =
        [FullTarget::Cursor, FullTarget::Primary, FullTarget::All, FullTarget::Each];
    pub fn from_u32(v: u32) -> Self {
        match v {
            1 => Self::Primary,
            2 => Self::All,
            3 => Self::Each,
            _ => Self::Cursor,
        }
    }
    pub fn as_u32(self) -> u32 {
        match self {
            Self::Cursor => 0,
            Self::Primary => 1,
            Self::All => 2,
            Self::Each => 3,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Cursor => "Monitor under the mouse",
            Self::Primary => "Primary monitor",
            Self::All => "All monitors in one image",
            Self::Each => "Each monitor as its own file",
        }
    }
}

/// Grab the monitor under the mouse with the pointer composited in. Returns the frame
/// and its top-left in screen coordinates (the region picker needs the origin).
pub fn grab_cursor_monitor() -> anyhow::Result<(RgbaImage, (i32, i32))> {
    let mon = cursor_monitor()?;
    let mut img = grab_mon(&mon)?;
    crate::cursor::maybe_overlay(&mut img, (mon.x, mon.y));
    Ok((img, (mon.x, mon.y)))
}

/// Grab per the "full capture target" setting. `Each` returns the pointer's monitor
/// here (single-image callers); `capture_each` does the per-file work.
pub fn grab_for_shot() -> anyhow::Result<RgbaImage> {
    grab_target(crate::settings::full_target())
}

pub fn grab_target(t: FullTarget) -> anyhow::Result<RgbaImage> {
    match t {
        FullTarget::Primary => {
            let mons = monitors()?;
            let mut img = grab_mon(&mons[0])?;
            crate::cursor::maybe_overlay(&mut img, (mons[0].x, mons[0].y));
            Ok(img)
        }
        FullTarget::All => {
            let (mut img, origin) = grab_all()?;
            crate::cursor::maybe_overlay(&mut img, origin);
            Ok(img)
        }
        FullTarget::Cursor | FullTarget::Each => Ok(grab_cursor_monitor()?.0),
    }
}

/// Grab every monitor separately (pointer drawn on the one it is over), save each as
/// `TrontSnap_<stamp>_monN.png`, and put the pointer's monitor on the clipboard.
pub fn capture_each() -> anyhow::Result<Vec<PathBuf>> {
    let mons = monitors()?;
    let (cx, cy) = cursor_pos();
    let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
    let dir = trontsnap_dir()?;
    std::fs::create_dir_all(&dir)?;
    let mut paths = Vec::new();
    let mut clip: Option<(RgbaImage, Vec<u8>, PathBuf)> = None;
    for (i, m) in mons.iter().enumerate() {
        let mut img = grab_mon(m)?;
        let has_cursor = m.contains(cx, cy);
        if has_cursor {
            crate::cursor::maybe_overlay(&mut img, (m.x, m.y));
        }
        let bytes = encode_png(&img)?;
        let path = dir.join(format!("TrontSnap_{stamp}_mon{}.png", i + 1));
        std::fs::write(&path, &bytes)?;
        if has_cursor || (clip.is_none() && i + 1 == mons.len()) {
            clip = Some((img, bytes, path.clone()));
        }
        paths.push(path);
    }
    if let Some((img, bytes, path)) = clip {
        if let Err(e) = crate::clipboard::set_all(&img, &bytes, &path) {
            eprintln!("trontsnap: clipboard set failed: {e:#}");
        }
        crate::sound::play_shutter();
        crate::toast::launch(&path, true);
    }
    Ok(paths)
}

/// Full-screen capture path: grab per the setting, deliver.
pub fn capture_full() -> anyhow::Result<()> {
    capture_with(crate::settings::full_target())
}

/// Capture with an explicit target (tray items and CLI modes use this).
pub fn capture_with(t: FullTarget) -> anyhow::Result<()> {
    if t == FullTarget::Each {
        for p in capture_each()? {
            println!("captured -> {}", p.display());
        }
        return Ok(());
    }
    let img = grab_target(t)?;
    let path = deliver(&img)?;
    println!("captured {}x{} -> clipboard + {}", img.width(), img.height(), path.display());
    Ok(())
}

/// Print the monitor layout (debug aid: `trontsnap monitors`).
pub fn print_monitors() -> anyhow::Result<()> {
    for (i, m) in monitors()?.iter().enumerate() {
        println!(
            "mon{} {}x{} at ({}, {}) scale {:.2}{}",
            i + 1,
            m.w,
            m.h,
            m.x,
            m.y,
            m.scale,
            if m.primary { " primary" } else { "" }
        );
    }
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

pub(crate) fn encode_png(img: &RgbaImage) -> anyhow::Result<Vec<u8>> {
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
