#!/usr/bin/env python3
"""fm1_mock.py [--delay SECS] [--log FILE] — stand-in file manager for a rig's PRIVATE dbus.

Owns org.freedesktop.FileManager1 (ShowItems/ShowFolders reply after --delay) and the bare name
org.freedesktop.portal.Desktop (no objects: OpenURI fails at once). Run it INSIDE the rig:
    gui_env.sh run 91 python3 fm1_mock.py --delay 8 --log reveal.log &
HARD-WON: without it, a Reveal in a rig asks dbus to activate the real Nautilus, and the rig's
dbus hands that to the REAL `systemd --user` (tracker, portals, gvfs... all started with the
rig's HOME). Owning the names first means nothing is activatable, so nothing leaks, and the
delay reproduces a cold file manager that answers slowly.
"""
import argparse
import sys
import time

import gi
gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib  # noqa: E402

ap = argparse.ArgumentParser()
ap.add_argument("--delay", type=float, default=8.0)
ap.add_argument("--log", default="/dev/null")
a = ap.parse_args()

XML = """<node><interface name="org.freedesktop.FileManager1">
<method name="ShowItems"><arg type="as" name="uris" direction="in"/><arg type="s" name="startup_id" direction="in"/></method>
<method name="ShowFolders"><arg type="as" name="uris" direction="in"/><arg type="s" name="startup_id" direction="in"/></method>
</interface></node>"""
node = Gio.DBusNodeInfo.new_for_xml(XML)


def log(msg):
    with open(a.log, "a") as f:
        f.write("%s %s\n" % (time.strftime("%H:%M:%S"), msg))


def on_call(conn, sender, path, iface, method, params, inv):
    log("%s %s (replying in %.1fs)" % (method, params.unpack()[0], a.delay))
    def reply():
        try:
            inv.return_value(None)
        except Exception:
            pass
        return False
    GLib.timeout_add(int(a.delay * 1000), reply)


def on_bus(conn, name):
    conn.register_object("/org/freedesktop/FileManager1", node.interfaces[0], on_call, None, None)


Gio.bus_own_name(Gio.BusType.SESSION, "org.freedesktop.FileManager1", Gio.BusNameOwnerFlags.NONE, on_bus, None, None)
Gio.bus_own_name(Gio.BusType.SESSION, "org.freedesktop.portal.Desktop", Gio.BusNameOwnerFlags.NONE, None, None, None)
log("mock up")
GLib.MainLoop().run()
