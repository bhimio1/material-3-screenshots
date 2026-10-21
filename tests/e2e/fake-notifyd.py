#!/usr/bin/env python3
"""A stand-in notification server for the end-to-end tests.

Logs every Notify call as a JSON line to $STUB_DIR/notifications.log, and
then, depending on $FAKE_ACTION, presses that action button ("close"
dismisses the notification instead, "none" does nothing).
Needs dbus-python and PyGObject.
"""
import json, os, sys
import dbus, dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

LOG = os.path.join(os.environ["STUB_DIR"], "notifications.log")
ACTION = os.environ.get("FAKE_ACTION", "close")


class Server(dbus.service.Object):
    def __init__(self, bus):
        super().__init__(bus, "/org/freedesktop/Notifications")
        self.next_id = 1

    @dbus.service.method("org.freedesktop.Notifications", in_signature="susssasa{sv}i", out_signature="u")
    def Notify(self, app, replaces, icon, summary, body, actions, hints, timeout):
        nid = self.next_id
        self.next_id += 1
        keys = [str(a) for a in actions[0::2]]
        with open(LOG, "a") as f:
            f.write(json.dumps({"id": nid, "app": str(app), "summary": str(summary), "body": str(body),
                                "icon": str(icon), "actions": keys,
                                "image": str(hints.get("image-path", ""))}) + "\n")
        if ACTION == "close":
            GLib.timeout_add(300, lambda: (self.NotificationClosed(nid, 2), False)[1])
        elif ACTION in keys:
            GLib.timeout_add(300, lambda: (self.ActionInvoked(nid, ACTION), False)[1])
        return nid

    @dbus.service.method("org.freedesktop.Notifications", in_signature="u", out_signature="")
    def CloseNotification(self, nid):
        with open(LOG, "a") as f:
            f.write(json.dumps({"closed": int(nid)}) + "\n")

    @dbus.service.method("org.freedesktop.Notifications", in_signature="", out_signature="as")
    def GetCapabilities(self):
        return ["actions", "body", "body-images"]

    @dbus.service.method("org.freedesktop.Notifications", in_signature="", out_signature="ssss")
    def GetServerInformation(self):
        return ("fake-notifyd", "m3", "1", "1.2")

    @dbus.service.signal("org.freedesktop.Notifications", signature="us")
    def ActionInvoked(self, nid, key):
        pass

    @dbus.service.signal("org.freedesktop.Notifications", signature="uu")
    def NotificationClosed(self, nid, reason):
        pass


DBusGMainLoop(set_as_default=True)
bus = dbus.SessionBus()
name = dbus.service.BusName("org.freedesktop.Notifications", bus)
Server(bus)
print("ready", flush=True)
GLib.MainLoop().run()
