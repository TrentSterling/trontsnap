// Cursor compositing, Linux side. On Wayland a client cannot read the global pointer at all, so
// there the CLI grabber draws it (capture.rs asks gnome-screenshot/spectacle/grim for it) and this
// is a no-op. On an X11 session xcap grabs without a pointer; XFixes can read the cursor image, so
// it is composited here.

use image::RgbaImage;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

/// True on a native Wayland session, where the grabber (not us) owns the pointer.
pub fn wayland_session() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// Composite the current pointer into an xcap grab of the primary monitor when "Capture cursor" is
/// on. `_origin` is the Windows virtual-desktop offset; here the primary monitor's own position
/// (queried below) plays that role. Best-effort: any failure leaves the shot pointer-less.
pub fn maybe_overlay(img: &mut RgbaImage, _origin: (i32, i32)) {
    if !crate::settings::capture_cursor() || wayland_session() {
        return;
    }
    if let Err(e) = overlay_x11(img) {
        crate::clipboard::log_line(&format!("cursor overlay skipped: {e:#}"));
    }
}

fn overlay_x11(img: &mut RgbaImage) -> anyhow::Result<()> {
    let (conn, _screen) = RustConnection::connect(None)?;
    conn.xfixes_query_version(4, 0)?.reply()?;
    let c = conn.xfixes_get_cursor_image()?.reply()?;
    // The pointer is in root coordinates; the grab starts at the primary monitor's corner.
    let (mx, my) = xcap::Monitor::all()
        .ok()
        .and_then(|ms| {
            let primary = ms.iter().position(|m| m.is_primary()).unwrap_or(0);
            ms.get(primary).map(|m| (m.x(), m.y()))
        })
        .unwrap_or((0, 0));
    let left = c.x as i32 - c.xhot as i32 - mx;
    let top = c.y as i32 - c.yhot as i32 - my;
    for j in 0..c.height as i32 {
        for i in 0..c.width as i32 {
            let (x, y) = (left + i, top + j);
            if x < 0 || y < 0 || x >= img.width() as i32 || y >= img.height() as i32 {
                continue;
            }
            // XFixes hands back ARGB with premultiplied alpha: out = src + dst * (1 - a).
            let argb = c.cursor_image[(j * c.width as i32 + i) as usize];
            let a = (argb >> 24) & 0xff;
            if a == 0 {
                continue;
            }
            let dst = img.get_pixel_mut(x as u32, y as u32);
            let src = [(argb >> 16) & 0xff, (argb >> 8) & 0xff, argb & 0xff];
            for k in 0..3 {
                dst.0[k] = (src[k] + dst.0[k] as u32 * (255 - a) / 255).min(255) as u8;
            }
            dst.0[3] = 255;
        }
    }
    Ok(())
}
