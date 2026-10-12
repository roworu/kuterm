"""close tabs to the right/left: the default palette commands, index handling, one confirmation"""

import time

import numpy as np

from harness import App, as_rgb, mask_bbox, near, unique_theme

FEATURE = "close tabs to the side"

THEME = unique_theme(0xC3)
C = as_rgb(THEME)
# active tab number and the foreground command, so the title tells which tab is on screen
TITLE = ["number", {"text": " "}, "command"]


def side_app(app_factory) -> App:
    """fixed tab width 200 so the tab count can be read from the bar, bundled commands used"""
    app = app_factory(
        {"theme": {"mode": "dark"}, "window_title": TITLE, "tab_width": 200},
        themes={"dark": THEME},
    )
    app.mouse(450, 500)
    return app


def open_tabs(app: App, n: int) -> None:
    """ctrl+shift+t until there are n tabs, the window title is the active tab number"""
    for i in range(2, n + 1):
        app.key("ctrl+shift+t")
        app.wait_title(f"{i} bash")
    app.wait(lambda: tab_count(app) == n, msg=f"{n} tabs")


def tab_count(app: App) -> int:
    """number of open tabs, read from the right border of each tab in the bar"""
    row = near(app.shot()[app.bar_height() // 2 : app.bar_height() // 2 + 1], C["border"])[0]
    return int(np.count_nonzero(row))


def dialog(app: App) -> tuple | None:
    """bbox of the danger colored "close" button, none while no confirmation is up"""
    mask = near(app.shot(), C["danger_button"])
    return mask_bbox(mask) if mask.sum() > 200 else None


def test_close_tabs_to_the_right_keeps_the_active_and_its_left_neighbors(app_factory):
    """the bundled command closes every tab right of the active one, active tab is kept"""
    app = side_app(app_factory)
    open_tabs(app, 4)
    app.key("alt+2")
    app.wait_title("2 bash")

    app.palette("close tabs to the right")
    app.wait(lambda: tab_count(app) == 2, msg="only two tabs left")
    app.snap("tabs to the right of tab 2 closed")
    app.wait_title("2 bash")

    # remaining tabs are the first two, next wraps from 2 straight back to 1
    app.key("ctrl+Tab")
    app.wait_title("1 bash")
    # tab 3 is gone, activating it does nothing
    app.key("alt+3")
    time.sleep(0.4)
    assert app.title() == "1 bash", app.title()


def test_close_tabs_to_the_left_keeps_the_active_and_its_right_neighbors(app_factory):
    """the bundled command closes every tab left of the active one, active tab becomes first"""
    app = side_app(app_factory)
    open_tabs(app, 4)
    app.key("alt+3")
    app.wait_title("3 bash")

    app.palette("close tabs to the left")
    app.wait(lambda: tab_count(app) == 2, msg="only two tabs left")
    app.snap("tabs to the left of tab 3 closed")
    # original tabs 3 and 4 survive, they are renumbered 1 and 2, the active one is 1
    app.wait_title("1 bash")

    app.key("alt+2")
    app.wait_title("2 bash")
    # no third tab is left
    app.key("alt+3")
    time.sleep(0.4)
    assert app.title() == "2 bash", app.title()


def test_closing_an_empty_side_does_nothing(app_factory):
    """no tab on that side means the command is a no-op and opens no dialog"""
    app = side_app(app_factory)
    open_tabs(app, 2)

    app.palette("close tabs to the right")
    time.sleep(0.5)
    assert tab_count(app) == 2
    assert dialog(app) is None
    app.key("alt+1")
    app.wait_title("1 bash")

    app.palette("close tabs to the left")
    time.sleep(0.5)
    assert tab_count(app) == 2
    assert dialog(app) is None


def test_close_side_asks_once_for_all_running_programs(app_factory):
    """tabs closing together are asked about once, the dialog closes all of them on enter"""
    app = side_app(app_factory)
    open_tabs(app, 3)
    # tab 2 keeps a program running, tabs 1 and 3 are idle shells
    app.key("alt+2")
    app.wait_title("2 bash")
    app.run("sleep 1000")
    app.wait_title("2 sleep")
    app.key("alt+1")
    app.wait_title("1 bash")

    app.palette("close tabs to the right")
    app.wait(lambda: dialog(app), msg="close confirmation for the busy side")
    # nothing is closed before the dialog is answered
    assert tab_count(app) == 3
    assert app.title() == "1 bash", app.title()
    app.snap("one confirmation for both tabs on the right")
    app.key("Escape")
    app.wait(lambda: dialog(app) is None, msg="dialog closed")
    assert tab_count(app) == 3

    app.palette("close tabs to the right")
    app.wait(lambda: dialog(app), msg="close confirmation again")
    app.key("Return")
    # the bar hides with a single tab, so zero borders means the active tab is all that is left
    app.wait(lambda: tab_count(app) == 0, msg="only the active tab left")
    app.wait_title("1 bash")


def test_one_tab_exiting_keeps_the_batch_dialog(app_factory):
    """a tab on the closing side exiting on its own still lets the rest close on enter"""
    app = side_app(app_factory)
    open_tabs(app, 3)
    # tab 2 keeps a program running and exits on its own while the dialog is up
    app.key("alt+2")
    app.wait_title("2 bash")
    app.run("sleep 3; exit")
    app.wait_title("2 sleep")
    app.key("alt+1")
    app.wait_title("1 bash")

    app.palette("close tabs to the right")
    app.wait(lambda: dialog(app), msg="close confirmation for the busy side")
    # let tab 2 leave by itself
    app.wait(lambda: tab_count(app) == 2, msg="tab 2 exited")
    assert dialog(app) is not None, "the batch dialog was dismissed with the tab that exited"
    app.snap("dialog still up after the first tab exited")
    app.key("Return")
    app.wait(lambda: tab_count(app) == 0, msg="the idle tab 3 was closed too")
