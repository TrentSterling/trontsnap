#!/usr/bin/env python3
"""XTest input driver for the harness (no xdotool / python-xlib on this box).

ctypes over libX11 + libXtst. Targets the display from -d / $DISPLAY.

  xinput.py -d :99 move 400 300
  xinput.py -d :99 click 400 300            # left click (position optional)
  xinput.py -d :99 click 400 300 --button 3 # or: rightclick X Y
  xinput.py -d :99 ctrlclick 400 300        # Ctrl held across the click
  xinput.py -d :99 shiftclick 400 300
  xinput.py -d :99 altclick 400 300
  xinput.py -d :99 doubleclick 400 300
  xinput.py -d :99 drag 100 100 300 300     # left-drag, interpolated moves
  xinput.py -d :99 scroll down 3            # wheel notches (button 4/5)
  xinput.py -d :99 key ctrl+a               # combos: ctrl/shift/alt/super + key
  xinput.py -d :99 key Delete Escape ctrl+c # several in a row
  xinput.py -d :99 type "hello world"
  xinput.py -d :99 pos                      # prints "X Y"
  xinput.py -d :99 focus [TITLE]            # XSetInputFocus on the top-level whose name contains
                                            # TITLE (default "TrontSnap"). REQUIRED once: with no
                                            # window manager nothing is focused, and egui drops
                                            # keyboard events + Ctrl/Shift state until FocusIn.
  xinput.py -d :99 windows                  # list top-level windows: id, geometry, mapped, name
  xinput.py -d :99 find TITLE               # search the WHOLE tree (reparented/framed windows too),
                                            # print "0xID WxH+X+Y(abs, root coords) mapped name"; exit 1 if none

Several commands can be chained with ';' as ONE argv token each, e.g.
  xinput.py -d :99 'move 10 10' 'click' 'key Escape'
(only when the first token after the options contains a space).
"""
import argparse
import ctypes
import ctypes.util
import os
import shlex
import sys
import time

x11 = ctypes.CDLL(ctypes.util.find_library("X11") or "libX11.so.6")
xtst = ctypes.CDLL(ctypes.util.find_library("Xtst") or "libXtst.so.6")

VP = ctypes.c_void_p
x11.XOpenDisplay.restype = VP
x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
x11.XCloseDisplay.argtypes = [VP]
x11.XFlush.argtypes = [VP]
x11.XSync.argtypes = [VP, ctypes.c_int]
x11.XDefaultScreen.argtypes = [VP]
x11.XDefaultRootWindow.restype = ctypes.c_ulong
x11.XDefaultRootWindow.argtypes = [VP]
x11.XStringToKeysym.restype = ctypes.c_ulong
x11.XStringToKeysym.argtypes = [ctypes.c_char_p]
x11.XKeysymToKeycode.restype = ctypes.c_ubyte
x11.XKeysymToKeycode.argtypes = [VP, ctypes.c_ulong]
x11.XKeycodeToKeysym.restype = ctypes.c_ulong
x11.XKeycodeToKeysym.argtypes = [VP, ctypes.c_ubyte, ctypes.c_int]
x11.XQueryTree.argtypes = [
    VP, ctypes.c_ulong, ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.c_ulong),
    ctypes.POINTER(ctypes.POINTER(ctypes.c_ulong)), ctypes.POINTER(ctypes.c_uint),
]
x11.XFree.argtypes = [VP]
x11.XFetchName.argtypes = [VP, ctypes.c_ulong, ctypes.POINTER(ctypes.c_char_p)]
x11.XSetInputFocus.argtypes = [VP, ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
x11.XRaiseWindow.argtypes = [VP, ctypes.c_ulong]
x11.XTranslateCoordinates.argtypes = [
    VP, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_int, ctypes.c_int,
    ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_ulong),
]
x11.XGetInputFocus.argtypes = [VP, ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.c_int)]


