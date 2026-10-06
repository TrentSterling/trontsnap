// Cursor compositing, Linux side: a deliberate no-op for now. On X11 the
// pointer could be fetched via XFixes, but on Wayland a client cannot read the
// global cursor at all, and several of the fallback grabbers (gnome-screenshot)
// can include it themselves. Revisit if "capture cursor" matters on this side.

use image::RgbaImage;

pub fn maybe_overlay(_img: &mut RgbaImage, _origin: (i32, i32)) {}
