"""window frames with the menu bar and tab bar: which rows sit above the grid, where the window
buttons go, that they work and that the empty part of the top row moves the window"""

import itertools

import numpy as np
import pytest

from harness import App, as_rgb, close_to, ink, sh, session_env, unique_theme

FEATURE = "window frame"

THEME = unique_theme(0x80)
C = as_rgb(THEME)

DECORATIONS = ["server", "none", "client"]
MENU_BAR = ["never", "always"]
# every frame with and without the menu bar, hiding the tab bar for one tab or not
COMBOS = list(itertools.product(DECORATIONS, MENU_BAR, [True, False]))
COMBO_IDS = [f"{d}-menu_{m}-{'hide' if h else 'keep'}" for d, m, h in COMBOS]


def frame_app(app_factory, decorations: str, menu_bar: str, hide: bool, **settings) -> App:
    base = {"theme": {"mode": "dark"}, "window_title": ["number"], "hide_bar_for_one_tab": hide,
            "window": {"decorations": decorations, "menu_bar": menu_bar}}
    app = app_factory({**base, **settings}, themes={"dark": THEME})
    # keep the pointer away from the bars, hover changes their colors
    app.mouse(450, 500)
    return app


def bar_borders(app: App, img) -> int:
    """number of bars stacked at the top, each ends with a border line"""
    h = app.bar_height()
    n = 0
    while close_to(tuple(img[(n + 1) * h - 1, 450]), C["border"]):
        n += 1
    return n


