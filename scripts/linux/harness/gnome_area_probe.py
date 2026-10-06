#!/usr/bin/env python3
"""Does org.gnome.Shell.ScreencastArea give us a region recording, with the cursor?

  gnome_area_probe.py X Y W H --out DIR [--secs 3] [--cursor 1|0]

Records the given rectangle of the live desktop for a few seconds through the same
D-Bus call start_gnome() would use for a region (CPU-only VP8: no GPU video block
is touched), then reports the clip's real size/fps and extracts frame(s) for a look.
"""
import argparse, json, os, subprocess, sys, time


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("x", type=int); ap.add_argument("y", type=int); ap.add_argument("w", type=int); ap.add_argument("h", type=int)
    ap.add_argument("--out", required=True)
    ap.add_argument("--secs", type=float, default=3)
    ap.add_argument("--cursor", type=int, default=1)
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    from gi.repository import Gio, GLib
    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    tpl = os.path.abspath(os.path.join(a.out, "area_%d" % int(time.time())))  # relative would land in ~/Videos
    opts = {"framerate": GLib.Variant("i", 30), "draw-cursor": GLib.Variant("b", bool(a.cursor))}
    t0 = time.time()
    ok, f = bus.call_sync("org.gnome.Shell.Screencast", "/org/gnome/Shell/Screencast", "org.gnome.Shell.Screencast",
                          "ScreencastArea", GLib.Variant("(iiiisa{sv})", (a.x, a.y, a.w, a.h, tpl, opts)),
                          GLib.VariantType("(bs)"), Gio.DBusCallFlags.NONE, 8000, None).unpack()
    t_ready = time.time() - t0
    time.sleep(a.secs)
    bus.call_sync("org.gnome.Shell.Screencast", "/org/gnome/Shell/Screencast", "org.gnome.Shell.Screencast",
                  "StopScreencast", None, None, Gio.DBusCallFlags.NONE, 8000, None)
    last = -1
    for _ in range(40):
        n = os.path.getsize(f) if os.path.exists(f) else 0
        if n and n == last:
            break
        last = n
        time.sleep(0.15)
    r = {"ok": ok, "file": f, "start_call_s": round(t_ready, 2), "bytes": os.path.getsize(f) if os.path.exists(f) else 0}
    r["probe"] = json.loads(subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries",
                                            "stream=codec_name,width,height,r_frame_rate:format=duration", "-of", "json", f],
                                           capture_output=True, text=True).stdout or "{}")
    n = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-count_packets", "-show_entries",
                        "stream=nb_read_packets", "-of", "csv=p=0", f], capture_output=True, text=True).stdout.strip()
    r["frames"] = n
    png = f + "_frame.png"
    subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-i", f, "-frames:v", "1", "-update", "1", png])
    r["frame_png"] = png
    print(json.dumps(r, indent=1))


if __name__ == "__main__":
    main()
