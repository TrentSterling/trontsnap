#!/usr/bin/env python3
"""gtk4_clip_probe.py -- what can a real GTK4 app paste from the clipboard?

Opens a tiny native window (Wayland when WAYLAND_DISPLAY is set and GDK_BACKEND
is not forced; under an Xvfb rig set GDK_BACKEND=x11), waits until it is the
active window (Wayland only delivers the selection to the focused client), then
dumps what Gdk.Clipboard offers and what each consumer-style read returns:

  formats   Gdk.ContentFormats mime list (what a paste destination SEES)
  <mime>    raw bytes per advertised mime (size + sha1 + short preview)
  texture   read_texture_async  (what image-paste in GTK4 apps does)
  text      read_text_async
  files     read_value_async(Gdk.FileList)  (what Nautilus-style file paste uses)

usage: gtk4_clip_probe.py [--out DIR] [--wait SECS] [--tag NAME] [--no-bytes]
Output: one JSON object on stdout, bytes in DIR/<tag>_<mime-with-slashes-as-_>.
Exit 2 if the window never became active (no verdict possible).
"""
import argparse, hashlib, json, os, sys

import gi
gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gtk, Gdk, GLib, Gio  # noqa: E402

ap = argparse.ArgumentParser()
ap.add_argument("--out", default=".")
ap.add_argument("--wait", type=float, default=8.0)
ap.add_argument("--tag", default="probe")
ap.add_argument("--no-bytes", action="store_true")
args = ap.parse_args()
os.makedirs(args.out, exist_ok=True)

result = {"tag": args.tag, "backend": None, "active": False, "formats": [], "mimes": {}}
app = Gtk.Application(application_id="xyz.tront.cliprobe", flags=Gio.ApplicationFlags.NON_UNIQUE)


def finish(code=0):
    print(json.dumps(result, indent=1))
    app.quit()
    sys.stdout.flush()
    os._exit(code)


def read_bytes(clip, mime, done):
    # HARD-WON: never read the stream synchronously on the main thread. On the X11
    # backend the transfer needs this very main loop, so a blocking read_bytes()
    # deadlocks (futex wait forever). Splice into memory asynchronously instead.
    def record(data, got):
        safe = mime.replace("/", "_").replace(";", "_").replace("=", "_")
        path = os.path.join(args.out, f"{args.tag}_{safe}")
        if not args.no_bytes:
            open(path, "wb").write(data)
        result["mimes"][mime] = {
            "got": got, "size": len(data), "sha1": hashlib.sha1(data).hexdigest()[:12],
            "preview": data[:160].decode("utf-8", "replace") if not mime.startswith("image/") else None,
            "file": path if not args.no_bytes else None,
        }

    def cb(src, res):
        try:
            stream, got = src.read_finish(res)
            out = Gio.MemoryOutputStream.new_resizable()

            def spliced(o, r):
                try:
                    o.splice_finish(r)
                    record(bytes(o.steal_as_bytes().get_data()), got)
                except Exception as e:  # noqa: BLE001
                    result["mimes"][mime] = {"error": str(e)}
                done()
            out.splice_async(stream, Gio.OutputStreamSpliceFlags.CLOSE_SOURCE | Gio.OutputStreamSpliceFlags.CLOSE_TARGET,
                             GLib.PRIORITY_DEFAULT, None, spliced)
        except Exception as e:  # noqa: BLE001
            result["mimes"][mime] = {"error": str(e)}
            done()
    clip.read_async([mime], GLib.PRIORITY_DEFAULT, None, cb)


def run_probe(win):
    clip = win.get_clipboard()
    result["backend"] = type(win.get_display()).__name__
    fmts = clip.get_formats()
    result["formats"] = list(fmts.get_mime_types() or [])
    result["format_gtypes"] = [t.name for t in (fmts.get_gtypes() or [])]
    result["is_local"] = clip.is_local()
    steps = []

    for m in result["formats"]:
        steps.append(lambda d, m=m: read_bytes(clip, m, d))

    def tex(d):
        def cb(src, res):
            try:
                t = src.read_texture_finish(res)
                result["texture"] = f"{t.get_width()}x{t.get_height()}" if t else None
            except Exception as e:  # noqa: BLE001
                result["texture"] = f"ERR {e}"
            d()
        clip.read_texture_async(None, cb)

    def txt(d):
        def cb(src, res):
            try:
                result["text"] = src.read_text_finish(res)
            except Exception as e:  # noqa: BLE001
                result["text"] = f"ERR {e}"
            d()
        clip.read_text_async(None, cb)

    def files(d):
        def cb(src, res):
            try:
                v = src.read_value_finish(res)
                result["files"] = [f.get_path() or f.get_uri() for f in v.get_files()] if v else None
            except Exception as e:  # noqa: BLE001
                result["files"] = f"ERR {e}"
            d()
        clip.read_value_async(Gdk.FileList, GLib.PRIORITY_DEFAULT, None, cb)

    steps += [tex, txt, files]

    def next_step():
        if not steps:
            finish(0)
            return
        steps.pop(0)(next_step)
    next_step()


def on_activate(_app):
    win = Gtk.ApplicationWindow(application=_app, title="cliprobe", default_width=240, default_height=80)
    win.set_child(Gtk.Label(label=f"clipboard probe: {args.tag}"))
    started = [False]

    def maybe_start(*_):
        if win.is_active() and not started[0]:
            started[0] = True
            result["active"] = True
            GLib.timeout_add(250, lambda: (run_probe(win), False)[1])
    win.connect("notify::is-active", maybe_start)
    win.present()

    def timeout():
        if not started[0]:
            # Not focused: still try, but flag it. On Wayland the offer list is empty then.
            started[0] = True
            run_probe(win)
        return False
    GLib.timeout_add(int(args.wait * 1000), timeout)
    GLib.timeout_add(int(args.wait * 1000) + 15000, lambda: finish(3))  # hard stop


app.connect("activate", on_activate)
app.run([])
