TrontSnap for Linux
===================

Fast screenshots, region capture, OCR, recording and a searchable history
gallery. https://github.com/TrentSterling/trontsnap

Install (no root):   ./install.sh        -> ~/.local/bin/trontsnap + app menu entry
Upgrade:             unpack the newer tarball, run ./install.sh again
Uninstall:           ~/.local/share/trontsnap/uninstall.sh

First launch:
  - starts at login (hidden) and, on GNOME, binds Print, Ctrl+Print,
    Ctrl+Shift+Print and Ctrl+Alt+Print (full / region / record / OCR).
    uninstall.sh puts GNOME's own Print binding back.
  - other desktops: bind `trontsnap full`, `trontsnap region` and
    `trontsnap ocr` yourself in the keyboard settings.

Needs (usually apt install ffmpeg xclip wl-clipboard):
  ffmpeg            recording, GIF export, video thumbnails
  xclip or wl-clipboard   clipboard
Optional:
  tesseract-ocr     copy text from a region
  gnome-screenshot / grim / spectacle   Wayland capture fallback

It also wants an X11 or XWayland session plus the usual GL/xkbcommon client
libraries, which any desktop install already has. The binary needs a glibc at
least as new as the machine that built it (see the release notes).

The bundled Rajdhani font is under the SIL Open Font License
(share/doc/trontsnap/Rajdhani-OFL.txt).
