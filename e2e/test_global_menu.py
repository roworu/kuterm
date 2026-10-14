"""global menu: the menus offered over dbus for panel applets, found through window properties"""

import dbus
import pytest

from harness import App, Session, session_env, sh

# applets find the menus through x11 window properties, wayland has none of them yet
pytestmark = pytest.mark.x11_only

FEATURE = "global menu"

SETTINGS = {"theme": {"mode": "dark"}, "window_title": ["number"]}


def window_property(app: App, name: str) -> str:
    """utf8 string property of the app window, empty when unset"""
    out = sh("xprop", "-id", app.wid, name, env=session_env())
    return out.split(" = ", 1)[1].strip().strip('"') if " = " in out else ""


def exported(app: App):
    """(bus name, menubar object, app actions object) the window points applets at"""
    names = ["_GTK_UNIQUE_BUS_NAME", "_GTK_MENUBAR_OBJECT_PATH", "_GTK_APPLICATION_OBJECT_PATH"]
    bus_name, menubar, actions = (window_property(app, name) for name in names)
    bus = dbus.bus.BusConnection(Session.bus_address)
    return (
        bus_name,
        dbus.Interface(bus.get_object(bus_name, menubar), "org.gtk.Menus"),
        dbus.Interface(bus.get_object(bus_name, actions), "org.gtk.Actions"),
    )


def entries(menus) -> dict:
    """every menu of group 0 by id, as lists of plain dicts"""
    return {int(menu): [dict(entry) for entry in items] for group, menu, items in menus.Start([0])}


def items(model: dict, menu: int) -> list:
    """the items of a menu, its sections flattened"""
    found = []
    for entry in model[menu]:
        if ":section" in entry:
            found += items(model, int(entry[":section"][1]))
        else:
            found.append(entry)
    return found


def test_menus_are_exported_and_run_actions(app_factory):
    """the window names the bus and objects, the menus list kuterm's items with keys, activating
    "New Tab" opens a tab"""
    app = app_factory(SETTINGS)
    bus_name, menus, actions = app.wait(lambda: exported(app), msg="window properties")
    assert bus_name.startswith(":")
    model = entries(menus)
    bar = [str(entry["label"]) for entry in model[0]]
    assert bar[:2] == ["kuterm", "Shell"]
    shell = items(model, int(model[0][1][":submenu"][1]))
    new_tab = next(entry for entry in shell if str(entry["label"]) == "New Tab")
    # its key comes from the keymap, written the way gtk writes accelerators
    assert str(new_tab["accel"]).startswith("<")
    every = [str(entry.get("label")) for menu in model for entry in items(model, menu)]
    assert "Hide kuterm" not in every
    action = str(new_tab["action"])
    assert action.startswith("app.")
    assert action[4:] in [str(name) for name in actions.List()]
    actions.Activate(action[4:], dbus.Array([], signature="v"), dbus.Dictionary({}, signature="sv"))
    app.wait_title("2")
    app.snap("new tab from the global menu")
