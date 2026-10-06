#!/usr/bin/env python3
# Pointer/keyboard injection into the PRIVATE headless GNOME Shell (wl_env.sh) through mutter's
# org.gnome.Mutter.RemoteDesktop D-Bus API. Private bus only: the address is read from <wl dir>/dbus.addr.
# usage (via wl_env.sh input):  ACTION...
#   abs X Y     pin the pointer to the BOTTOM-RIGHT (1365,767), then walk to X,Y (unaccelerated).
#               HARD-WON: pinning top-left trips GNOME's hot corner (Activities overview) and the click lands in it.
#   rel DX DY | click | rclick | key Escape|Return|super | wait SEC
#   wheel N     N vertical wheel notches (negative = toward you / scroll down; positive = away / up)
# e.g.  wl_env.sh input key Escape            (leave the overview once after `start`)
#       wl_env.sh input abs 358 52 wait 0.5 click
import sys, time
import gi
from gi.repository import Gio, GLib

if len(sys.argv) < 3:
    raise SystemExit(__doc__)
W, a = sys.argv[1], sys.argv[2:]
addr = open(f"{W}/dbus.addr").read().strip()
conn = Gio.DBusConnection.new_for_address_sync(
    addr, Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION, None, None)

def call(path, iface, method, params=None):
    return conn.call_sync("org.gnome.Mutter.RemoteDesktop", path, iface, method, params, None,
                          Gio.DBusCallFlags.NONE, 5000, None)

sess = call("/org/gnome/Mutter/RemoteDesktop", "org.gnome.Mutter.RemoteDesktop", "CreateSession").unpack()[0]
S = "org.gnome.Mutter.RemoteDesktop.Session"
call(sess, S, "Start")
BTN = {"left": 0x110, "right": 0x111}          # evdev BTN_LEFT / BTN_RIGHT
KEYS = {"Escape": 1, "Return": 28, "super": 125}  # evdev keycodes

def rel(dx, dy):
    call(sess, S, "NotifyPointerMotionRelative", GLib.Variant("(dd)", (float(dx), float(dy)))); time.sleep(0.05)

def button(b, down):
    call(sess, S, "NotifyPointerButton", GLib.Variant("(ib)", (BTN[b], down))); time.sleep(0.08)

i = 0
while i < len(a):
    c = a[i]
    if c == "abs":
        for _ in range(4):
            rel(2000, 2000)
        x, y = float(a[i + 1]), float(a[i + 2]); i += 3
        for _ in range(8):
            rel((x - 1365.0) / 8, (y - 767.0) / 8)
    elif c == "rel":
        rel(a[i + 1], a[i + 2]); i += 3
    elif c in ("click", "rclick"):
        b = "left" if c == "click" else "right"; button(b, True); button(b, False); i += 1
    elif c == "key":
        k = KEYS[a[i + 1]]
        call(sess, S, "NotifyKeyboardKeycode", GLib.Variant("(ub)", (k, True))); time.sleep(0.08)
        call(sess, S, "NotifyKeyboardKeycode", GLib.Variant("(ub)", (k, False))); i += 2
    elif c == "wheel":
        # axis 0 = vertical; discrete steps, like a mouse wheel (a positive step is wheel-down in evdev
        # terms, so negate to make `wheel 1` mean "forward / up" as the Windows picker reads it)
        call(sess, S, "NotifyPointerAxisDiscrete", GLib.Variant("(ui)", (0, -int(a[i + 1])))); time.sleep(0.15); i += 2
    elif c == "wait":
        time.sleep(float(a[i + 1])); i += 2
    else:
        raise SystemExit(f"bad action {c}")
time.sleep(0.2)
call(sess, S, "Stop")