class Attrs(ctypes.Structure):
    # XWindowAttributes, only the leading fields we read (layout is stable).
    _fields_ = [("x", ctypes.c_int), ("y", ctypes.c_int), ("width", ctypes.c_int),
                ("height", ctypes.c_int), ("border_width", ctypes.c_int), ("depth", ctypes.c_int),
                ("visual", VP), ("root", ctypes.c_ulong), ("class_", ctypes.c_int),
                ("bit_gravity", ctypes.c_int), ("win_gravity", ctypes.c_int),
                ("backing_store", ctypes.c_int), ("backing_planes", ctypes.c_ulong),
                ("backing_pixel", ctypes.c_ulong), ("save_under", ctypes.c_int),
                ("colormap", ctypes.c_ulong), ("map_installed", ctypes.c_int),
                ("map_state", ctypes.c_int), ("all_event_masks", ctypes.c_long),
                ("your_event_mask", ctypes.c_long), ("do_not_propagate_mask", ctypes.c_long),
                ("override_redirect", ctypes.c_int), ("screen", VP)]


x11.XGetWindowAttributes.argtypes = [VP, ctypes.c_ulong, ctypes.POINTER(Attrs)]
x11.XQueryPointer.argtypes = [
    VP, ctypes.c_ulong,
    ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.c_ulong),
    ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_int),
    ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_int),
    ctypes.POINTER(ctypes.c_uint),
]
xtst.XTestFakeMotionEvent.argtypes = [VP, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_ulong]
xtst.XTestFakeButtonEvent.argtypes = [VP, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
xtst.XTestFakeKeyEvent.argtypes = [VP, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]

MODS = {
    "ctrl": "Control_L", "control": "Control_L",
    "shift": "Shift_L",
    "alt": "Alt_L",
    "super": "Super_L", "win": "Super_L", "meta": "Meta_L",
}
# Names a human types vs the X keysym names.
ALIASES = {
    "esc": "Escape", "del": "Delete", "enter": "Return", "return": "Return",
    "space": "space", "tab": "Tab", "backspace": "BackSpace",
    "pgup": "Prior", "pgdn": "Next", "pageup": "Prior", "pagedown": "Next",
    "print": "Print", "prtsc": "Print",
}

# Pacing. egui repaints on events, but a press+release inside one frame after a
# teleporting pointer can miss the hover state, so settle between steps.
SETTLE = 0.08   # after a move, before the button goes down
HOLD = 0.06     # press -> release
GAP = 0.05      # between keys


class X:
    def __init__(self, display):
        self.dpy = x11.XOpenDisplay(display.encode() if display else None)
        if not self.dpy:
            sys.exit(f"xinput: cannot open display {display!r}")
        self.screen = x11.XDefaultScreen(self.dpy)
        self.root = x11.XDefaultRootWindow(self.dpy)

    def sync(self):
        x11.XSync(self.dpy, 0)

    def pos(self):
        r, c = ctypes.c_ulong(), ctypes.c_ulong()
        rx, ry, wx, wy = (ctypes.c_int() for _ in range(4))
        m = ctypes.c_uint()
        x11.XQueryPointer(self.dpy, self.root, ctypes.byref(r), ctypes.byref(c),
                          ctypes.byref(rx), ctypes.byref(ry), ctypes.byref(wx), ctypes.byref(wy),
                          ctypes.byref(m))
        return rx.value, ry.value

    def toplevels(self):
        """[(id, x, y, w, h, mapped, name)] for root's children, bottom-to-top."""
        r, p = ctypes.c_ulong(), ctypes.c_ulong()
        kids = ctypes.POINTER(ctypes.c_ulong)()
        n = ctypes.c_uint()
        x11.XQueryTree(self.dpy, self.root, ctypes.byref(r), ctypes.byref(p),
                       ctypes.byref(kids), ctypes.byref(n))
        out = []
        for i in range(n.value):
            w = kids[i]
            a = Attrs()
            x11.XGetWindowAttributes(self.dpy, w, ctypes.byref(a))
            nm = ctypes.c_char_p()
            name = ""
            if x11.XFetchName(self.dpy, w, ctypes.byref(nm)) and nm.value:
                name = nm.value.decode("utf-8", "replace")
            out.append((w, a.x, a.y, a.width, a.height, a.map_state == 2, name))
        if n.value:
            x11.XFree(kids)
        return out

    def find(self, want, win=None, depth=0):
        """Recursive search by name. Returns [(id, absx, absy, w, h, mapped, name)]."""
        win = win or self.root
        r, p = ctypes.c_ulong(), ctypes.c_ulong()
        kids = ctypes.POINTER(ctypes.c_ulong)()
        n = ctypes.c_uint()
        if not x11.XQueryTree(self.dpy, win, ctypes.byref(r), ctypes.byref(p), ctypes.byref(kids), ctypes.byref(n)):
            return []
        ids = [kids[i] for i in range(n.value)]
        if n.value:
            x11.XFree(kids)
        out = []
        for w in ids:
            nm = ctypes.c_char_p()
            name = ""
            if x11.XFetchName(self.dpy, w, ctypes.byref(nm)) and nm.value:
                name = nm.value.decode("utf-8", "replace")
            if want.lower() in name.lower() and name:
                a = Attrs()
                x11.XGetWindowAttributes(self.dpy, w, ctypes.byref(a))
                ax, ay, ch = ctypes.c_int(), ctypes.c_int(), ctypes.c_ulong()
                x11.XTranslateCoordinates(self.dpy, w, self.root, 0, 0, ctypes.byref(ax), ctypes.byref(ay), ctypes.byref(ch))
                out.append((w, ax.value, ay.value, a.width, a.height, a.map_state == 2, name))
            if depth < 4:
                out += self.find(want, w, depth + 1)
        return out

    def focus_title(self, want):
        hits = [t for t in self.toplevels() if t[5] and want.lower() in t[6].lower()]
        if not hits:
            return None
        w = hits[-1][0]  # topmost match
        x11.XRaiseWindow(self.dpy, w)
        x11.XSetInputFocus(self.dpy, w, 2, 0)  # RevertToParent, CurrentTime
        self.sync()
        return w

    def ensure_focus(self):
        """HARD-WON: no window manager on Xvfb means no window ever has input focus
        (PointerRoot), and then keys + Ctrl/Shift state never reach egui. Quietly
        focus TrontSnap before any keyboard-ish command if nothing is focused."""
        f, rv = ctypes.c_ulong(), ctypes.c_int()
        x11.XGetInputFocus(self.dpy, ctypes.byref(f), ctypes.byref(rv))
        if f.value <= 1:
            self.focus_title("TrontSnap")
            time.sleep(0.15)

    def move(self, x, y):
        xtst.XTestFakeMotionEvent(self.dpy, self.screen, int(x), int(y), 0)
        self.sync()

    def button(self, b, down):
        xtst.XTestFakeButtonEvent(self.dpy, b, 1 if down else 0, 0)
        self.sync()

    def keycode(self, name):
        n = ALIASES.get(name.lower(), name)
        sym = x11.XStringToKeysym(n.encode())
        if not sym:
            sym = x11.XStringToKeysym(name.encode())
        if not sym:
            sys.exit(f"xinput: unknown key {name!r}")
        kc = x11.XKeysymToKeycode(self.dpy, sym)
        if not kc:
            sys.exit(f"xinput: no keycode for {name!r} on this keymap")
        return kc, sym

    def key(self, kc, down):
        xtst.XTestFakeKeyEvent(self.dpy, kc, 1 if down else 0, 0)
        self.sync()


def hold_mods(x, mods):
    codes = [x.keycode(MODS[m])[0] for m in mods]
    for c in codes:
        x.key(c, True)
    time.sleep(0.03)
    return codes


def release_mods(x, codes):
    time.sleep(0.03)
    for c in reversed(codes):
        x.key(c, False)


def click(x, pos, button=1, mods=(), count=1):
    if pos:
        x.move(*pos)
        time.sleep(SETTLE)
    codes = hold_mods(x, mods)
    for _ in range(count):
        x.button(button, True)
        time.sleep(HOLD)
        x.button(button, False)
        time.sleep(HOLD)
    release_mods(x, codes)


def press_key_combo(x, combo):
    parts = [p for p in combo.split("+") if p]
    if combo.endswith("++"):
        parts.append("plus")
    *mods, key = parts
    mods = [m.lower() for m in mods]
    for m in mods:
        if m not in MODS:
            sys.exit(f"xinput: unknown modifier {m!r} in {combo!r}")
    codes = hold_mods(x, mods)
    kc, _ = x.keycode(key)
    x.key(kc, True)
    time.sleep(HOLD)
    x.key(kc, False)
    release_mods(x, codes)
    time.sleep(GAP)


def type_text(x, text):
    shift = x.keycode("Shift_L")[0]
    for ch in text:
        name = {" ": "space", "\n": "Return", "\t": "Tab"}.get(ch, ch)
        kc, sym = x.keycode(name)
        need_shift = x11.XKeycodeToKeysym(x.dpy, kc, 0) != sym
        if need_shift:
            x.key(shift, True)
        x.key(kc, True)
        time.sleep(0.02)
        x.key(kc, False)
        if need_shift:
            x.key(shift, False)
        time.sleep(0.02)


def run(x, argv):
    if not argv:
        sys.exit("xinput: no command (see --help)")
    cmd, args = argv[0].lower(), argv[1:]

    def xy(a):
        if len(a) >= 2 and a[0].lstrip("-").isdigit() and a[1].lstrip("-").isdigit():
            return (int(a[0]), int(a[1])), a[2:]
        return None, a

    if cmd == "pos":
        print("%d %d" % x.pos())
    elif cmd == "windows":
        for w, wx, wy, ww, wh, mapped, name in x.toplevels():
            if name or mapped:
                print(f"0x{w:x} {ww}x{wh}+{wx}+{wy} {'mapped' if mapped else 'unmapped'} {name!r}")
    elif cmd == "find":
        hits = x.find(args[0] if args else "TrontSnap")
        for w, ax, ay, ww, wh, mapped, name in hits:
            print(f"0x{w:x} {ww}x{wh}+{ax}+{ay} {'mapped' if mapped else 'unmapped'} {name!r}")
        if not hits:
            sys.exit(1)
    elif cmd == "focus":
        want = args[0] if args else "TrontSnap"
        w = x.focus_title(want)
        if w is None:
            sys.exit(f"xinput: no mapped window named like {want!r} (try: windows)")
        print(f"focus 0x{w:x}")
    elif cmd == "move":
        p, _ = xy(args)
        if not p:
            sys.exit("xinput: move X Y")
        x.move(*p)
    elif cmd in ("click", "rightclick", "middleclick", "ctrlclick", "shiftclick",
                 "altclick", "ctrlshiftclick", "doubleclick"):
        p, rest = xy(args)
        button = {"rightclick": 3, "middleclick": 2}.get(cmd, 1)
        if "--button" in rest:
            button = int(rest[rest.index("--button") + 1])
        mods = {"ctrlclick": ["ctrl"], "shiftclick": ["shift"], "altclick": ["alt"],
                "ctrlshiftclick": ["ctrl", "shift"]}.get(cmd, [])
        if mods:
            x.ensure_focus()
        click(x, p, button, mods, 2 if cmd == "doubleclick" else 1)
    elif cmd == "drag":
        if len(args) < 4:
            sys.exit("xinput: drag X1 Y1 X2 Y2")
        x1, y1, x2, y2 = (int(v) for v in args[:4])
        x.move(x1, y1)
        time.sleep(SETTLE)
        x.button(1, True)
        time.sleep(HOLD)
        steps = 12
        for i in range(1, steps + 1):
            x.move(x1 + (x2 - x1) * i // steps, y1 + (y2 - y1) * i // steps)
            time.sleep(0.02)
        time.sleep(HOLD)
        x.button(1, False)
    elif cmd == "scroll":
        direction = args[0].lower() if args else "down"
        n = int(args[1]) if len(args) > 1 else 1
        b = 4 if direction == "up" else 5
        for _ in range(n):
            x.button(b, True)
            x.button(b, False)
            time.sleep(0.03)
    elif cmd == "key":
        x.ensure_focus()
        for combo in args:
            press_key_combo(x, combo)
    elif cmd == "type":
        x.ensure_focus()
        type_text(x, " ".join(args))
    elif cmd == "sleep":
        time.sleep(float(args[0]))
    else:
        sys.exit(f"xinput: unknown command {cmd!r}")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("-d", "--display", default=os.environ.get("DISPLAY"))
    ap.add_argument("cmd", nargs=argparse.REMAINDER)
    a = ap.parse_args()
    if not a.display:
        sys.exit("xinput: pass -d :N or set DISPLAY (refusing to guess, the real session is not a target)")
    x = X(a.display)
    try:
        # Chained form: every token is a whole command line.
        if a.cmd and " " in a.cmd[0]:
            for line in a.cmd:
                run(x, shlex.split(line))
        else:
            run(x, a.cmd)
    finally:
        x.sync()
        x11.XCloseDisplay(x.dpy)


if __name__ == "__main__":
    main()
