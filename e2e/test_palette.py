"""command palette: open and close, filtering, keys and clicks, recent and pinned commands, all actions"""

import json
import time

import numpy as np
import pytest

from harness import BASH, App, as_rgb, close_to, dominant, get_clipboard, mask_bbox, near, set_clipboard, unique_theme

FEATURE = "command palette"

THEME = unique_theme(0xc0)
C = as_rgb(THEME)


def write(name: str) -> dict:
    """command typing a marker line into the shell"""
    return {"name": name, "actions": [{"type": f"echo {name} >> $T2_TMP/ran\n"}]}


def palette_app(app_factory, commands: list, **settings) -> App:
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["number"], **settings},
                      themes={"dark": THEME}, commands={"commands": commands})
    app.mouse(450, 580)
    return app


def panel(app: App, img=None) -> tuple | None:
    """bbox of the palette panel, drawn in the tab bar color over the terminal"""
    img = app.shot() if img is None else img
    mask = near(img, C["tab_bar_background"], 2)
    # the tab bar has the panel color too, the panel always starts below it
    mask[: app.bar_height()] = False
    return mask_bbox(mask) if mask.sum() > 5000 else None


def divider(img, box: tuple) -> int:
    """y of the border colored line between the query and the list"""
    x0, y0, x1, y1 = box
    rows = np.flatnonzero(near(img[y0:y1, x0 + 5 : x1 - 5], C["border"]).mean(axis=1) > 0.9) + y0
    return int(rows[rows > y0 + 5].min())


def ran(app: App) -> list:
    path = app.file("ran")
    return path.read_text().split() if path.exists() else []


def test_open_and_escape(app_factory):
    """ctrl-shift-p opens the palette near the top, escape closes it"""
    app = palette_app(app_factory, [write("one")])
    app.key("ctrl+shift+p")
    box = app.wait(lambda: panel(app), msg="palette open")
    x0, y0, x1, _ = box
    # centered, 4 rems below the top
    assert abs((x0 + x1) / 2 - 450) <= 2 and 60 <= y0 <= 80, box
    app.snap("palette open")
    app.key("Escape")
    app.wait(lambda: panel(app) is None, msg="palette closed")


def test_same_key_toggles(app_factory):
    """the palette key closes an open palette"""
    app = palette_app(app_factory, [write("one")])
    app.key("ctrl+shift+p")
    app.wait(lambda: panel(app), msg="palette open")
    app.key("ctrl+shift+p")
    app.wait(lambda: panel(app) is None, msg="palette closed")


def test_disabled_palette_never_opens(app_factory):
    """command_palette.enable false ignores the key"""
    app = palette_app(app_factory, [write("one")], command_palette={"enable": False})
    app.key("ctrl+shift+p")
    time.sleep(1)
    assert panel(app) is None


def test_typing_filters_and_enter_runs(app_factory):
    """typing filters by name with fuzzy matching, enter runs the best match"""
    app = palette_app(app_factory, [write("alpha"), write("bravo"), write("charlie")])
    app.palette("brv", run=False)
    app.snap("query brv")
    app.key("Return")
    app.wait(lambda: ran(app) == ["bravo"], msg=f"bravo ran, got {ran(app)}")
    app.wait(lambda: panel(app) is None, msg="palette closed after running")


def test_arrows_pick_a_command(app_factory):
    """down and up move the selection, enter runs the selected command"""
    app = palette_app(app_factory, [write("item-a"), write("item-b"), write("item-c")])
    app.palette("item", run=False)
    app.key("Down", "Down", "Up")
    app.snap("second item selected")
    app.key("Return")
    app.wait(lambda: ran(app) == ["item-b"], msg=f"item-b ran, got {ran(app)}")


