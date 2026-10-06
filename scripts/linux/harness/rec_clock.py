#!/usr/bin/env python3
"""Fullscreen wall-clock beacon for measuring recorder latency/sync on the REAL screen.

Draws, on a flat #14283c ground: a human-readable HH:MM:SS.mmm, plus a 40-block
binary strip of (epoch milliseconds mod 2^40) along the top-left (block i = bit
39-i, 32 px square, white=1 black=0) and a 41st "flash" block that is white
only while a flash is active. rec_decode() reads those back from a PNG/video
frame with PIL alone, so a clip frame converts to "what wall-clock instant was
on screen" to within ~1 frame. Native Wayland fullscreen = pixel-exact origin.

stdin commands:  flash  -> flash block on for 0.6 s and play a 1 kHz beep (pw-play) at that
                           instant (stdout: {"flash_wall": <epoch>})
                 quit
stdout: {"ready": epoch} once drawn; {"ptr": [x, y]} on pointer motion/enter.

Also importable: decode_frame(path) -> (epoch_ms_mod_2^40 | None, flash_bool).
"""
import json, os, struct, subprocess, sys, tempfile, time, wave, math

BLK = 32
NBITS = 40
# Bottom edge: GNOME pins notification banners top-centre, and the app's own toast docks
# bottom-right at y<=720, so y=724..756 is the one strip nothing else paints on.
YOFF = 724
GROUND = (0x14, 0x28, 0x3C)


def decode_frame(path, yoff=YOFF):
    from PIL import Image
    im = Image.open(path).convert("RGB")
    val = 0
    for i in range(NBITS):
        x, y = i * BLK + BLK // 2, yoff + BLK // 2
        r, g, b = im.getpixel((x, y))
        val = (val << 1) | (1 if (r + g + b) / 3 > 128 else 0)
    r, g, b = im.getpixel((NBITS * BLK + BLK // 2, yoff + BLK // 2))
    return val, (r + g + b) / 3 > 128


def wall_from_mod(v, ref_epoch_s):
    """Resolve the 40-bit ms value against a nearby reference time (seconds)."""
    ref_ms = int(ref_epoch_s * 1000)
    mod = 1 << NBITS
    base = ref_ms - (ref_ms % mod)
    cands = [base - mod + v, base + v, base + mod + v]
    return min(cands, key=lambda c: abs(c - ref_ms)) / 1000.0


def make_beep(path):
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(48000)
        n = int(48000 * 0.3)
        w.writeframes(b"".join(struct.pack("<h", int(6000 * math.sin(2 * math.pi * 1000 * i / 48000))) for i in range(n)))


def main():
    import gi
    gi.require_version("Gtk", "3.0")
    gi.require_version("GdkPixbuf", "2.0")
    from gi.repository import Gtk, GLib, Gdk, GdkPixbuf

    # HARD-WON: python3-gi-cairo isn't installed here, so a Gtk.DrawingArea "draw"
    # handler dies with "Couldn't find foreign struct converter for cairo.Context"
    # and the window silently stays default grey. Build the strip as a pixbuf instead.
    beep = os.path.join(tempfile.gettempdir(), "rec_clock_beep.wav")
    make_beep(beep)
    state = {"flash_until": 0.0, "drawn": False, "last": time.time()}
    W = (NBITS + 1) * BLK
    blocks = {0: bytes(3 * BLK), 1: bytes([255]) * (3 * BLK)}

    win = Gtk.Window(title="rec_clock")
    css = Gtk.CssProvider()
    css.load_from_data(b"window{background:#14283c;} label{color:#e6f2ff;font:bold 96px monospace;}")
    Gtk.StyleContext.add_provider_for_screen(Gdk.Screen.get_default(), css, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
    fixed = Gtk.Fixed()
    img = Gtk.Image()
    lbl = Gtk.Label(label="")
    fixed.put(img, 0, YOFF)
    fixed.put(lbl, 40, 80)
    win.add(fixed)
    win.fullscreen()
    win.add_events(Gdk.EventMask.POINTER_MOTION_MASK | Gdk.EventMask.ENTER_NOTIFY_MASK)

    def tick():
        now = time.time()
        # a gap here = this process was starved/blocked (not the compositor): rules the beacon in or out
        if now - state["last"] > 0.08:
            print(json.dumps({"stall": [round(state["last"], 3), round(now - state["last"], 3)]}), flush=True)
        state["last"] = now
        v = int(now * 1000) % (1 << NBITS)
        row = b"".join(blocks[(v >> (NBITS - 1 - i)) & 1] for i in range(NBITS))
        row += blocks[1 if now < state["flash_until"] else 0]
        pb = GdkPixbuf.Pixbuf.new_from_bytes(GLib.Bytes.new(row * BLK), GdkPixbuf.Colorspace.RGB, False, 8, W, BLK, W * 3)
        img.set_from_pixbuf(pb)
        lbl.set_text(time.strftime("%H:%M:%S", time.localtime(now)) + ".%03d" % (int(now * 1000) % 1000))
        if not state["drawn"]:
            state["drawn"] = True
            print(json.dumps({"ready": now}), flush=True)
        return True

    win.connect("motion-notify-event", lambda w, e: print(json.dumps({"ptr": [round(e.x), round(e.y)]}), flush=True) or False)
    win.connect("enter-notify-event", lambda w, e: print(json.dumps({"ptr": [round(e.x), round(e.y)]}), flush=True) or False)
    win.connect("destroy", Gtk.main_quit)
    GLib.timeout_add(16, tick)

    def on_stdin(fd, cond):
        line = sys.stdin.readline()
        if not line or line.strip() == "quit":
            Gtk.main_quit()
            return False
        if line.strip() == "flash":
            now = time.time()
            state["flash_until"] = now + 0.6
            subprocess.Popen(["pw-play", beep], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            print(json.dumps({"flash_wall": now}), flush=True)
        return True

    GLib.io_add_watch(sys.stdin.fileno(), GLib.IO_IN, on_stdin)
    win.show_all()
    Gtk.main()
    os._exit(0)


if __name__ == "__main__":
    main()
