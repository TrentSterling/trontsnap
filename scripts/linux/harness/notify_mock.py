#!/usr/bin/env python3
"""Mock org.freedesktop.Notifications for a rig's PRIVATE bus: logs every Notify /
CloseNotification as a JSON line (epoch time, id, replaces, summary, body) to $NOTIFY_LOG
and honours replaces_id like a real daemon (same id back), so a test can assert what a user
would have seen on screen and when. Never run on the real session bus (it would shadow GNOME's).

  gui_env.sh run N python3 notify_mock.py &      # after `gui_env.sh start`, before the first banner
"""
import json, os, sys, time
import gi
gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib

LOG = os.environ.get("NOTIFY_LOG", "/dev/null")
XML = """<node><interface name="org.freedesktop.Notifications">
<method name="Notify"><arg type="s" direction="in"/><arg type="u" direction="in"/><arg type="s" direction="in"/>
<arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="as" direction="in"/>
<arg type="a{sv}" direction="in"/><arg type="i" direction="in"/><arg type="u" direction="out"/></method>
<method name="CloseNotification"><arg type="u" direction="in"/></method>
<method name="GetCapabilities"><arg type="as" direction="out"/></method>
<method name="GetServerInformation"><arg type="s" direction="out"/><arg type="s" direction="out"/>
<arg type="s" direction="out"/><arg type="s" direction="out"/></method>
<signal name="NotificationClosed"><arg type="u"/><arg type="u"/></signal>
</interface></node>"""
state = {"next": 1}


def log(d):
    d["t"] = time.time()
    with open(LOG, "a") as f:
        f.write(json.dumps(d) + "\n")


def handle(conn, sender, path, iface, method, params, inv):
    if method == "Notify":
        app, rid, icon, summary, body, actions, hints, timeout = params.unpack()
        nid = rid or state["next"]
        if not rid:
            state["next"] += 1
        log({"op": "notify", "id": nid, "replaces": rid, "summary": summary, "body": body, "app": app})
        inv.return_value(GLib.Variant("(u)", (nid,)))
    elif method == "CloseNotification":
        (nid,) = params.unpack()
        log({"op": "close", "id": nid})
        inv.return_value(None)
    elif method == "GetCapabilities":
        inv.return_value(GLib.Variant("(as)", (["body"],)))
    else:
        inv.return_value(GLib.Variant("(ssss)", ("mock", "trontsnap-harness", "1", "1.2")))


def on_bus(conn, name):
    node = Gio.DBusNodeInfo.new_for_xml(XML)
    conn.register_object("/org/freedesktop/Notifications", node.interfaces[0], handle, None, None)


Gio.bus_own_name(Gio.BusType.SESSION, "org.freedesktop.Notifications", Gio.BusNameOwnerFlags.NONE, on_bus,
                 lambda *a: print(json.dumps({"ready": time.time()}), flush=True), lambda *a: sys.exit("could not own the name"))
GLib.MainLoop().run()
