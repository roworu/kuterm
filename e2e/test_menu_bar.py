"""menu bar drawn in the window: the app menus open from it and run their actions, with every
window frame and with the tab bar below it"""

import numpy as np
import pytest

from harness import App, as_rgb, close_to, ink, near, unique_theme

FEATURE = "menu bar"

THEME = unique_theme(0x70)
C = as_rgb(THEME)


def menu_app(app_factory, decorations: str, hide: bool) -> App:
    settings = {"theme": {"mode": "dark"}, "window_title": ["number"], "hide_bar_for_one_tab": hide,
                "window": {"menu_bar": "always", "decorations": decorations}}
    app = app_factory(settings, themes={"dark": THEME})
    # keep the pointer away from the bar, hover changes its colors
    app.mouse(450, 500)
    return app


def titles(app: App, img) -> list:
    """(x0, x1) of every ink group in the bar, left to right: the menu titles first, then the
    window buttons of a "client" frame"""
    h = app.bar_height()
    columns = ink(img[2 : h - 2], C["tab_bar_background"]).any(axis=0)
    xs = np.flatnonzero(columns)
    # letters of one title sit closer together than the padding between titles
    groups = np.split(xs, np.flatnonzero(np.diff(xs) > 8 * app.scale) + 1)
    return [(int(g[0]), int(g[-1]) + 1) for g in groups if len(g)]


def dropdown(app: App, before, img) -> tuple | None:
    """bbox of the open menu below the bar, found by the side lines of its border: the tab bar
    under it has the same colors, so only border pixels that are new since it was closed count"""
    h = app.bar_height()
    changed = np.abs(img[h:].astype(int) - before[h:].astype(int)).max(axis=-1) > 0
    mask = changed & near(img[h:], C["border"])
    columns = mask.sum(axis=0)
    # its shadow can blend into the border color here and there, the sides are full lines
    if columns.max() < 3 * h:
        return None
    xs = np.flatnonzero(columns >= 0.8 * columns.max())
    ys = np.flatnonzero(mask[:, xs[0]])
    return int(xs[0]), int(ys[0]) + h, int(xs[-1]) + 1, int(ys[-1]) + 1 + h


@pytest.mark.parametrize("hide", [True, False], ids=["hide", "keep"])
@pytest.mark.parametrize("decorations", ["server", "none", "client"])
def test_menu_bar_runs_new_tab(app_factory, decorations, hide):
    """clicking "Shell" opens its menu over the tab bar and terminal, "New Tab" in it opens a
    second tab"""
    app = menu_app(app_factory, decorations, hide)
    app.pty_size()
    app.wait(lambda: len(titles(app, app.shot())) >= 5, msg="menu titles")
    img = app.shot()
    assert close_to(tuple(img[app.bar_height() - 1, 450]), C["border"])
    x0, x1 = titles(app, img)[1]
    app.click((x0 + x1) // 2, app.bar_height() // 2)
    box = app.wait(lambda: dropdown(app, img, app.shot()), msg="open menu")
    app.snap(f"shell menu open, decorations {decorations}")
    # the first ink line inside its border is "New Tab"
    menu = app.shot()[box[1] + 1 : box[3] - 1, box[0] + 1 : box[2] - 1]
    rows = np.flatnonzero(ink(menu, C["tab_bar_background"]).any(axis=1))
    first = rows[: np.argmax(np.diff(rows) > 1) + 1] if len(rows) > 1 else rows
    app.click(box[0] + round(30 * app.scale), box[1] + int(first.mean()))
    app.wait_title("2")
    app.snap("new tab from the menu")
