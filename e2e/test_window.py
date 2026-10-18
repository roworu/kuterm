"""window settings: padding, startup size and state, remembered size and opacity"""

import json
import math

import pytest

from harness import BACKEND, App, bundled_theme, close_to, near, session_env, sh, swaymsg, xdo

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


# the last row painted red in the alternate screen, like htop's key bar, or on the normal one
RED_BOTTOM = ("printf '{alt}\\033[2J\\033[%d;1H\\033[41m%*s\\033[0;1H\\033[0m' "
              "$(tput lines) $(tput cols) ''; sleep 60\n")


@pytest.mark.parametrize("alt", [True, False])
def test_edge_rows_of_full_screen_programs_reach_the_window_edge(app_factory, alt):
    """a full screen program's bottom row fills the padding under it, with no strip of the
    terminal background, a shell's output keeps the padding"""
    app = app_factory({"theme": {"mode": "dark"}, "terminal": {"scrollbar": {"enable": "off"}},
                       "window": {"padding": {"x": 4, "y": 4}}},
                      script=RED_BOTTOM.format(alt="\\033[?1049h" if alt else ""))
    app.wait(lambda: near(app.shot()[:, 450:451], RED, 3).any(), msg="red bottom row")
    img = app.wait_still()
    app.snap("bottom row of a full screen program" if alt else "bottom row on the normal screen")
    bottom = img[-1, 450]
    if alt:
        assert close_to(bottom, RED, 3), bottom
    else:
        assert close_to(bottom, DARK["terminal_background"], 3), bottom


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


def app_windows(app: App) -> list:
    """ids of the app's visible windows"""
    if BACKEND == "wayland":
        found, stack = [], [swaymsg("-t", "get_tree")]
        while stack:
            node = stack.pop()
            if node.get("pid") == app.proc.pid and node.get("visible"):
                found.append(str(node["id"]))
            stack.extend(node.get("nodes", []) + node.get("floating_nodes", []))
        return found
    return xdo("search", "--onlyvisible", "--pid", str(app.proc.pid), check=False).split()


def test_tab_dragged_out_gets_its_own_window(app_factory):
    """a tab dragged out of the window and let go opens a window of its own with the tab, its
    shell still running; on x11, dragged back onto the first window it joins it again"""
    settings = {"theme": {"mode": "dark"}, "hide_bar_for_one_tab": False, "window_title": ["number"]}
    app = app_factory(settings)
    app.key("ctrl+shift+t")
    app.wait_title("2")
    app.run("echo moved-shell")
    h = app.bar_height()
    w = app.size()[0]
    # the second tab, past the first one's width
    start = (300, h // 2)
    app.drag(start, (w + 400, 300))
    app.wait(lambda: len(app_windows(app)) == 2, msg="a window for the dragged tab")
    # the first window keeps its other tab
    app.wait_title("1")
    app.snap("first window after its second tab was dragged out")
    if BACKEND == "wayland":
        return
    new = next(wid for wid in app_windows(app) if wid != app.wid)
    info = sh("xwininfo", "-id", new, env=session_env())
    val = dict(line.strip().split(":", 1) for line in info.splitlines() if ":" in line)
    x, y = int(val["Absolute upper-left X"]), int(val["Absolute upper-left Y"])
    ax, ay, aw, ah = app.geometry()
    # from the tab of the new window onto the middle of the first one
    xdo("mousemove", str(x + 60), str(y + h // 2))
    xdo("mousedown", "1")
    for i in range(1, 9):
        xdo("mousemove", str(x + 60 + (ax + aw // 2 - x - 60) * i // 8), str(y + h // 2 + (ay + ah // 2 - y - h // 2) * i // 8))
    xdo("mouseup", "1")
    app.wait(lambda: len(app_windows(app)) == 1, msg="the emptied window closes")
    app.wait_title("2")
    app.snap("the tab dropped back on the first window")
