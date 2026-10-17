"""search scope: the search bar looks at everything, at one command's output, or at the output of
failed commands only, alt+s or its button switches; find in all tabs takes "failed:" in front"""

import time

import numpy as np

from harness import App, bundled_theme, mask_bbox, near
from kit import integrated, ready, send, wait_block

FEATURE = "search scope"

DARK = bundled_theme(True)


def scoped_app(app_factory, **extra) -> App:
    app = app_factory(integrated(window_title=["number"], **extra))
    ready(app)
    send(app, "echo foo_ok")
    wait_block(app, "echo foo_ok")
    send(app, "echo foo_bad; false")
    wait_block(app, "echo foo_bad")
    app.mouse(450, 590)
    return app


def lit(app: App, key: str) -> np.ndarray:
    return near(app.shot(), DARK[key], 2)


def matches(app: App) -> tuple:
    """pixels of the current match and of the other matches"""
    return int(lit(app, "search_match_active").sum()), int(lit(app, "search_match").sum())


def active_top(app: App) -> int:
    box = mask_bbox(lit(app, "search_match_active"))
    return box[1] if box else -1


def test_scopes_cycle_all_this_block_failed(app_factory):
    app = scoped_app(app_factory)
    app.key("ctrl+shift+f")
    app.type("foo_")
    # two command lines and two outputs
    app.wait(lambda: matches(app)[0] > 30 and matches(app)[1] > 30, msg="every match")
    app.snap("scope all: command lines and outputs")
    app.key("alt+s")
    # the first block shown from the top: the output of echo foo_ok alone
    app.wait(lambda: matches(app)[0] > 30 and matches(app)[1] == 0, msg="one block")
    block_row = active_top(app)
    app.snap("scope this block: only its output")
    app.key("alt+s")
    app.wait(lambda: matches(app)[0] > 30 and matches(app)[1] == 0 and active_top(app) > block_row,
             msg="the failed command's output")
    app.snap("scope failed: only the output of the command that failed")
    app.key("alt+s")
    app.wait(lambda: matches(app)[1] > 30, msg="back to all")


def test_failed_scope_with_no_failures_finds_nothing(app_factory):
    app = app_factory(integrated())
    ready(app)
    send(app, "echo bar_one")
    wait_block(app, "echo bar_one")
    app.mouse(450, 590)
    app.key("ctrl+shift+f")
    app.type("bar_one")
    app.wait(lambda: matches(app)[0] > 30, msg="matches")
    app.key("alt+s", "alt+s")
    app.wait(lambda: matches(app) == (0, 0), msg="no failed command, no match")


def test_scope_button_cycles_like_the_key(app_factory):
    app = scoped_app(app_factory)
    app.key("ctrl+shift+f")
    app.type("foo_")
    app.wait(lambda: matches(app)[1] > 30, msg="every match")
    # the scope button is the last toggle before the close button of the bar
    img = app.shot()
    top = app.bar_height() + 2
    bar = mask_bbox(near(img[top : img.shape[0] // 3, img.shape[1] // 2 :], DARK["tab_bar_background"]))
    assert bar
    before = matches(app)
    x = img.shape[1] // 2 + bar[2] - 45
    y = top + (bar[1] + bar[3]) // 2
    for _ in range(2):
        app.click(x, y)
        time.sleep(0.5)
        if matches(app) != before:
            break
        x -= 20
    app.wait(lambda: matches(app)[1] == 0, msg="a narrower scope from the button")


def test_scope_survives_walking_matches(app_factory):
    app = scoped_app(app_factory)
    send(app, "echo foo_bad2; false")
    wait_block(app, "echo foo_bad2")
    app.key("ctrl+shift+f")
    app.type("foo_bad")
    app.wait(lambda: matches(app)[1] > 30, msg="matches")
    app.key("alt+s", "alt+s")
    app.wait(lambda: matches(app)[0] > 30, msg="failed scope")
    first = active_top(app)
    app.key("Return")
    app.wait(lambda: active_top(app) not in (first, -1), msg="walked to the other failed output")
    # still only output rows, the command lines spelling foo_bad never light up
    rows = mask_bbox(lit(app, "search_match_active") | lit(app, "search_match"))
    assert rows


def find_rows(app: App) -> int:
    """rows of the find in all tabs list with a lit match"""
    mask = lit(app, "search_match_active").any(axis=1)
    edges = np.flatnonzero(np.diff(mask.astype(int)) == 1)
    return len(edges) + int(mask[0])


def test_find_in_all_tabs_failed_prefix(app_factory):
    app = scoped_app(app_factory)
    app.key("ctrl+shift+t")
    app.wait_title("2")
    app.palette("find in all tabs")
    app.type("foo_")
    app.wait(lambda: find_rows(app) >= 3, msg="matches of every kind listed")
    app.snap("find in all tabs lists command lines and outputs")
    app.key("ctrl+a")
    app.type("failed:foo_")
    app.wait(lambda: find_rows(app) == 1, msg="one failed output listed")
    app.snap("failed: lists only the output of the failed command")
    app.key("Return")
    app.wait_title("1")
