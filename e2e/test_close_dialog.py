"""close dialog: closing a tab or quitting with a running program asks first, keys and clicks answer it"""

import time

import numpy as np
import pytest

from harness import App, as_rgb, bundled_theme, count, ink, mask_bbox, near, sh, unique_theme, wait_until
from test_safety import gone

FEATURE = "close tab dialog"

THEME = unique_theme(0x70)
C = as_rgb(THEME)


def busy_app(app_factory, program: str = "sleep 1000", **settings) -> App:
    """two tabs, the active second one running program in the foreground"""
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["number", {"text": " "}, "command"],
                       "tab_width": 200, "tab_title": [{"text": "title"}], **settings},
                      themes={"dark": THEME})
    app.mouse(450, 580)
    app.wait_title("1 bash")
    app.key("ctrl+shift+t")
    app.wait_title("2 bash")
    app.run(program)
    app.wait_title(f"2 {program.split()[0]}")
    return app


def dialog(app: App) -> tuple | None:
    """bbox of the "close" button, drawn in the danger color, none while the dialog is closed"""
    mask = near(app.shot(), C["danger_button"])
    return mask_bbox(mask) if mask.sum() > 200 else None


def cancel_selected(app: App) -> bool:
    """the "cancel" button is filled like the active tab once it is the one enter presses"""
    img = app.shot()[app.bar_height() + 10 :]
    return count(img, C["tab_active_background"]) > 200


def ask(app: App) -> tuple:
    """press the close tab key and wait for the dialog, returns the "close" button bbox"""
    app.key("ctrl+shift+w")
    return app.wait(lambda: dialog(app), msg="close tab dialog")


def kept(app: App) -> None:
    """the dialog closed and the busy tab is still open"""
    app.wait(lambda: dialog(app) is None, msg="dialog closed")
    time.sleep(0.5)
    assert app.title() == "2 sleep"


def test_running_tab_asks_before_closing(app_factory):
    """closing a tab with a running program shows the dialog, escape keeps the tab, enter closes it"""
    app = busy_app(app_factory)
    x0, y0, x1, _ = ask(app)
    # centered near the top like the palette, "close" at the right end of the panel
    assert x1 > 450 and 60 <= y0 <= 200, (x0, y0, x1)
    assert not cancel_selected(app)
    app.snap("dialog asking to close the tab running sleep")
    app.key("Escape")
    kept(app)
    app.snap("escape kept the tab")
    ask(app)
    app.key("Return")
    app.wait_title("1 bash")
    assert dialog(app) is None


def test_arrows_pick_the_button_enter_presses(app_factory):
    """left selects "cancel", right selects "close" again"""
    app = busy_app(app_factory)
    ask(app)
    app.key("Left")
    app.wait(lambda: cancel_selected(app), msg="cancel selected")
    app.snap("cancel selected")
    app.key("Return")
    kept(app)
    ask(app)
    app.key("Left", "Right")
    app.wait(lambda: not cancel_selected(app), msg="close selected")
    app.key("Return")
    app.wait_title("1 bash")


@pytest.mark.parametrize("key, closes", [("y", True), ("n", False)])
def test_y_and_n_answer(app_factory, key, closes):
    """y closes the tab, n keeps it"""
    app = busy_app(app_factory)
    ask(app)
    app.key(key)
    app.wait(lambda: dialog(app) is None, msg="dialog closed")
    if closes:
        app.wait_title("1 bash")
    else:
        time.sleep(0.5)
        assert app.title() == "2 sleep"


