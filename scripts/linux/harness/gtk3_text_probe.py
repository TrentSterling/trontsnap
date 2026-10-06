#!/usr/bin/env python3
"""gtk3_text_probe.py -- what would a GTK3 text consumer (VTE terminals such as
gnome-terminal, gedit, most GNOME text widgets) get from the clipboard? Same
Gtk.Clipboard calls they make.

  X11 backend (XWayland on the live session, or an Xvfb rig):  GDK_BACKEND=x11
  native Wayland (what gnome-terminal really is):               GDK_BACKEND=wayland --window
      Wayland only hands the selection to a FOCUSED client, so --window maps a tiny
      window and waits until it is active before asking.

Prints one JSON object: targets, text_available, text, uris_available, uris, image_available.
"""
import json, os, sys
os.environ.setdefault("GDK_BACKEND", "x11")
import gi
gi.require_version("Gtk", "3.0")
from gi.repository import Gtk, Gdk, GLib  # noqa: E402


def probe():
    cb = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD)
    ok, targets = cb.wait_for_targets()
    print(json.dumps({
        "backend": type(Gdk.Display.get_default()).__name__,
        "targets": sorted(t.name() for t in targets) if ok and targets else [],
        "text_available": cb.wait_is_text_available(),
        "text": cb.wait_for_text(),
        "uris_available": cb.wait_is_uris_available(),
        "uris": cb.wait_for_uris(),
        "image_available": cb.wait_is_image_available(),
    }), flush=True)


if "--window" not in sys.argv:
    probe()
    sys.exit(0)

win = Gtk.Window(title="gtk3probe")
win.set_default_size(200, 60)
win.add(Gtk.Label(label="clipboard probe (gtk3)"))
done = [False]


def run(*_):
    if not done[0]:
        done[0] = True
        probe()
        Gtk.main_quit()
    return False


win.connect("notify::is-active", lambda w, p: w.is_active() and GLib.timeout_add(300, run))
win.connect("destroy", Gtk.main_quit)
win.show_all()
GLib.timeout_add(8000, run)  # not focused after 8 s: ask anyway and let the output show it
Gtk.main()
