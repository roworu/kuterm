"""synced scroll: panes joined to the sync scroll together by the same lines, each stopping at
the top of its own history; a pane left out scrolls alone"""

import time

import numpy as np

from harness import App, as_rgb, ink, unique_theme
from kit import SOCKET, listening, msg, tabs

FEATURE = "synced scroll"

THEME = unique_theme(0x97)
C = as_rgb(THEME)
KEYS = {"split_right": "ctrl-shift-r", "toggle_scroll_sync": "ctrl-shift-y", "focus_pane_left": "ctrl-shift-left",
        "focus_pane_right": "ctrl-shift-right"}


def two_panes(app_factory, left: int = 200, right: int = 200) -> App:
    """two panes side by side, each printed numbers 1 to n"""
    # undimmed, so both panes have the same background
    app = app_factory({**SOCKET, "hide_bar_for_one_tab": False, "window_title": ["number"],
                       "panes": {"dim_inactive": 0}},
                      themes={"dark": THEME}, keybindings=KEYS)
    listening(app)
    app.key("ctrl+shift+t")
    app.wait_title("2")
    app.key("ctrl+shift+r")
    app.wait(lambda: len(tabs(app)) == 2 and len({t["pane"] for t in tabs(app)}) == 2, msg="two panes")
    left_id, right_id = (str(t["id"]) for t in sorted(tabs(app), key=lambda t: t["pane"]))
    for pane, n in ((left_id, left), (right_id, right)):
        msg(app, "send-text", "--pane", pane, f"clear; seq 1 {n}\n")
    time.sleep(1.5)
    app.mouse(10, 10)
    return app


def pane_text(app: App, side: str) -> np.ndarray:
    """the numbers shown at the top left of one pane, as pixels"""
    img = app.shot()
    h, w = img.shape[:2]
    x0 = 0 if side == "left" else w // 2 + 1
    # each pane's grid starts at its own left edge plus the padding
    left = x0 + np.flatnonzero(ink(img[app.bar_height() + 4 : h - 40, x0 : x0 + 60], C["terminal_background"]).any(axis=0))[0]
    return img[app.bar_height() + 4 : h - 60, left : left + 40]


def profile(app: App, side: str) -> np.ndarray:
    return pane_text(app, side)


def same(app: App) -> bool:
    return bool(np.array_equal(pane_text(app, "left"), pane_text(app, "right")))


def join_both(app: App) -> None:
    w, h = app.size()
    for x in (w // 4, w * 3 // 4):
        app.click(x, h // 2)
        time.sleep(0.3)
        app.key("ctrl+shift+y")
        time.sleep(0.3)


def test_synced_panes_scroll_together(app_factory):
    app = two_panes(app_factory)
    app.wait(lambda: same(app), msg="both panes show the same numbers")
    join_both(app)
    w, h = app.size()
    app.wheel(w // 4, h // 2, up=True, clicks=8)
    app.wait(lambda: same(app), msg="both scrolled up alike")
    app.snap("both panes scrolled up the same lines, each marked synced")
    app.wheel(w * 3 // 4, h // 2, up=False, clicks=3)
    time.sleep(0.5)
    app.wait(lambda: same(app), msg="the right pane leads back down, the left follows")


def test_a_pane_left_out_scrolls_alone(app_factory):
    app = two_panes(app_factory)
    app.wait(lambda: same(app), msg="same at start")
    w, h = app.size()
    app.wheel(w // 4, h // 2, up=True, clicks=8)
    time.sleep(1)
    assert not same(app)
    app.snap("only the left pane scrolled, nothing is synced")


def test_one_pane_alone_is_no_sync(app_factory):
    app = two_panes(app_factory)
    w, h = app.size()
    app.click(w // 4, h // 2)
    app.key("ctrl+shift+y")
    app.wheel(w // 4, h // 2, up=True, clicks=8)
    time.sleep(1)
    assert not same(app)


def test_leaving_the_sync_stops_following(app_factory):
    app = two_panes(app_factory)
    join_both(app)
    w, h = app.size()
    # the right pane leaves again
    app.click(w * 3 // 4, h // 2)
    app.key("ctrl+shift+y")
    time.sleep(0.3)
    app.wheel(w // 4, h // 2, up=True, clicks=8)
    time.sleep(1)
    assert not same(app)


