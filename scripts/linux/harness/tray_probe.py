#!/usr/bin/env python3
"""tray_probe.py CMD [ARGS] -- act as a StatusNotifier HOST against the app's tray item, on the bus in
$DBUS_SESSION_BUS_ADDRESS (run it through `gui_env.sh run N python3 tray_probe.py ...` so that is a rig's
PRIVATE bus; never the real session). Prints JSON, one object per line.

  name                 the item's bus name (org.kde.StatusNotifierItem-<pid>-1), exit 1 if none
  layout               the dbusmenu as a host sees it: [{id,label,enabled,type,toggle}] + revision
  icon                 IconPixmap sizes + sha1 of the 22 px one + red pixels in its bottom-right quadrant
                       (the recording badge) + Status/ToolTip title
  activate             Activate(0,0)  (a KDE click / a GNOME double click)
  secondary            SecondaryActivate(0,0)  (middle click)
  click ID             dbusmenu Event(ID, "clicked")
  signals SECS         listen SECS seconds, print every NewIcon/NewToolTip/LayoutUpdated
"""
import hashlib
import json
import sys
import time

import gi

gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib

ITEM, MENU = "org.kde.StatusNotifierItem", "com.canonical.dbusmenu"
conn = Gio.bus_get_sync(Gio.BusType.SESSION, None)


def call(name, path, iface, method, params=None):
    return conn.call_sync(name, path, iface, method, params, None, Gio.DBusCallFlags.NONE, 5000, None)


def item_name():
    names = call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "ListNames").unpack()[0]
    hits = [n for n in names if n.startswith("org.kde.StatusNotifierItem-")]
    return hits[0] if hits else None


def prop(name, iface, p):
    return call(name, "/StatusNotifierItem", "org.freedesktop.DBus.Properties", "Get",
                GLib.Variant("(ss)", (iface, p))).unpack()[0]


def out(**kw):
    print(json.dumps(kw), flush=True)


def main():
    cmd, args = sys.argv[1], sys.argv[2:]
    name = item_name()
    if cmd == "name":
        if not name:
            sys.exit(1)
        out(name=name)
        return
    if not name:
        sys.exit("tray_probe: no StatusNotifierItem on this bus")
    if cmd == "layout":
        rev, (_, _, kids) = call(name, "/MenuBar", MENU, "GetLayout", GLib.Variant("(iias)", (0, 10, []))).unpack()
        items = []
        for kid in kids:
            i, p, _ = kid
            items.append({"id": i, "label": p.get("label", ""), "enabled": p.get("enabled", True),
                          "type": p.get("type", "standard"), "toggle": p.get("toggle-state")})
        out(revision=rev, items=items)
    elif cmd == "icon":
        pm = prop(name, ITEM, "IconPixmap")
        sizes = [w for w, h, _ in pm]
        w, h, px = next(p for p in pm if p[0] == 22)
        px = bytes(px)
        red = 0
        for y in range(h // 2, h):
            for x in range(w // 2, w):
                a, r, g, b = px[(y * w + x) * 4:(y * w + x) * 4 + 4]
                red += a > 200 and r > 200 and g < 110 and b < 110
        tip = prop(name, ITEM, "ToolTip")
        out(sizes=sizes, sha=hashlib.sha1(px).hexdigest()[:12], badge_px=red,
            status=prop(name, ITEM, "Status"), tip=tip[2], windowid=prop(name, ITEM, "WindowId"))
    elif cmd == "activate":
        call(name, "/StatusNotifierItem", ITEM, "Activate", GLib.Variant("(ii)", (0, 0)))
        out(ok="Activate")
    elif cmd == "secondary":
        call(name, "/StatusNotifierItem", ITEM, "SecondaryActivate", GLib.Variant("(ii)", (0, 0)))
        out(ok="SecondaryActivate")
    elif cmd == "click":
        call(name, "/MenuBar", MENU, "Event", GLib.Variant("(isvu)", (int(args[0]), "clicked", GLib.Variant("i", 0), 0)))
        out(ok="clicked", id=int(args[0]))
    elif cmd == "signals":
        loop = GLib.MainLoop()

        def on_sig(_c, _s, path, iface, member, params):
            out(t=round(time.time(), 2), signal=member, iface=iface, args=str(params.unpack()))

        for iface in (ITEM, MENU):
            conn.signal_subscribe(name, iface, None, None, None, Gio.DBusSignalFlags.NONE, on_sig)
        GLib.timeout_add(int(float(args[0]) * 1000), loop.quit)
        loop.run()
    else:
        sys.exit(__doc__)


main()
