"""old output keeps the width it was printed at ("terminal.keep_block_width"): narrowing the
window leaves finished wide lines on one row, cut with an arrow and scrolled sideways with
shift+wheel, instead of rewrapping them over two rows"""

import time

import numpy as np

from harness import App, as_rgb, near, unique_theme
from kit import integrated, ready, send, text, wait_block

FEATURE = "keep print width"

THEME = unique_theme(0x99)
C = as_rgb(THEME)


def wide_app(app_factory, keep: bool) -> App:
    app = app_factory(integrated(terminal={"keep_block_width": keep}, shell_integration={"mark_sources": "none"}),
                      themes={"dark": THEME})
    ready(app)
    # one line nearly as wide as the window, ending in B; ansi red, so its rows can be counted
    # in pixels without looking at any other text
    send(app, "printf '\\033[31m'; printf 'A%.0s' $(seq 70); printf '\\033[0m'; echo B")
    wait_block(app, "printf")
    app.wait(lambda: wide_rows(app) == 1, msg="the wide line drawn")
    app.mouse(5, 5)
    return app


def wide_rows(app: App) -> int:
    """drawn rows holding the wide line. glyph edges leave gaps of a few scanlines, so rows are
    grouped by gaps smaller than half a line: split by the wrap, the line makes two groups"""
    ys = np.nonzero(near(app.shot(), C["ansi"][1], 2).any(axis=1))[0]
    if not len(ys):
        return 0
    return len(np.split(ys, np.nonzero(np.diff(ys) > app.line_height() / 2)[0] + 1))


def test_narrowing_keeps_the_line_on_one_row(app_factory):
    # one app at a time: on wayland floating windows of the same test land on the same rect,
    # and a shot of the lower one would show the upper one
    kept = wide_app(app_factory, True)
    kept.resize(450, 600)
    kept.wait_still()
    time.sleep(0.5)  # and it must stay on one row
    assert wide_rows(kept) == 1, wide_rows(kept)
    kept.snap("the wide line stays on one row, cut with an arrow")
    kept.close()
    plain = wide_app(app_factory, False)
    plain.resize(450, 600)
    plain.wait_still()
    # rewrapped, the plain one draws the line over two rows
    plain.wait(lambda: wide_rows(plain) == 2, msg="the wide line rewrapped")
    plain.snap("without the setting the line rewraps")


def test_the_text_read_back_is_the_whole_line(app_factory):
    app = wide_app(app_factory, True)
    app.resize(450, 600)
    app.wait_still()
    assert "A" * 70 + "B" in text(app).replace("\n", "")


def test_shift_wheel_scrolls_the_kept_line_sideways(app_factory):
    app = wide_app(app_factory, True)
    app.resize(450, 600)
    app.wait_still()
    time.sleep(0.5)
    before = app.shot()
    # over the wide line, the second row of the grid
    y = app.bar_height() + round(app.line_height() * 1.5)
    with app.hold("shift"):
        app.wheel(100, y, up=False, clicks=5)
    app.wait(lambda: not np.array_equal(app.shot(), before), msg="line scrolled sideways")
    app.snap("scrolled to the end of the kept line, B shows")


def test_widening_again_shows_it_as_printed(app_factory):
    kept = wide_app(app_factory, True)
    kept.resize(450, 600)
    kept.wait_still()
    kept.resize(900, 600)
    kept.wait(lambda: wide_rows(kept) == 1, msg="the kept line back on one row")
    kept.close()
    plain = wide_app(app_factory, False)
    plain.resize(450, 600)
    plain.wait(lambda: wide_rows(plain) == 2, msg="the plain line rewrapped")
    plain.resize(900, 600)
    plain.wait(lambda: wide_rows(plain) == 1, msg="the plain line back on one row")


def test_a_command_still_running_rewraps(app_factory):
    """only finished blocks keep their width, the live one rewraps like any terminal"""
    app = app_factory(integrated(terminal={"keep_block_width": True}, shell_integration={"mark_sources": "none"}),
                      themes={"dark": THEME})
    ready(app)
    send(app, "printf '\\033[31m'; printf 'A%.0s' $(seq 70); printf '\\033[0m'; echo B; sleep 60")
    app.wait(lambda: wide_rows(app) == 1, msg="the wide line drawn, still running")
    app.resize(450, 600)
    app.wait_still()
    app.wait(lambda: wide_rows(app) == 2, msg="the live line rewrapped")
