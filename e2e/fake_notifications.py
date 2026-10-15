"""fake org.freedesktop.Notifications on the session bus, recording what apps show.

prints "ready" once the bus name is owned, then one json line per notification shown:
{"id": 1, "summary": "...", "body": "...", "actions": ["key", "label", ...]}.
reads commands from stdin, one per line: "invoke <id> <key>" clicks a button like a user
would, emitting ActionInvoked and then NotificationClosed.
"""

import json
import sys

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

IFACE = "org.freedesktop.Notifications"
PATH = "/org/freedesktop/Notifications"
# NotificationClosed reason for a notification dismissed by the user
DISMISSED = 2


class Server(dbus.service.Object):
    def __init__(self, bus: dbus.Bus) -> None:
        super().__init__(bus, PATH)
        self.next_id = 1

    @dbus.service.method(IFACE, in_signature="susssasa{sv}i", out_signature="u")
    def Notify(self, app_name, replaces_id, app_icon, summary, body, actions, hints, expire_timeout):
        nid = self.next_id
        self.next_id += 1
        print(json.dumps({"id": nid, "app": str(app_name), "summary": str(summary), "body": str(body),
                          "actions": [str(a) for a in actions]}), flush=True)
        return dbus.UInt32(nid)

    @dbus.service.method(IFACE, in_signature="u", out_signature="")
    def CloseNotification(self, nid):
        self.NotificationClosed(nid, 3)

    @dbus.service.method(IFACE, in_signature="", out_signature="as")
    def GetCapabilities(self):
        return ["actions", "body"]

    @dbus.service.method(IFACE, in_signature="", out_signature="ssss")
    def GetServerInformation(self):
        return ("fake", "kuterm-e2e", "1", "1.2")

    @dbus.service.signal(IFACE, signature="us")
    def ActionInvoked(self, nid, key):
        pass

    @dbus.service.signal(IFACE, signature="uu")
    def NotificationClosed(self, nid, reason):
        pass


def main() -> None:
    DBusGMainLoop(set_as_default=True)
    bus = dbus.SessionBus()
    server = Server(bus)
    _name = dbus.service.BusName(IFACE, bus, do_not_queue=True)
    loop = GLib.MainLoop()

    def on_stdin(source, condition) -> bool:
        line = sys.stdin.readline()
        if not line:
            loop.quit()
            return False
        parts = line.split()
        if parts and parts[0] == "invoke":
            nid = dbus.UInt32(int(parts[1]))
            server.ActionInvoked(nid, parts[2])
            server.NotificationClosed(nid, dbus.UInt32(DISMISSED))
        return True

    GLib.io_add_watch(sys.stdin, GLib.IO_IN | GLib.IO_HUP, on_stdin)
    print("ready", flush=True)
    loop.run()


if __name__ == "__main__":
    main()