def window_buttons(app: App, img) -> list:
    """(x, y) centers of the client drawn buttons in the top row, left to right: fullscreen,
    minimize, maximize and close. empty when there are none"""
    h = app.bar_height()
    # they sit at the right end, right of anything the menus or tabs could put there
    x0 = 900 - round(220 * app.scale)
    columns = np.flatnonzero(ink(app.shot()[2 : h - 2, x0:], C["tab_bar_background"]).any(axis=0))
    if not len(columns):
        return []
    # each button pads its glyph wider than the gaps inside one glyph
    groups = np.split(columns, np.flatnonzero(np.diff(columns) > 8 * app.scale) + 1)
    return [(x0 + (int(g[0]) + int(g[-1])) // 2, h // 2) for g in groups[-4:]]


def check_layout(app: App, decorations: str, tabs: int) -> None:
    rows = app.bars_above(tabs)
    h = app.bar_height()
    app.wait(lambda: bar_borders(app, app.shot()) == rows,
             msg=f"{rows} bars above the grid, got {bar_borders(app, app.shot())}")
    img = app.shot()
    assert close_to(tuple(img[rows * h + 2, 450]), C["terminal_background"])
    # "client" always keeps a top row for its buttons, without bars the top row is the grid
    if rows:
        buttons = window_buttons(app, img)
        assert len(buttons) == (4 if decorations == "client" else 0), buttons
    else:
        assert decorations != "client"
    assert app.pty_size(f"size{tabs}") == app.layout_pty(tabs=tabs)


@pytest.mark.parametrize("decorations,menu_bar,hide", COMBOS, ids=COMBO_IDS)
def test_rows_above_the_grid(app_factory, decorations, menu_bar, hide):
    """the frame, menu bar and hide_bar_for_one_tab decide the bars above the grid with one and
    two tabs, the shell gets the rows left below them, "client" puts its buttons at the right
    end of the top row"""
    app = frame_app(app_factory, decorations, menu_bar, hide)
    check_layout(app, decorations, 1)
    app.snap(f"decorations {decorations}, menu bar {menu_bar}, one tab")
    app.key("ctrl+shift+t")
    app.wait_title("2")
    check_layout(app, decorations, 2)
    app.snap(f"decorations {decorations}, menu bar {menu_bar}, two tabs")
    app.key("ctrl+shift+w")
    app.wait_title("1")
    app.wait(lambda: bar_borders(app, app.shot()) == app.bars_above(1),
             msg="bars of one tab again")


@pytest.mark.parametrize("status_bar", [False, True], ids=["no_status", "status"])
@pytest.mark.parametrize("decorations,menu_bar,hide", COMBOS, ids=COMBO_IDS)
def test_window_fits_columns_and_rows(app_factory, decorations, menu_bar, hide, status_bar):
    """the window opens big enough for window.columns by window.rows cells next to every bar"""
    app = app_factory({"hide_bar_for_one_tab": hide, "status_bar": {"enable": status_bar},
                       "window": {"decorations": decorations, "menu_bar": menu_bar,
                                  "remember_size": False, "columns": 70, "rows": 20}})
    app.snap(f"decorations {decorations}, menu bar {menu_bar}, status bar {status_bar}")
    assert app.pty_size() == (20, 70)


@pytest.mark.parametrize("hide", [True, False], ids=["hide", "keep"])
@pytest.mark.parametrize("menu_bar", MENU_BAR)
def test_client_close_button_quits(app_factory, menu_bar, hide):
    """the close button of "client" decorations quits like the window manager's, from the title
    bar, the menu bar or the tab bar"""
    app = frame_app(app_factory, "client", menu_bar, hide)
    app.pty_size()
    buttons = app.wait(lambda: window_buttons(app, app.shot()), msg="window buttons")
    app.snap(f"client decorations, menu bar {menu_bar}")
    app.click(*buttons[-1])
    assert app.exit_code() == 0


@pytest.mark.parametrize("menu_bar", MENU_BAR)
def test_client_fullscreen_button(app_factory, menu_bar):
    """the fullscreen button of "client" decorations goes fullscreen and back, the buttons move
    with the right edge"""
    app = frame_app(app_factory, "client", menu_bar, True)
    app.pty_size()
    app.click(*app.wait(lambda: window_buttons(app, app.shot()), msg="window buttons")[0])
    app.wait(app.fullscreen, msg="fullscreen")
    app.mouse(450, 300)
    app.snap("fullscreen from the button")
    # the buttons are found again in the bigger window
    width = app.size()[0]
    x0 = width - round(220 * app.scale)
    h = app.bar_height()

    def fullscreen_button():
        columns = np.flatnonzero(ink(app.shot()[2 : h - 2, x0:], C["tab_bar_background"]).any(axis=0))
        groups = np.split(columns, np.flatnonzero(np.diff(columns) > 8 * app.scale) + 1)
        return len(groups) >= 4 and (x0 + (int(groups[-4][0]) + int(groups[-4][-1])) // 2, h // 2)

    app.click(*app.wait(fullscreen_button, msg="buttons at the new right edge"))
    app.wait(lambda: not app.fullscreen(), msg="windowed again")


def drag_spot(app: App) -> tuple:
    """a point in the empty part of the top row, between the menus or tabs and the buttons"""
    return round(620 * app.scale), app.bar_height() // 2


@pytest.mark.parametrize("hide", [True, False], ids=["hide", "keep"])
@pytest.mark.parametrize("menu_bar", MENU_BAR)
@pytest.mark.parametrize("decorations", ["none", "client"])
def test_empty_top_row_moves_the_window(app_factory, decorations, menu_bar, hide):
    """without a server frame a drag on the empty part of the top row moves the window"""
    app = frame_app(app_factory, decorations, menu_bar, hide)
    app.pty_size()
    x, y, _, _ = app.geometry()
    start = drag_spot(app)
    app.drag(start, (start[0] + 150, start[1] + 100))
    app.wait(lambda: app.geometry()[:2] != (x, y), msg="window moved")
    nx, ny, _, _ = app.geometry()
    assert nx > x and ny > y, ((x, y), (nx, ny))
    # still the same size and layout
    assert app.size() == (900, 600)
    assert bar_borders(app, app.shot()) == app.bars_above(1)


@pytest.mark.parametrize("menu_bar", MENU_BAR)
def test_server_frame_keeps_the_top_row_still(app_factory, menu_bar):
    """with a server frame the top row is not a handle, the window stays where it is"""
    app = frame_app(app_factory, "server", menu_bar, False)
    app.pty_size()
    before = app.geometry()
    start = drag_spot(app)
    app.drag(start, (start[0] + 150, start[1] + 100))
    app.pty_size("size2")
    assert app.geometry() == before


@pytest.mark.x11_only
@pytest.mark.parametrize("menu_bar", MENU_BAR)
@pytest.mark.parametrize("decorations", ["none", "client"])
def test_double_click_on_top_row_maximizes(app_factory, decorations, menu_bar):
    """a double click on the empty part of the top row maximizes the window"""
    app = frame_app(app_factory, decorations, menu_bar, True)
    app.pty_size()

    def maximized() -> bool:
        state = sh("xprop", "-id", app.wid, "_NET_WM_STATE", env=session_env())
        return "_NET_WM_STATE_MAXIMIZED_VERT" in state and "_NET_WM_STATE_MAXIMIZED_HORZ" in state

    app.click(*drag_spot(app), repeat=2)
    try:
        app.wait(maximized, msg="maximized window", timeout=4)
    except AssertionError:
        # a busy machine can stretch the two clicks past the double click time, once more
        app.click(*drag_spot(app), repeat=2)
        app.wait(maximized, msg="maximized window")
    app.snap("maximized by a double click")
