"""features that depend on where the grid sits, checked with the bars around it set in different
ways: menu bar, title bar, tab bar, status bar, padding, a left scrollbar, ui font size, hidpi,
fullscreen, splits and settings changed while running"""

import numpy as np
import pytest

from harness import App, get_clipboard, mask_bbox, near
from test_decorations import C, THEME, bar_borders, window_buttons
from test_mouse import READ, SGR, got

FEATURE = "settings combinations"

LAYOUTS = {
    # the defaults: a title bar with the window buttons above one tab
    "title_bar": {"window": {"decorations": "client"}},
    "client_menu": {"window": {"decorations": "client", "menu_bar": "always"}},
    "none_menu_tab_bar": {"hide_bar_for_one_tab": False,
                          "window": {"decorations": "none", "menu_bar": "always"}},
    "server_menu_status": {"status_bar": {"enable": True},
                           "window": {"decorations": "server", "menu_bar": "always"}},
    # every bar at once, with a bigger ui font, padding and the scrollbar on the left
    "all_bars": {"hide_bar_for_one_tab": False, "status_bar": {"enable": True}, "ui_font_size": 24,
                 "terminal": {"scrollbar": {"enable": "on", "placement": "left", "auto_hide": 0}},
                 "window": {"decorations": "client", "menu_bar": "always", "padding": {"x": 20, "y": 12}}},
}
CLIENT = ["title_bar", "client_menu", "all_bars"]

# no glyph edge blends into pure magenta, so its pixels are the scrollbar thumb alone
BAR = (255, 0, 255)


def merge(base: dict, over: dict) -> dict:
    """over merged into base key by key, like the app merges settings"""
    out = dict(base)
    for key, value in over.items():
        out[key] = merge(out[key], value) if isinstance(value, dict) and isinstance(out.get(key), dict) else value
    return out


def layout_app(app_factory, layout: str, settings=None, **kwargs) -> App:
    base = {"theme": {"mode": "dark"}, "window_title": ["number"]}
    app = app_factory(merge(merge(base, LAYOUTS[layout]), settings or {}),
                      themes={"dark": {**THEME, "scrollbar": "#ff00ff"}}, **kwargs)
    # keep the pointer off the bars, hover changes their colors
    app.mouse(450, 300)
    return app


def center(app: App, col: int, row: int) -> tuple:
    x0, y0, x1, y1 = app.cell_rect(col, row, top=app.top())
    return (x0 + x1) // 2, (y0 + y1) // 2


@pytest.mark.parametrize("layout", LAYOUTS)
def test_layout_gives_the_shell_its_rows(app_factory, layout):
    """the shell gets the rows and columns left by the bars, padding and scrollbar"""
    app = layout_app(app_factory, layout)
    assert app.pty_size() == app.layout_pty()
    app.snap(f"layout {layout}")


@pytest.mark.parametrize("layout", LAYOUTS)
def test_click_reports_the_right_cell(app_factory, layout):
    """a program asking for the mouse gets the cell under the pointer, not one shifted by the bars"""
    app = layout_app(app_factory, layout, script=READ.format(modes=SGR, seconds=3))
    app.wait_file("ready")
    app.wait_line(0, 6, top=app.top())
    # under load a click right after the first frames can land a cell off with client
    # decorations on wayland, so the window settles first
    app.wait_still()
    app.click(*center(app, 4, 2))
    assert got(app) == b"\x1b[<0;5;3M\x1b[<0;5;3m"


@pytest.mark.parametrize("layout", LAYOUTS)
def test_double_click_selects_the_word_under_the_pointer(app_factory, layout):
    """a double click selects the word in the cell under the pointer"""
    app = layout_app(app_factory, layout)
    app.run("clear; echo alpha bravo charlie")
    app.wait_line(0, 19, top=app.top())
    app.click(*center(app, 8, 0), repeat=2)
    app.snap("double click on bravo")
    app.key("ctrl+shift+c")
    app.wait(lambda: get_clipboard() == "bravo", msg=f"clipboard, got {get_clipboard()!r}")


def thumb(app: App) -> tuple | None:
    ys, xs = np.nonzero(near(app.shot(), BAR, 3))
    return (int(xs.min()), int(ys.min()), int(xs.max()) + 1, int(ys.max()) + 1) if len(xs) else None