def test_click_on_close_button(app_factory):
    """clicking "close" closes the tab"""
    app = busy_app(app_factory)
    x0, y0, x1, y1 = ask(app)
    app.click((x0 + x1) // 2, (y0 + y1) // 2)
    app.wait_title("1 bash")


def test_click_outside_cancels(app_factory):
    """a click outside the dialog keeps the tab"""
    app = busy_app(app_factory)
    ask(app)
    app.click(450, 550)
    kept(app)


@pytest.mark.parametrize("button", [1, 2])
def test_tab_bar_closing_asks_too(app_factory, button):
    """the x of the tab and a middle click on it ask like the key does"""
    app = busy_app(app_factory)
    h = app.bar_height()
    if button == 1:
        # the active second tab shows its x at its right end
        box = mask_bbox(ink(app.shot()[1 : h - 2, 350:398], C["tab_active_background"]))
        assert box is not None
        app.click(350 + (box[0] + box[2]) // 2, 1 + (box[1] + box[3]) // 2)
    else:
        app.click(300, h // 2, button=2)
    app.wait(lambda: dialog(app), msg="close tab dialog")
    app.snap("asked after a click on the tab bar")
    app.key("Return")
    app.wait_title("1 bash")


def test_keys_do_not_reach_the_program(app_factory):
    """keys typed while the dialog is open are not sent to the tab behind it"""
    app = busy_app(app_factory, program="cat > $T2_TMP/typed")
    ask(app)
    app.type("abc")
    app.snap("typed abc while asked")
    app.key("Escape")
    app.wait(lambda: dialog(app) is None, msg="dialog closed")
    # nothing typed reached cat, so one ctrl-d ends it. a second one would exit bash too
    app.key("ctrl+d")
    app.wait_title("2 bash")
    assert app.file("typed").read_text() == ""


def test_dialog_closes_when_the_program_tab_exits(app_factory):
    """a tab whose shell exits while asked about closes, and the dialog with it"""
    app = busy_app(app_factory, program="sleep 4; exit")
    ask(app)
    app.wait_title("1 bash")
    app.wait(lambda: dialog(app) is None, msg="dialog closed with its tab")


def test_disabled_warning_closes_at_once(app_factory):
    """close_running_tab_warn false closes a busy tab without asking"""
    app = busy_app(app_factory, close_running_tab_warn=False)
    app.key("ctrl+shift+w")
    app.wait_title("1 bash")
    assert dialog(app) is None


def program_pid(program: str) -> int:
    """pid of the only process running exactly `program`"""
    out = sh("pgrep", "-xf", program, check=False).split()
    assert len(out) == 1, f"{program!r} runs {len(out)} times"
    return int(out[0])


def quit_asked(app: App, how: str) -> None:
    """ask to quit through the palette or the window manager, and wait for the dialog"""
    if how == "palette":
        app.palette("app quit")
    else:
        app.wm_close()
    app.wait(lambda: dialog(app), msg="quit dialog")


@pytest.mark.parametrize("how", ["palette", "window"])
def test_quit_asks_with_running_program(app_factory, how):
    """quitting with the palette or closing the window asks first, escape keeps the app, enter quits"""
    app = busy_app(app_factory, program="sleep 1001")
    pid = program_pid("sleep 1001")
    quit_asked(app, how)
    app.snap(f"asked before quitting ({how}) with sleep running")
    app.key("Escape")
    kept(app)
    assert app.alive()
    # the window manager did not unmap the window
    assert app._find_window() == app.wid
    quit_asked(app, how)
    app.key("Return")
    assert app.exit_code() == 0
    wait_until(lambda: gone(pid), msg="program ended with the app")


@pytest.mark.parametrize("how", ["palette", "window"])
def test_idle_quit_asks_nothing(app_factory, how):
    """with only shells in the tabs the app quits at once, ending the shells"""
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["number"]}, themes={"dark": THEME})
    app.run(f"echo $$ > {app.file('shell')}")
    shell = int(app.wait_file("shell"))
    if how == "palette":
        app.palette("app quit")
    else:
        app.wm_close()
    assert app.exit_code() == 0
    wait_until(lambda: gone(shell), msg="shell ended")


@pytest.mark.parametrize("how", ["palette", "window"])
def test_disabled_warning_quits_at_once(app_factory, how):
    """close_running_tab_warn false quits a busy app without asking"""
    app = busy_app(app_factory, program="sleep 1001", close_running_tab_warn=False)
    pid = program_pid("sleep 1001")
    if how == "palette":
        app.palette("app quit")
    else:
        app.wm_close()
    assert app.exit_code() == 0
    wait_until(lambda: gone(pid), msg="program ended with the app")


def panel(app: App) -> np.ndarray:
    """the dialog above its buttons, the panel is filled with the tab bar color"""
    img = app.shot()
    top = app.bar_height() + 2
    x0, y0, x1, _ = mask_bbox(near(img[top:], C["tab_bar_background"]))
    buttons = dialog(app)
    return img[top + y0 + 2 : buttons[1] - 4, x0 + 2 : x1 - 2]


def test_dialog_names_inactive_tab(app_factory):
    """a middle click on a busy tab that is not active asks about that tab and closes it"""
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["number", {"text": " "}, "command"],
                       "tab_width": 200, "tab_title": [{"text": "tab "}, "number"]},
                      themes={"dark": THEME})
    app.mouse(450, 580)
    app.key("ctrl+shift+t")
    app.wait_title("2 bash")
    app.run("sleep 1001")
    app.wait_title("2 sleep")
    pid = program_pid("sleep 1001")
    app.key("ctrl+shift+t")
    app.wait_title("3 bash")

    app.click(300, app.bar_height() // 2, button=2)
    app.wait(lambda: dialog(app), msg="close tab dialog")
    app.snap('asked about tab 2 "tab 2" while tab 3 is active')
    asked_inactive = panel(app)
    app.key("Escape")
    app.wait(lambda: dialog(app) is None, msg="dialog closed")
    assert app.title() == "3 bash"

    # the same question asked from the tab itself reads the same, naming tab 2 both times
    app.key("alt+2")
    app.wait_title("2 sleep")
    ask(app)
    assert np.array_equal(panel(app), asked_inactive)
    app.key("Escape")
    app.wait(lambda: dialog(app) is None, msg="dialog closed")
    app.key("alt+3")
    app.wait_title("3 bash")

    app.click(300, app.bar_height() // 2, button=2)
    app.wait(lambda: dialog(app), msg="close tab dialog")
    app.key("Return")
    # the active tab moved left into the closed tab's place
    app.wait_title("2 bash")
    wait_until(lambda: gone(pid), msg="program of the closed tab ended")


def test_modified_keys_do_not_answer(app_factory):
    """ctrl+y and ctrl+n are not answers, the dialog stays open"""
    app = busy_app(app_factory)
    ask(app)
    app.key("ctrl+y", "ctrl+n", "ctrl+Return", "alt+y")
    time.sleep(0.5)
    assert dialog(app) is not None
    assert app.title() == "2 sleep"
    app.snap("still asked after ctrl+y and ctrl+n")


def test_new_dialog_does_not_stack(app_factory):
    """the close tab key while asked does not open a second dialog, one escape closes it"""
    app = busy_app(app_factory)
    ask(app)
    app.key("ctrl+shift+w")
    time.sleep(0.3)
    app.key("Escape")
    kept(app)


def test_palette_replaces_dialog(app_factory):
    """the palette key while asked shows the palette instead, and keeps the tab"""
    app = busy_app(app_factory)
    ask(app)
    app.key("ctrl+shift+p")
    app.wait(lambda: dialog(app) is None, msg="dialog replaced")
    img = app.shot()[app.bar_height() + 2 :]
    assert count(img, C["tab_bar_background"]) > 5000, "no palette"
    app.snap("palette replaced the dialog")
    app.key("Escape")
    time.sleep(0.3)
    assert app.title() == "2 sleep"


def test_dialog_light_theme(app_factory):
    """in the bundled light theme the close button uses its danger color"""
    light = bundled_theme(False)
    app = app_factory({"theme": {"mode": "light"}, "window_title": ["number", {"text": " "}, "command"]})
    app.key("ctrl+shift+t")
    app.wait_title("2 bash")
    app.run("sleep 1000")
    app.wait_title("2 sleep")
    app.key("ctrl+shift+w")
    app.wait(lambda: count(app.shot(), light["danger_button"]) > 200, msg="close button in the light danger color")
    app.snap("close dialog in the light theme")
