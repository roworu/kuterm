"""scrollback on screen: scrollbar modes, placement, width, auto hide, history length, smooth scroll, wheel speed"""

import time

import numpy as np
import pytest

from harness import App, bundled_theme, get_clipboard, mask_bbox, near, set_clipboard

FEATURE = "scrollbar and scrolling"

DARK = bundled_theme(True)
# no glyph edge blends into pure magenta, so its pixels are the thumb alone
BAR = (255, 0, 255)
LINES = "for i in $(seq 1 {n}); do echo line $i; done\n"


def scroll_app(app_factory, scrollbar=None, lines: int = 0, **terminal) -> App:
    # auto hide keeps even an "on" bar hidden until the first scroll, so it is off unless tested
    terminal["scrollbar"] = {"auto_hide": 0, **(scrollbar or {})}
    return app_factory({"theme": {"mode": "dark"}, "terminal": terminal},
                       script=LINES.format(n=lines) if lines else None,
                       themes={"dark": {"scrollbar": "#ff00ff"}})


def thumb(app: App, img=None) -> tuple | None:
    """bbox of the scrollbar thumb"""
    img = app.shot() if img is None else img
    return mask_bbox(near(img, BAR, 3))


@pytest.mark.parametrize("enable,history,shown", [
    ("dynamic", False, False), ("dynamic", True, True),
    ("on", False, True), ("on", True, True),
    ("off", False, False), ("off", True, False),
])
def test_scrollbar_enable(app_factory, enable, history, shown):
    """scrollbar.enable on always shows it, off never, dynamic once there is history"""
    app = scroll_app(app_factory, {"enable": enable}, lines=200 if history else 0)
    if history:
        app.wait(lambda: near(app.shot(), DARK["terminal_foreground"], 20).sum() > 500, msg="output")
    if shown:
        app.wait(lambda: thumb(app), msg="scrollbar")
        x0, _, x1, _ = thumb(app)
        assert x1 == 900 and x1 - x0 == 8
    else:
        time.sleep(1)
        assert thumb(app) is None


def test_scrollbar_off_gives_room_to_the_grid(app_factory):
    """with the scrollbar off its width goes to the grid"""
    app = scroll_app(app_factory, {"enable": "off"}, font_size=10)
    assert app.pty_size() == app.expected_pty(900, 600)


@pytest.mark.parametrize("placement", ["left", "right"])
def test_scrollbar_placement(app_factory, placement):
    """scrollbar.placement puts the bar at the left or right edge, the grid moves over"""
    app = scroll_app(app_factory, {"enable": "on", "placement": placement, "width": 12})
    app.wait(lambda: thumb(app), msg="scrollbar")
    x0, _, x1, _ = thumb(app)
    assert (x0, x1) == ((0, 12) if placement == "left" else (888, 900))


@pytest.mark.parametrize("width,expected", [(2, 2), (30, 30), (100, 64), (1, 8)])
def test_scrollbar_width(app_factory, width, expected):
    """scrollbar.width sets the bar width, limited to 2..64"""
    app = scroll_app(app_factory, {"enable": "on", "width": width})
    app.wait(lambda: thumb(app), msg="scrollbar")
    x0, _, x1, _ = thumb(app)
    assert x1 - x0 == expected


def test_scrollbar_auto_hide(app_factory):
    """scrollbar.auto_hide hides the bar after some quiet seconds, scrolling shows it again"""
    app = scroll_app(app_factory, {"enable": "dynamic", "auto_hide": 1}, lines=200)
    app.wait(lambda: thumb(app), msg="scrollbar")
    app.snap("history printed, bar shown")
    app.wait(lambda: thumb(app) is None, msg="bar hidden", timeout=5)
    app.snap("bar hidden after a second")
    app.wheel(450, 300, up=True, clicks=3)
    app.wait(lambda: thumb(app), msg="bar back after scrolling")


@pytest.mark.parametrize("history", [50, 0])
def test_max_history_length(app_factory, history):
    """max_history_length limits the scrollback, the thumb shows how much is kept"""
    app = scroll_app(app_factory, {"enable": "on"}, lines=1000, max_history_length=history)
    rows = app.expected_pty(900, 600)[0]
    kept = 50 if history else 1000 - rows + 1
    expected = max(600 * rows / (kept + rows), 20)
    app.wait(lambda: thumb(app) and abs((thumb(app)[3] - thumb(app)[1]) - expected) <= 3,
             msg=f"thumb {expected:.0f}px high, got {thumb(app)}")


def test_click_on_track_jumps(app_factory):
    """clicking the scrollbar track jumps there, the thumb follows"""
    app = scroll_app(app_factory, {"enable": "on"}, lines=500, smooth_scroll={"enable": False})
    app.wait(lambda: thumb(app) and thumb(app)[3] >= 598, msg=f"thumb at the bottom, got {thumb(app)}")
    app.click(895, 5)
    app.wait(lambda: thumb(app) and thumb(app)[1] <= 2, msg=f"thumb at the top, got {thumb(app)}")


