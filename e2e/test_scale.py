"""hidpi: the app at x11 scale factors 2 and 1.5, sizes in physical pixels follow the scale"""

import time

import pytest

from harness import App, bundled_theme, get_clipboard, mask_bbox, near
from test_font import SAMPLE, check_cells

# the scale comes from GPUI_X11_SCALE_FACTOR here, wayland takes it from the output
pytestmark = pytest.mark.x11_only

FEATURE = "hidpi scale"

DARK = bundled_theme(True)
SCALES = ["2", "1.5"]


def scaled(app_factory, scale: str, settings=None, **kwargs) -> App:
    return app_factory({"theme": {"mode": "dark"}, **(settings or {})},
                       env={"GPUI_X11_SCALE_FACTOR": scale}, **kwargs)


@pytest.mark.parametrize("scale", SCALES)
def test_window_and_pty_size(app_factory, scale):
    """the 900x600 window is scaled on screen, the pty keeps the size it has at scale 1"""
    app = scaled(app_factory, scale)
    s = float(scale)
    assert app.size() == (round(900 * s), round(600 * s))
    assert app.pty_size() == app.expected_pty(900, 600)
    app.snap(f"bash at scale {scale}")


@pytest.mark.parametrize("scale", SCALES)
def test_cells_scale(app_factory, scale):
    """the red 10x2 block covers the scaled cells"""
    app = scaled(app_factory, scale, script=SAMPLE)
    check_cells(app)
    app.snap(f"sample text at scale {scale}")


@pytest.mark.parametrize("scale", SCALES)
def test_tab_bar_scales(app_factory, scale):
    """the tab bar is 2 rems tall times the scale"""
    app = scaled(app_factory, scale, {"hide_bar_for_one_tab": False}, themes={"dark": {
        "tab_bar_background": "#ff00ff", "tab_active_background": "#ff00ff"}})
    h = app.bar_height()
    app.wait(lambda: near(app.shot()[:, 5 * app.size()[0] // 6], (255, 0, 255), 3).any(), msg="tab bar")
    column = near(app.shot()[:, 5 * app.size()[0] // 6], (255, 0, 255), 3)
    top, bottom = column.nonzero()[0][[0, -1]]
    # the bottom border is drawn inside the bar height
    assert top == 0 and abs(int(bottom) + 1 - h) <= round(float(scale)) + 1, (top, bottom, h)
    app.snap(f"tab bar at scale {scale}")


@pytest.mark.parametrize("scale", SCALES)
def test_bar_cursor_width_scales(app_factory, scale):
    """the bar cursor is 2 logical pixels wide, so 2 * scale physical ones"""
    app = scaled(app_factory, scale, {"terminal": {"cursor_shape": "bar"}}, script="")
    x0, y0, x1, y1 = app.cell_rect(0, 0)

    def width():
        box = mask_bbox(near(app.shot()[y0 : y1 + 2, max(x0 - 2, 0) : x1 + 2], DARK["cursor"], 3))
        return None if box is None else box[2] - box[0]

    app.wait(width, msg="bar cursor")
    assert abs(width() - 2 * float(scale)) <= 1, width()
    app.snap(f"bar cursor at scale {scale}")


@pytest.mark.parametrize("scale", SCALES)
def test_drag_selects_scaled_cells(app_factory, scale):
    """a drag over scaled cells selects the text under them"""
    app = scaled(app_factory, scale, {"window_title": ["number"]})
    app.run("clear; echo alpha bravo charlie")
    time.sleep(1)
    x0, y0, _, y1 = app.cell_rect(6, 0)
    _, _, x1, _ = app.cell_rect(10, 0)
    # xdotool moves in physical pixels, like the screenshots
    app.drag((x0 + 2, (y0 + y1) // 2), (x1 - 2, (y0 + y1) // 2))
    app.snap(f"bravo selected at scale {scale}")
    app.key("ctrl+shift+c")
    app.wait(lambda: get_clipboard() == "bravo", msg=f"clipboard, got {get_clipboard()!r}")


@pytest.mark.parametrize("scale", SCALES)
def test_scrollbar_width_scales(app_factory, scale):
    """the scrollbar is its logical width times the scale"""
    app = scaled(app_factory, scale, {"terminal": {"scrollbar": {"enable": "on", "auto_hide": 0}}},
                 themes={"dark": {"scrollbar": "#ff00ff"}})
    box = app.wait(lambda: mask_bbox(near(app.shot(), (255, 0, 255), 3)), msg="scrollbar")
    assert box[2] == app.size()[0]
    assert abs(box[2] - box[0] - app.scrollbar_width()) <= 1, (box, app.scrollbar_width())
    app.snap(f"scrollbar at scale {scale}")
