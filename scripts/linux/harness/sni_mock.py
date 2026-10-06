#!/usr/bin/env python3
"""Mock org.kde.StatusNotifierWatcher for a rig's PRIVATE bus (what GNOME's AppIndicator extension is
on the real session). Logs, as JSON lines to $SNI_LOG: every RegisterStatusNotifierItem, the item's
properties as the host would read them (Id, Status, Category, IconPixmap sizes, ToolTip, Menu), and when
the item's bus name vanishes. SIGUSR1 = a left click: calls Activate(0,0) on the last registered item that is still on the bus.

  gui_env.sh run N python3 sni_mock.py &
"""
import json, os, signal, sys, time
import gi
gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib

LOG = os.environ.get("SNI_LOG", "/dev/null")
XML = """<node><interface name="org.kde.StatusNotifierWatcher">
<method name="RegisterStatusNotifierItem"><arg type="s" direction="in"/></method>
<method name="RegisterStatusNotifierHost"><arg type="s" direction="in"/></method>
<property name="RegisteredStatusNotifierItems" type="as" access="read"/>
<property name="IsStatusNotifierHostRegistered" type="b" access="read"/>
<property name="ProtocolVersion" type="i" access="read"/>
<signal name="StatusNotifierItemRegistered"><arg type="s"/></signal>
<signal name="StatusNotifierItemUnregistered"><arg type="s"/></signal>
<signal name="StatusNotifierHostRegistered"/>
</interface></node>"""
items, last = [], {}
proxies = {}   # name -> proxy of every item still on the bus


def log(d):
    d["t"] = time.time()
    with open(LOG, "a") as f:
        f.write(json.dumps(d) + "\n")


def inspect(bus, name):
    def made(src, res):
        try:
            p = Gio.DBusProxy.new_for_bus_finish(res)
        except GLib.Error as e:
            log({"op": "proxy-error", "name": name, "err": str(e)})
            return
        last["proxy"] = p
        proxies[name] = p
        g = lambda k: (p.get_cached_property(k).unpack() if p.get_cached_property(k) is not None else None)
        pm = g("IconPixmap") or []
        log({"op": "item", "name": name, "id": g("Id"), "status": g("Status"), "category": g("Category"),
             "pixmaps": [[w, h, len(b)] for w, h, b in pm], "tooltip": g("ToolTip") and [g("ToolTip")[2], g("ToolTip")[3]],
             "menu": g("Menu"), "title": g("Title")})
    Gio.DBusProxy.new_for_bus(Gio.BusType.SESSION, Gio.DBusProxyFlags.NONE, None, name, "/StatusNotifierItem",
                              "org.kde.StatusNotifierItem", None, made)
    Gio.bus_watch_name(Gio.BusType.SESSION, name, Gio.BusNameWatcherFlags.NONE, None,
                       lambda c, n: (log({"op": "gone", "name": n}), items.remove(n) if n in items else None, proxies.pop(n, None)))


def handle(conn, sender, path, iface, method, params, inv):
    if method == "RegisterStatusNotifierItem":
        (name,) = params.unpack()
        items.append(name)
        log({"op": "register", "name": name})
        inspect(conn, name)
        conn.emit_signal(None, path, iface, "StatusNotifierItemRegistered", GLib.Variant("(s)", (name,)))
    inv.return_value(None)


def get_prop(conn, sender, path, iface, prop):
    return {"RegisteredStatusNotifierItems": GLib.Variant("as", items),
            "IsStatusNotifierHostRegistered": GLib.Variant("b", True),
            "ProtocolVersion": GLib.Variant("i", 0)}.get(prop)


def on_bus(conn, name):
    node = Gio.DBusNodeInfo.new_for_xml(XML)
    conn.register_object("/StatusNotifierWatcher", node.interfaces[0], handle, get_prop, None)


def click(*_):
    # HARD-WON: the LAST REGISTERED item, not the last seen: a short-lived second instance (rec_fake S6's
    # cold-started app) used to leave a dead proxy here, and every later click went nowhere.
    p = next((proxies[n] for n in reversed(items) if n in proxies), None)
    if p is None:
        return True
    log({"op": "activate"})
    p.call("Activate", GLib.Variant("(ii)", (0, 0)), Gio.DBusCallFlags.NONE, 5000, None, lambda s, r: s.call_finish(r))
    return True


Gio.bus_own_name(Gio.BusType.SESSION, "org.kde.StatusNotifierWatcher", Gio.BusNameOwnerFlags.NONE, on_bus,
                 lambda *a: print(json.dumps({"ready": time.time()}), flush=True), lambda *a: sys.exit("could not own the name"))
GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGUSR1, click)
GLib.MainLoop().run()