def test_scroll_multiplier(app_factory):
    """scroll_multiplier sets how many lines one wheel step scrolls"""
    # lines per wheel step differ by backend, so two multipliers are compared
    moved = {}
    for multiplier in (1, 5):
        app = app_factory({"theme": {"mode": "dark"}, "terminal": {
            "scrollbar": {"enable": "on", "auto_hide": 0}, "smooth_scroll": {"enable": False},
            "scroll_multiplier": multiplier}},
            script=LINES.format(n=300), themes={"dark": {"scrollbar": "#ff00ff"}},
            name=f"scroll_multiplier_{multiplier}")
        app.wait(lambda: thumb(app) and thumb(app)[3] >= 598, msg=f"thumb at the bottom, got {thumb(app)}")
        app.wait_still("output done")
        bottom = thumb(app)[3]
        app.wheel(450, 300, up=True, clicks=2)
        app.wait(lambda: thumb(app) and thumb(app)[3] < bottom, msg="thumb moved up")
        app.wait_still("both wheel steps")
        moved[multiplier] = bottom - thumb(app)[3]
        app.snap(f"2 wheel steps with scroll_multiplier {multiplier}")
        app.close()
    assert moved[5] >= moved[1] * 3, moved


@pytest.mark.parametrize("enable", [True, False])
def test_smooth_scroll(app_factory, enable):
    """smooth_scroll glides to the new position over its duration, disabled jumps right away"""
    app = scroll_app(app_factory, {"enable": "on"}, lines=500,
                     smooth_scroll={"enable": enable, "duration": 1000, "easing": "linear"})
    app.wait(lambda: thumb(app) and thumb(app)[3] >= 598, msg=f"thumb at the bottom, got {thumb(app)}")
    app.wait_still("output done")
    app.wheel(450, 300, up=True, clicks=5)
    frames = []
    start = time.monotonic()
    while time.monotonic() - start < 1.6:
        frames.append(thumb(app))
        time.sleep(0.05)
    tops = [f[1] for f in frames if f]
    final = tops[-1]
    # positions the thumb went through on its way
    between = {t for t in tops if t not in (tops[0], final)}
    if enable:
        assert len(set(tops)) >= 4, tops
    else:
        assert len(between) <= 1, tops
    assert final < 600 - 30
    assert np.all(np.diff(tops) <= 0) or not enable


# MARK sits on the second to last row, then more output pushes it up
MARK_SCRIPT = ('r=$(stty size | cut -d" " -f1); seq 1 $((r - 2)); echo MARK; '
               "step 1; seq 1 5; step 2; seq 1 400")


def selected_rows(app: App) -> list:
    """rows whose first cells are painted in the selection color"""
    img = app.shot()
    rows = app.expected_pty(900, 600)[0]
    # glyphs cover part of the cells, the selection color fills the rest
    return [row for row in range(rows)
            if near(img[slice(*app.cell_rect(0, row, 4)[1::2]), slice(*app.cell_rect(0, row, 4)[0::2])],
                    DARK["selection"], 3).mean() > 0.3]


def mark_selected(app_factory, **terminal) -> tuple:
    """app with MARK selected by a drag, returns it and the row of MARK"""
    app = app_factory({"theme": {"mode": "dark"}, "terminal": {"scrollbar": {"auto_hide": 0}, **terminal}},
                      script=MARK_SCRIPT)
    row = app.expected_pty(900, 600)[0] - 2
    app.wait_line(row, 4)
    x0, y0, _, y1 = app.cell_rect(0, row)
    _, _, x1, _ = app.cell_rect(3, row)
    app.drag((x0 + 1, (y0 + y1) // 2), (x1 - 1, (y0 + y1) // 2))
    app.wait(lambda: selected_rows(app) == [row], msg=f"MARK selected, rows {selected_rows(app)}")
    return app, row


def test_selection_moves_with_output(app_factory):
    """output that scrolls the screen moves the selection up with its text"""
    app, row = mark_selected(app_factory)
    app.snap("MARK selected")
    app.step(1)
    app.wait(lambda: selected_rows(app) == [row - 5], msg=f"selection 5 rows up, rows {selected_rows(app)}")
    app.snap("5 more lines moved the selection up")
    app.key("ctrl+shift+c")
    app.wait(lambda: get_clipboard() == "MARK", msg=f"clipboard, got {get_clipboard()!r}")


def test_selection_moves_with_wheel(app_factory):
    """scrolling into history moves the selection down with its text"""
    app, row = mark_selected(app_factory)
    app.step(1)
    app.wait(lambda: selected_rows(app) == [row - 5], msg="selection moved up")
    app.wheel(450, 300, up=True, clicks=1)
    app.wait(lambda: (rows := selected_rows(app)) and rows[0] > row - 5, msg=f"selection moved down, rows {selected_rows(app)}")
    app.snap("the wheel scrolled up, the selection moved down with MARK")
    set_clipboard("before")
    app.key("ctrl+shift+c")
    app.wait(lambda: get_clipboard() == "MARK", msg=f"clipboard, got {get_clipboard()!r}")


def test_selection_dropped_from_history_clears(app_factory):
    """a selection pushed out of a short history is dropped, copy keeps the clipboard"""
    app, row = mark_selected(app_factory, max_history_length=10)
    app.step(1)
    app.step(2)
    app.wait(lambda: near(app.shot(), DARK["terminal_foreground"], 30).any(), msg="output")
    app.wait_still("output done")
    assert selected_rows(app) == []
    set_clipboard("before")
    app.key("ctrl+shift+c")
    # the clipboard must stay, so there is nothing to wait for
    time.sleep(0.5)
    assert get_clipboard() == "before"
    assert app.alive()
    app.snap("MARK left the history, nothing is selected")
