"""window settings: padding, startup size and state, remembered size, frame and opacity"""

import json
import math

import numpy as np
import pytest

from harness import App, bundled_theme, close_to, ink, sh, session_env

FEATURE = "window"

DARK = bundled_theme(True)
RED = DARK["ansi"][1]

# a 10 cells wide red block on the first two rows
RED_BLOCK = "printf '\\033[41m          \\033[0m\\n\\033[41m          \\033[0m\\n'\n"


def red_box(app: App) -> tuple:
    """(x0, y0, x1, y1) of the red block on screen"""
    app.wait(lambda: close_to(app.cells_color(0, 0, 10), RED, 3), msg="red block")
    red = (abs(app.shot().astype(int) - RED).max(axis=-1) <= 3)
    ys, xs = red.nonzero()
    return xs.min(), ys.min(), xs.max() + 1, ys.max() + 1


@pytest.mark.parametrize("x,y", [(0, 0), (40, 30)])
def test_padding_moves_the_grid(app_factory, x, y):
    """window.padding keeps the grid that far from the window edges"""
    app = app_factory({"theme": {"mode": "dark"}, "window": {"padding": {"x": x, "y": y}}}, script=RED_BLOCK)
    x0, y0, x1, y1 = app.cell_rect(0, 0, 10, 2)
    box = red_box(app)
    app.snap(f"padding x {x}, y {y}")
    assert all(abs(a - b) <= 1 for a, b in zip(box, (x0, y0, x1, y1))), (box, (x0, y0, x1, y1))


def test_padding_shrinks_the_pty(app_factory):
    """the shell gets the rows and columns that fit inside the padding"""
    app = app_factory({"window": {"padding": {"x": 60, "y": 45}}})
    assert app.pty_size() == app.expected_pty(900, 600)


def test_balanced_padding_centers_the_grid(app_factory):
    """padding_balance splits the room left by whole cells between both sides"""
    pad = 10
    app = app_factory({"theme": {"mode": "dark"}, "terminal": {"scrollbar": {"enable": "off"}},
                       "window": {"padding": {"x": pad, "y": pad}, "padding_balance": True}}, script=RED_BLOCK)
    cw, lh = app._cell_width(), app._line_height()
    spare_x = (900 - 2 * pad) % cw
    spare_y = (600 - 2 * pad) % lh
    x0, y0, _, _ = red_box(app)
    app.snap("balanced padding")
    assert abs(x0 - math.floor(pad + spare_x / 2)) <= 1, (x0, spare_x)
    assert abs(y0 - math.floor(pad + spare_y / 2)) <= 1, (y0, spare_y)


def test_window_opens_with_columns_and_rows(app_factory):
    """without a remembered size the window fits window.columns by window.rows cells"""
    app = app_factory({"window": {"remember_size": False, "columns": 70, "rows": 20}})
    app.snap("70 x 20 cells")
    assert app.pty_size() == (20, 70)


def test_remember_size_saves_the_last_size(app_factory):
    """window.remember_size writes the size the window was resized to"""
    app = app_factory({"window": {"remember_size": True}})
    path = app.home / ".local/state/kuterm/window_size.json"
    app.resize(700, 450)
    app.wait(lambda: json.loads(path.read_text()) == {"width": 700.0, "height": 450.0}, msg="saved size")
    restarted = app_factory({"window": {"remember_size": True}}, start=False)
    restarted.write(str(restarted.home / ".local/state/kuterm/window_size.json"), path.read_text())
    restarted.start()
    restarted.wait(lambda: restarted.size() == (700, 450), msg="remembered size")


def test_startup_fullscreen(app_factory):
    """window.startup "fullscreen" opens the window fullscreen"""
    app = app_factory({"window": {"startup": "fullscreen"}})
    app.wait(app.fullscreen, msg="fullscreen window")


@pytest.mark.x11_only
def test_startup_maximized(app_factory):
    """window.startup "maximized" opens the window maximized"""
    app = app_factory({"window": {"startup": "maximized"}})

    def maximized() -> bool:
        state = sh("xprop", "-id", app.wid, "_NET_WM_STATE", env=session_env())
        return "_NET_WM_STATE_MAXIMIZED_VERT" in state and "_NET_WM_STATE_MAXIMIZED_HORZ" in state

    app.wait(maximized, msg="maximized window")


@pytest.mark.parametrize("decorations", ["none", "client"])
def test_frameless_window_keeps_the_tab_bar(app_factory, decorations):
    """without a frame the top row stays shown with one tab, so the window can be moved: the tab
    bar for "none", an empty title bar with the buttons for "client"."""
    app = app_factory({"theme": {"mode": "dark"}, "hide_bar_for_one_tab": True,
                       "window": {"decorations": decorations}})
    app.snap(f"decorations {decorations}")
    assert app.pty_size() == app.expected_pty(900, 600, top=app.bar_height())


@pytest.mark.parametrize("menu_bar", ["never", "always"])
def test_client_close_button_quits(app_factory, menu_bar):
    """the close button drawn with "client" decorations quits like the window manager's, it sits
    on the top row, which is the menu bar when it is shown"""
    app = app_factory({"theme": {"mode": "dark"},
                       "window": {"decorations": "client", "menu_bar": menu_bar}})
    app.pty_size()
    app.snap(f"client decorations, menu bar {menu_bar}")
    # the close button is the last one at the right end of the top row, as high as the tab bar
    app.click(round((900 - 18) * app.scale), app.bar_height() // 2)
    assert app.exit_code() == 0


def window_buttons(app: App) -> list:
    """(x, y) centers of the client drawn buttons, left to right: fullscreen, minimize, maximize
    and close"""
    h = app.bar_height()
    columns = np.flatnonzero(ink(app.shot()[2 : h - 2], DARK["tab_bar_background"]).any(axis=0))
    # each button pads its glyph wider than the gaps inside one glyph
    groups = np.split(columns, np.flatnonzero(np.diff(columns) > 8 * app.scale) + 1)
    return [((int(g[0]) + int(g[-1])) // 2, h // 2) for g in groups[-4:]]


def test_client_fullscreen_button(app_factory):
    """the fullscreen button of "client" decorations goes fullscreen and back"""
    app = app_factory({"theme": {"mode": "dark"}, "window": {"decorations": "client"}})
    app.pty_size()
    app.mouse(450, 300)
    app.snap("client decorations with their buttons")
    app.click(*window_buttons(app)[-4])
    app.wait(app.fullscreen, msg="fullscreen")
    app.mouse(450, 300)
    # the buttons moved with the right edge
    app.click(*app.wait(lambda: window_buttons(app)[-4], msg="buttons"))
    app.wait(lambda: not app.fullscreen(), msg="windowed again")


@pytest.mark.wayland_only
def test_opacity_shows_through_the_background(app_factory):
    """window.opacity makes the default background see through, colored cells stay solid"""
    app = app_factory({"theme": {"mode": "dark"}, "window": {"opacity": 0.2}}, script=RED_BLOCK)
    red_box(app)
    app.snap("opacity 0.2")
    img = app.shot()
    x0, y0, x1, y1 = app.cell_rect(20, 5, 5, 1)
    assert not close_to(tuple(img[(y0 + y1) // 2, (x0 + x1) // 2]), DARK["terminal_background"], 3)
    assert close_to(app.cells_color(0, 0, 10), RED, 3)