@pytest.mark.parametrize("layout", LAYOUTS)
def test_scrollbar_track_spans_the_grid(app_factory, layout):
    """the scrollbar runs from below the top bars to above the status bar, a click at the top of
    its track jumps to the top of the history"""
    settings = {"terminal": {"scrollbar": {"enable": "on", "auto_hide": 0}, "smooth_scroll": {"enable": False}}}
    app = layout_app(app_factory, layout, settings, script="seq 1 500\n")
    status = round(1.5 * app.ui_font_size() * app.scale) if app.settings.get("status_bar", {}).get("enable") else 0
    bottom = app.size()[1] - status
    app.wait(lambda: thumb(app) and abs(thumb(app)[3] - bottom) <= 2, msg=f"thumb at the bottom, got {thumb(app)}")
    x0, _, x1, _ = thumb(app)
    app.click((x0 + x1) // 2, app.top() + 3)
    app.wait(lambda: thumb(app) and abs(thumb(app)[1] - app.top()) <= 2, msg=f"thumb at the top, got {thumb(app)}")
    app.mouse(450, 300)
    app.snap("history scrolled to the top from the track")


@pytest.mark.parametrize("layout", LAYOUTS)
def test_palette_leaves_the_bars_uncovered(app_factory, layout):
    """the command palette opens over the terminal, the bars above stay as they were"""
    # a title that never changes, so only the palette could change the tab bar
    app = layout_app(app_factory, layout, {"tab_title": [{"text": "title"}]})
    app.pty_size()
    before = app.shot()
    app.key("ctrl+shift+p")
    changed = lambda: np.abs(app.shot().astype(int) - before.astype(int)).max(axis=-1) > 0
    app.wait(lambda: changed()[app.top():].sum() > 5000, msg="palette open")
    app.snap("palette open")
    covered = mask_bbox(changed()[: app.top()])
    assert covered is None, f"bars changed in {covered}"
    app.key("Escape")


@pytest.mark.parametrize("layout", [name for name, layout in LAYOUTS.items() if "status_bar" in layout])
def test_notification_leaves_the_status_bar_uncovered(app_factory, layout):
    """notifications stack above the status bar instead of over it"""
    # blocks that never change, so only a notification could change the bar
    settings = {"status_bar": {"left": [{"text": "left"}], "right": [{"text": "right"}]}}
    app = layout_app(app_factory, layout, settings,
                     commands={"commands": [{"name": "say hi", "actions": [{"notify": "hi from e2e"}]}]})
    app.pty_size()
    status = round(1.5 * app.ui_font_size() * app.scale)
    # the shell draws its next prompt after the size was read
    before = app.wait_still("the next prompt")
    app.palette("say hi")
    changed = lambda: np.abs(app.shot().astype(int) - before.astype(int)).max(axis=-1) > 0
    # the palette is gone and the notification is up in the bottom right corner
    app.wait(lambda: changed()[:, 600:].sum() > 3000 and not changed()[: app.size()[1] // 2].any(),
             msg="notification")
    app.snap("notification with the status bar")
    # its shadow may darken the bar a little, the notification itself must not cover it
    covered = mask_bbox(np.abs(app.shot().astype(int) - before.astype(int))[-status:].max(axis=-1) > 40)
    assert covered is None, f"status bar covered in {covered}"


@pytest.mark.parametrize("command", ["split right", "split down"])
@pytest.mark.parametrize("layout", CLIENT)
def test_split_keeps_the_window_buttons_in_the_corner(app_factory, layout, command):
    """after a split the window buttons stay once at the top right, and the close button quits"""
    app = layout_app(app_factory, layout)
    app.key("ctrl+shift+t")
    app.wait_title("2")
    top = app.top(2)
    app.palette(command)

    def split() -> bool:
        # below the bars a border line now crosses the panes, the divider or the new tab bar
        border = near(app.shot()[top + 2 : -5], C["border"])
        return border.mean(axis=0 if command == "split right" else 1).max() > 0.6

    app.wait(split, msg=f"divider after {command}")
    app.mouse(450, 300)
    buttons = app.wait(lambda: window_buttons(app, app.shot()), msg="window buttons")
    app.snap(f"{command} with {layout}")
    assert len(buttons) == 4, buttons
    app.click(*buttons[-1])
    assert app.exit_code() == 0


@pytest.mark.parametrize("layout", LAYOUTS)
def test_drag_tab_to_the_bottom_half_splits(app_factory, layout):
    """a tab dragged from its bar below the other bars to the bottom half splits the pane there"""
    app = layout_app(app_factory, layout, {"tab_title": [{"text": "title"}], "panes": {"divider_width": 4}})
    app.key("ctrl+shift+t")
    app.wait_title("2")
    top = app.top(2)
    w, h = app.size()
    app.drag((round(100 * app.scale), top - app.bar_height() // 2), (w // 2, top + (h - top) * 3 // 4))

    def split() -> bool:
        # a border line now crosses the whole width below the bars
        return near(app.shot()[top + 2 : -5], C["border"]).mean(axis=1).max() > 0.6

    app.wait(split, msg="horizontal split")
    app.snap(f"tab dragged down with {layout}")


@pytest.mark.parametrize("layout", LAYOUTS)
def test_fullscreen_keeps_the_layout(app_factory, layout):
    """fullscreen and back keeps the bars, the shell gets the rows of the whole screen"""
    app = layout_app(app_factory, layout)
    app.pty_size()
    app.key("F11")
    app.wait(app.fullscreen, msg="fullscreen")
    app.wait(lambda: app.size()[0] > 900 * app.scale, msg="fullscreen size")
    w, h = (round(v / app.scale) for v in app.size())
    app.wait(lambda: bar_borders(app, app.shot()) == app.bars_above(), msg="bars in fullscreen")
    assert app.pty_size("full") == app.layout_pty(w, h)
    app.key("F11")
    app.wait(lambda: not app.fullscreen(), msg="windowed again")
    app.wait(lambda: app.size() == (round(900 * app.scale), round(600 * app.scale)), msg="old size")
    assert app.pty_size("windowed") == app.layout_pty()


@pytest.mark.x11_only
@pytest.mark.parametrize("layout", LAYOUTS)
def test_hidpi_layout(app_factory, layout):
    """at scale 2 the bars and cells grow together, the shell keeps its rows and clicks still
    land in the right cell"""
    app = layout_app(app_factory, layout, script=READ.format(modes=SGR, seconds=6),
                     env={"GPUI_X11_SCALE_FACTOR": "2"})
    app.wait_file("ready")
    app.wait_line(0, 6, top=app.top())
    # under load a click right after the first frames can land a cell off with client
    # decorations on wayland, so the window settles first
    app.wait_still()
    assert bar_borders(app, app.shot()) == app.bars_above()
    app.snap(f"layout {layout} at scale 2")
    app.click(*center(app, 4, 2))
    assert got(app) == b"\x1b[<0;5;3M\x1b[<0;5;3m"


# the frame is read at startup, so each change keeps it
RELOADS = [
    ("title_bar", "client_menu"),
    ("client_menu", "title_bar"),
    ("title_bar", "all_bars"),
    ("all_bars", "title_bar"),
    ("server", "server_menu_status"),
    ("none_menu_tab_bar", "none"),
]
MORE_LAYOUTS = {"server": {"window": {"decorations": "server"}}, "none": {"window": {"decorations": "none"}}}
BASE = {"theme": {"mode": "dark", "themes": {"dark": THEME}}, "window_title": ["number"]}


@pytest.mark.parametrize("before,after", RELOADS, ids=[f"{a}-to-{b}" for a, b in RELOADS])
def test_saved_layout_applies_live(app_factory, before, after):
    """saving other menu, tab and status bar settings changes the bars and the shell's rows
    right away"""
    layouts = {**LAYOUTS, **MORE_LAYOUTS}
    app = app_factory(merge(BASE, layouts[before]))
    app.mouse(450, 300)
    assert app.pty_size("before") == app.layout_pty()
    # the remembered size the harness added keeps the window at 900x600
    settings = merge({**BASE, "window": {"remember_size": True}}, layouts[after])
    app.write("settings.jsonc", settings)
    app.settings = settings
    app.wait(lambda: bar_borders(app, app.shot()) == app.bars_above(), msg=f"bars of {after}")
    app.wait(lambda: app.pty_size("after") == app.layout_pty(), msg=f"shell rows of {after}")
    app.snap(f"{before} changed to {after}")


@pytest.mark.parametrize("before,after", [("server", "client"), ("client", "server"), ("server", "none")])
def test_saved_decorations_apply_on_restart(app_factory, before, after):
    """window.decorations is read at startup: a saved change keeps the frame until a restart"""
    app = app_factory(merge(BASE, {"window": {"decorations": before}}))
    app.mouse(450, 300)
    started = app.settings
    rows = app.pty_size("before")
    changed = merge(started, {"window": {"decorations": after}, "window_title": [{"text": "reloaded"}]})
    app.write("settings.jsonc", changed)
    # the title shows the rest of the file was applied
    app.wait_title("reloaded")
    assert bar_borders(app, app.shot()) == app.bars_above()
    assert app.pty_size("after") == rows
    app.snap(f"decorations {after} saved, still {before}")
    app.close()
    app.settings = changed
    app.start()
    app.mouse(450, 300)
    app.wait(lambda: bar_borders(app, app.shot()) == app.bars_above(), msg=f"bars of {after}")
    assert app.pty_size("restarted") == app.layout_pty()
    app.snap(f"decorations {after} after a restart")
