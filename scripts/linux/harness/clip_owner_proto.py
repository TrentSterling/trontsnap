#!/usr/bin/env python3
"""clip_owner_proto.py -- PROTOTYPE of a multi-target clipboard owner (X11 / XWayland).

xclip and wl-copy serve ONE mime type per process. A single selection owner can
offer several targets at once; this script proves that (and which target sets real
paste destinations accept) before the Rust version (a `trontsnap clip-serve`
mode) is written. GTK4 (Gdk.ContentProvider union) on the X11 backend, so on GNOME Wayland it is an
XWayland selection that Mutter bridges to native Wayland clients, exactly like
xclip today.

usage: clip_owner_proto.py [--ttl SECS] OFFER...
  OFFER  MIME=@/path/file     bytes of a file
         MIME=literal text    literal (no leading @)
         MIME=uri:/path       "file://<percent-encoded path>\\r\\n"
         MIME=gnome:/path     "copy\\nfile://<percent-encoded path>"  (x-special/gnome-copied-files)
         MIME=path:/path      the bare path (text/plain for terminals)
Stays alive until another app takes the clipboard or --ttl expires; prints READY.
"""
import argparse, os, re, sys, urllib.parse
os.environ.setdefault("GDK_BACKEND", "x11")
import gi
gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gtk, Gdk, GLib  # noqa: E402

ap = argparse.ArgumentParser()
ap.add_argument("--ttl", type=float, default=120)
ap.add_argument("offers", nargs="+")
args = ap.parse_args()


def uri_of(p):
    return "file://" + urllib.parse.quote(os.path.abspath(p), safe="/")


payload = {}
for o in args.offers:
    # MIME types can contain '=' ("text/plain;charset=utf-8"): split at the first '=' that is
    # followed by a value prefix instead of the first '=' blindly.
    m = re.search(r"=(?=(@|uri:|gnome:|path:))", o)
    mime, val = (o[:m.start()], o[m.end():]) if m else (o.partition("=")[0], o.partition("=")[2])
    if val.startswith("@"):
        data = open(val[1:], "rb").read()
    elif val.startswith("uri:"):
        data = (uri_of(val[4:]) + "\r\n").encode()
    elif val.startswith("gnome:"):
        data = ("copy\n" + uri_of(val[6:])).encode()
    elif val.startswith("path:"):
        data = val[5:].encode()
    else:
        data = val.encode()
    payload[mime] = data

Gtk.init()
loop = GLib.MainLoop()
display = Gdk.Display.get_default()
clip = display.get_clipboard()
provs = [Gdk.ContentProvider.new_for_bytes(m, GLib.Bytes.new(b)) for m, b in payload.items()]
prov = Gdk.ContentProvider.new_union(provs)
assert clip.set_content(prov), "set_content failed"
# Selection loss: "changed" fires for our own set_content too, so arm the handler
# after startup and only treat a non-local change as "someone else took the clipboard".
def _arm():
    clip.connect("changed", lambda c: (not c.is_local()) and (print("LOST", flush=True), loop.quit()))
    return False
GLib.timeout_add(500, _arm)
print("READY", ",".join(payload), flush=True)
GLib.timeout_add(int(args.ttl * 1000), loop.quit)
loop.run()
