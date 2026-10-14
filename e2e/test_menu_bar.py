"""menu bar drawn in the window: the app menus open from it and run their actions"""

import numpy as np

from harness import App, as_rgb, close_to, ink, mask_bbox, near, unique_theme

FEATURE = "menu bar"

THEME = unique_theme(0x70)
C = as_rgb(THEME)


def menu_app(app_factory) -> App:
    # a frame of its own, so no window buttons sit next to the menus
    settings = {"theme": {"mode": "dark"}, "window_title": ["number"],
                "window": {"menu_bar": "always", "decorations": "server"}}
    app = app_factory(settings, themes={"dark": THEME})
    # keep the pointer away from the bar, hover changes its colors
    app.mouse(450, 500)
    return app


def titles(app: App, img) -> list:
    """(x0, x1) of every menu title in the bar, left to right"""
    h = app.bar_height()
    columns = ink(img[2 : h - 2], C["tab_bar_background"]).any(axis=0)
    xs = np.flatnonzero(columns)
    # letters of one title sit closer together than the padding between titles
    groups = np.split(xs, np.flatnonzero(np.diff(xs) > 8 * app.scale) + 1)
    return [(int(g[0]), int(g[-1]) + 1) for g in groups if len(g)]


def dropdown(app: App, img) -> tuple | None:
    """bbox of the open menu below the bar"""
    h = app.bar_height()
    box = mask_bbox(near(img[h:], C["tab_bar_background"]))
    return box and (box[0], box[1] + h, box[2], box[3] + h)


def test_menu_bar_runs_new_tab(app_factory):
    """clicking "Shell" opens its menu, "New Tab" in it opens a second tab"""
    app = menu_app(app_factory)
    app.wait(lambda: len(titles(app, app.shot())) == 5, msg="menu titles")
    img = app.shot()
    assert close_to(tuple(img[app.bar_height() - 1, 450]), C["border"])
    names = titles(app, img)
    x0, x1 = names[1]
    app.click((x0 + x1) // 2, app.bar_height() // 2)
    box = app.wait(lambda: dropdown(app, app.shot()), msg="open menu")
    app.snap("shell menu open")
    # the first ink line in the menu is "New Tab"
    img = app.shot()
    rows = np.flatnonzero(ink(img[box[1] : box[3], box[0] : box[2]], C["tab_bar_background"]).any(axis=1))
    first = rows[: np.argmax(np.diff(rows) > 1) + 1] if len(rows) > 1 else rows
    app.click(box[0] + round(30 * app.scale), box[1] + int(first.mean()))
    app.wait_title("2")
    app.snap("new tab from the menu")