def test_arrows_wrap_around(app_factory):
    """up on the first command goes to the last one, down on the last goes to the first"""
    app = palette_app(app_factory, [write("item-a"), write("item-b"), write("item-c")],
                      command_palette={"show_recent": False})
    app.palette("item", run=False)
    app.key("Up")
    app.snap("up from the first item wrapped to the last")
    app.key("Return")
    app.wait(lambda: ran(app) == ["item-c"], msg=f"item-c ran, got {ran(app)}")
    app.palette("item", run=False)
    app.key("Down", "Down", "Down")
    app.key("Return")
    app.wait(lambda: ran(app) == ["item-c", "item-a"], msg=f"item-a ran, got {ran(app)}")


def test_no_matches(app_factory):
    """a query matching nothing shows an empty list, enter does nothing"""
    app = palette_app(app_factory, [write("alpha")])
    app.palette("zzzz", run=False)
    app.snap("no matching commands")
    app.key("Return")
    time.sleep(1)
    assert ran(app) == []


def test_click_runs_a_command(app_factory):
    """clicking a listed command runs it"""
    app = palette_app(app_factory, [write("clickme"), write("other")])
    app.palette("clickme", run=False)
    box = app.wait(lambda: panel(app), msg="palette")
    x0, _, x1, _ = box
    app.click((x0 + x1) // 2 - 100, divider(app.shot(), box) + 20)
    app.wait(lambda: ran(app) == ["clickme"], msg=f"clickme ran, got {ran(app)}")


def test_categories_show_before_names(app_factory):
    """a category is shown before the name and grouped, the query matches it too"""
    app = palette_app(app_factory, [{**write("one"), "category": "group"}, write("two")])
    app.palette("group:one", run=False)
    app.snap("category shown before the name")
    app.key("Return")
    app.wait(lambda: ran(app) == ["one"], msg=f"one ran, got {ran(app)}")


@pytest.mark.parametrize("show_recent", [True, False])
def test_recent_commands_first(app_factory, show_recent):
    """show_recent lists the last run command first, off keeps name order"""
    app = palette_app(app_factory, [write("aaa"), write("zzz")], command_palette={"show_recent": show_recent})
    app.palette("zzz")
    app.wait(lambda: ran(app) == ["zzz"], msg="zzz ran")
    app.palette("")
    expected = ["zzz", "zzz"] if show_recent else ["zzz", "aaa"]
    app.wait(lambda: ran(app) == expected, msg=f"{expected}, got {ran(app)}")


def test_pinned_commands_first(app_factory):
    """pinned commands stay at the very top"""
    app = palette_app(app_factory, [write("aaa"), {**write("zzz"), "pinned": True}])
    app.palette("", run=False)
    app.snap("pinned command on top")
    app.key("Return")
    app.wait(lambda: ran(app) == ["zzz"], msg=f"zzz ran, got {ran(app)}")


def test_click_on_pin_unpins_and_saves(app_factory):
    """clicking the pin of a pinned command unpins it and saves that choice"""
    app = palette_app(app_factory, [write("aaa"), {**write("zzz"), "pinned": True}])
    app.palette("", run=False)
    box = app.wait(lambda: panel(app), msg="palette")
    x1 = box[2]
    img = app.shot()
    top = divider(img, box)
    # the pin icon sits at the right end of the first row
    region = img[top + 4 : top + 36, x1 - 60 : x1 - 4]
    pin = mask_bbox(near(region, C["text_muted"], 40))
    assert pin is not None
    app.mouse(x1 - 60 + (pin[0] + pin[2]) // 2, top + 4 + (pin[1] + pin[3]) // 2)
    time.sleep(0.3)
    app.snap("hovering the pin shows the unpin icon")
    app.click(x1 - 60 + (pin[0] + pin[2]) // 2, top + 4 + (pin[1] + pin[3]) // 2)
    pins = app.home / ".local/state/kuterm/pinned_commands.json"
    app.wait(pins.exists, msg="pins saved")
    assert json.loads(pins.read_text())["overrides"] == {"zzz": False}


def test_tab_actions(app_factory):
    """new_tab, activate_tab, prev_tab, next_tab and close_tab actions"""
    app = palette_app(app_factory, [
        {"name": "three tabs", "actions": ["new_tab", "new_tab", {"activate_tab": 2}]},
        {"name": "go left", "actions": ["prev_tab"]},
        {"name": "go right", "actions": ["next_tab"]},
        {"name": "close it", "actions": ["close_tab"]},
    ])
    app.palette("three tabs")
    app.wait_title("2")
    app.palette("go left")
    app.wait_title("1")
    app.palette("go left")
    app.wait_title("3")
    app.palette("go right")
    app.wait_title("1")
    app.snap("three tabs, first one active")
    app.palette("close it")
    app.wait_title("1")
    app.palette("close it")
    app.wait_title("1")
    app.palette("close it")
    app.wait(lambda: not app.alive(), msg="last tab closed quits")


def test_pick_tab_action(app_factory):
    """pick_tab lists the open tabs in tab order, picking one switches to it"""
    app = palette_app(app_factory, [{"name": "three tabs", "actions": ["new_tab", "new_tab"]},
                                    {"name": "pick", "actions": ["pick_tab"]}])
    app.palette("three tabs")
    app.wait_title("3")
    app.palette("pick")
    h = app.bar_height()
    # the tab bar has the panel color too, so look below it
    box = app.wait(lambda: panel(app, app.shot()[h:]), msg="tab picker")
    img = app.shot()[h:]
    top = divider(img, box)
    # one row of text per tab below the query
    rows = np.flatnonzero(near(img[top + 2 : box[3] - 2, box[0] + 4 : box[2] - 4], C["text"], 40).any(axis=1))
    assert len(rows) and len(np.flatnonzero(np.diff(rows) > 4)) + 1 == 3, rows
    app.snap("tab picker listing three tabs")
    app.key("Down", "Return")
    app.wait_title("2")
    app.palette("pick")
    app.wait(lambda: panel(app, app.shot()[h:]), msg="tab picker again")
    app.key("Up", "Return")
    app.wait_title("3")


def test_type_action_in_new_tab(app_factory):
    """a command can open a tab and type into its fresh shell"""
    app = palette_app(app_factory, [{"name": "hello tab", "actions": [
        "new_tab", {"type": "echo hello > $T2_TMP/hello\n"}]}])
    app.palette("hello tab")
    app.wait_title("2")
    app.wait_file("hello", "hello")


def test_copy_and_paste_actions(app_factory):
    """copy puts the selection on the clipboard, paste types the clipboard"""
    app = palette_app(app_factory, [{"name": "copy it", "actions": ["copy"]},
                                    {"name": "paste it", "actions": ["paste"]}])
    set_clipboard(f"echo from clipboard > {app.file('pasted')}\n")
    app.palette("paste it")
    # bash turns on bracketed paste, so the pasted line waits for enter
    app.key("Return")
    app.wait_file("pasted", "from clipboard")
    app.run("clear; echo COPYTHIS")
    time.sleep(1)
    x0, y0, _, y1 = app.cell_rect(0, 0)
    _, _, x1, _ = app.cell_rect(7, 0)
    app.drag((x0 + 1, (y0 + y1) // 2), (x1 - 1, (y0 + y1) // 2))
    time.sleep(0.3)
    app.palette("copy it")
    app.wait(lambda: get_clipboard() == "COPYTHIS", msg=f"clipboard, got {get_clipboard()!r}")


def test_scroll_actions(app_factory):
    """scroll_top, scroll_bottom, scroll_up and scroll_down move the view"""
    app = palette_app(app_factory, [
        {"name": "to top", "actions": ["scroll_top"]}, {"name": "to bottom", "actions": ["scroll_bottom"]},
        {"name": "up some", "actions": [{"scroll_up": 20}]}, {"name": "down some", "actions": [{"scroll_down": 5}]},
    ], terminal={"scrollbar": {"enable": "on", "auto_hide": 0}, "smooth_scroll": {"enable": False}})
    app.run("clear; seq 1 300")
    time.sleep(1)

    def thumb():
        img = app.shot()
        return mask_bbox(near(img[:, 880:], C["scrollbar"], 2))

    app.wait(lambda: thumb() and thumb()[3] >= 598, msg="thumb at the bottom")
    app.palette("to top")
    app.wait(lambda: thumb() and thumb()[1] <= 2, msg=f"thumb at the top {thumb()}")
    app.snap("scrolled to the top")
    app.palette("to bottom")
    app.wait(lambda: thumb() and thumb()[3] >= 598, msg="thumb at the bottom again")
    app.palette("up some")
    app.wait(lambda: thumb() and thumb()[3] < 590, msg="scrolled up")
    up = thumb()
    app.palette("down some")
    app.wait(lambda: thumb() and up[3] < thumb()[3] < 590, msg="scrolled down a bit")


def test_about_page(app_factory):
    """the about action shows the version page until a key is pressed"""
    app = palette_app(app_factory, [{"name": "show about", "actions": ["about"]}])
    app.palette("show about")
    time.sleep(0.5)
    app.wait(lambda: panel(app), msg="about page")
    app.snap("about page")
    app.key("space")
    app.wait(lambda: panel(app) is None, msg="about closed")


def test_quit_action(app_factory):
    """quit closes every tab and exits"""
    app = palette_app(app_factory, [{"name": "bye", "actions": ["new_tab", "quit"]}])
    app.palette("bye", run=False)
    app.snap("quit command picked")
    app.key("Return")
    app.wait(lambda: not app.alive(), msg="app exit")


def test_profile_actions(app_factory):
    """new_tab_with_profile and new_background_tab_with_profile open the named profile"""
    app = palette_app(app_factory, [
        {"name": "open tmp", "actions": [{"new_tab_with_profile": "tmp"}]},
        {"name": "bg tmp", "actions": [{"new_background_tab_with_profile": "tmp"},
                                       {"type": "pwd > $T2_TMP/bg\n"}]},
        {"name": "missing", "actions": [{"new_tab_with_profile": "nope"}]},
    ], profiles=[{"name": "main", "command": BASH}, {"name": "tmp", "command": BASH, "working_directory": "/tmp"}],
        window_title=["number", {"text": " "}, "folder"])
    app.wait_title("1 ~")
    app.palette("open tmp")
    app.wait_title("2 tmp")
    app.palette("bg tmp")
    app.wait_file("bg", "/tmp")
    assert app.title() == "2 tmp"
    app.palette("missing")
    app.wait(lambda: near(app.shot()[400:, 450:], C["tab_bar_background"]).sum() > 300,
             msg="no such profile notification")
    app.snap("notification for a missing profile")


def test_bundled_commands_work_without_commands_section(app_factory):
    """without a "commands" section the bundled commands are listed, no commands.jsonc is written"""
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["number"]}, themes={"dark": THEME})
    app.palette("tabs: create new")
    app.wait_title("2")
    assert not (app.config / "commands.jsonc").exists()


def test_broken_commands_section_is_reported(app_factory):
    """an unknown action in the commands section prints an error naming the section, defaults are used"""
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["number"]},
                      commands={"commands": [{"name": "x", "actions": ["launch_rockets"]}]})
    app.wait(lambda: "invalid settings in" in app.output(), msg="error on stderr")
    assert "commands" in app.output() and "launch_rockets" in app.output()
    assert app.alive()


def test_background_tab_notifies_when_done(app_factory):
    """a background tab runs a slow command, its notification switches to it when clicked"""
    app = palette_app(app_factory, [{"name": "slow job", "actions": [
        "new_background_tab", {"type": "sleep 2\n"}, {"notify_when_done": "job finished"}]}],
        notifications={"timeout": 0})
    app.palette("slow job")
    time.sleep(0.5)
    assert app.title() == "1"

    def note():
        region = near(app.shot()[400:, 450:], C["tab_bar_background"])
        return mask_bbox(region) if region.sum() > 300 else None

    box = app.wait(note, msg="done notification", timeout=15)
    app.snap("job finished notification")
    x0, y0, x1, y1 = box
    app.click(450 + (x0 + x1) // 2, 400 + (y0 + y1) // 2)
    app.wait_title("2")
    app.wait(lambda: note() is None, msg="notification gone after click")
    assert close_to(dominant(app.shot()), C["terminal_background"])
