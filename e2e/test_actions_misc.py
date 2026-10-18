"""the built-in actions no other file covers: the font size trio, equalize panes, split left
and up, select all, select last output, closing tabs to either side, opening the settings file
and hide_mouse_when_typing. driven through the palette names, the default keybindings, keymap
entries and kuterm msg run-command, checked against the pixels and the shell"""

import os

import numpy as np

from harness import (App, as_rgb, bundled_settings, close_to, get_clipboard, mask_bbox, median, near,
                     unique_theme)
from kit import SOCKET, integrated, listening, msg, ready, send, tabs, text, wait_block

FEATURE = "the remaining built-in actions"

THEME = unique_theme(0x8e)
C = as_rgb(THEME)
# the bundled font size the font tests start from
BASE_FONT = float(bundled_settings()["terminal"]["font"]["size"])


def lines_of(value: str) -> list:
    """lines without their trailing spaces and without the empty ones at the end"""
    lines = [line.rstrip() for line in value.splitlines()]
    while lines and not lines[-1]:
        lines.pop()
    return lines


# ---- font size -------------------------------------------------------------


def grid_at(app: App, size: float) -> tuple:
    """(rows, cols) the harness layout math gives for a font size. a tab keeps its own size at
    runtime, so the expectation is built from the settings dict the app was made with"""
    terminal = app.settings.setdefault("terminal", {})
    flat = terminal.get("font_size")
    terminal["font_size"] = size
    try:
        return app.layout_pty()
    finally:
        if flat is None:
            terminal.pop("font_size", None)
        else:
            terminal["font_size"] = flat


def size_at(app: App, size: float) -> tuple:
    """(rows, cols) the shell of the active tab reports once the font change was drawn. the
    resize of the pty lands with the frame that draws the new cells, so the screen is given a
    chance to settle first, and the read types one command per try"""
    want = grid_at(app, size)
    got = None
    for _ in range(3):
        app.wait_still(f"the grid of font size {size}")
        got = app.pty_size()
        if got == want:
            break
    return got


def test_font_size_keybinding_resizes_the_grid(app_factory):
    """ctrl+= and ctrl-- make the active tab's font a pixel bigger or smaller and ctrl+0 takes
    it back to the size from settings: the shell sees the new rows and cols at every step.
    the keys are pressed by their keysym names, the only spelling both xdotool and the wayland
    virtual keyboard understand"""
    app = app_factory({"theme": {"mode": "dark"}, "terminal": {"font_size": BASE_FONT}},
                      themes={"dark": THEME})
    base = app.pty_size()
    assert base == grid_at(app, BASE_FONT)
    app.key("ctrl+equal")
    bigger = size_at(app, BASE_FONT + 1)
    assert bigger[0] < base[0] and bigger[1] < base[1], (bigger, base)
    app.key("ctrl+equal")
    bigger2 = size_at(app, BASE_FONT + 2)
    assert bigger2[1] < bigger[1], (bigger2, bigger)
    app.snap("two steps bigger, the grid holds fewer columns")
    app.key("ctrl+minus")
    assert size_at(app, BASE_FONT + 1) == bigger
    app.key("ctrl+0")
    assert size_at(app, BASE_FONT) == base
    app.snap("back at the size from settings")


def test_font_size_palette_names_resize_and_reset(app_factory):
    """"bigger font", "smaller font" and "reset font size" in the palette do what the keys do"""
    app = app_factory({"theme": {"mode": "dark"}, "terminal": {"font_size": BASE_FONT}},
                      themes={"dark": THEME})
    base = app.pty_size()
    app.palette("bigger font")
    assert size_at(app, BASE_FONT + 1)[1] < base[1]
    app.palette("smaller font")
    assert size_at(app, BASE_FONT) == base
    app.palette("smaller font")
    smaller = size_at(app, BASE_FONT - 1)
    assert smaller[1] > base[1], (smaller, base)
    app.snap("one step smaller through the palette")
    app.palette("reset font size")
    assert size_at(app, BASE_FONT) == base
    app.snap("reset font size put the cells back")


# ---- panes -----------------------------------------------------------------


def panes(app: App) -> list:
    """pane ids in layout order, left and top first"""
    return sorted({tab["pane"] for tab in tabs(app)})


def pane_tab(app: App, pane: int) -> str:
    """the tab id of a pane"""
    return str(next(tab["id"] for tab in tabs(app) if tab["pane"] == pane))


def pane_size(app: App, tab: str, name: str) -> tuple:
    """(rows, cols) the shell of a pane reports, asked over remote control. the file is removed
    before the question, so a size read earlier is never mistaken for the answer"""
    path = app.file(name)
    path.unlink(missing_ok=True)
    send(app, f"stty size > {path}.part && mv {path}.part {path}", pane=tab)
    return tuple(map(int, app.wait_file(name).split()))


def patch(app: App, x: int, y: int) -> tuple:
    """median color of a small square of the grid around a window point"""
    img = app.shot()
    return median(img[y - 12 : y + 12, x - 12 : x + 12])


def lit(app: App, x: int, y: int) -> bool:
    """true when that point shows the plain terminal background, panes without focus are dimmed"""
    return close_to(patch(app, x, y), C["terminal_background"], 4)


def test_equalize_panes_gives_the_panes_the_fresh_split_size(app_factory):
    """after the resize actions moved the divider, "equalize panes" in the palette gives both
    panes the size a fresh split had, their shells report the same rows and cols again"""
    app = app_factory(
        integrated(window_title=["number"], panes={"dim_inactive": 0}),
        themes={"dark": THEME},
        keybindings={"split_right": "ctrl-shift-r", "resize_pane_right": "ctrl-shift-l"})
    ready(app)
    assert msg(app, "split", "right").returncode == 0
    app.wait(lambda: len(panes(app)) == 2, msg="the second pane")
    left, right = (pane_tab(app, pane) for pane in panes(app))
    ready(app, right)
    halves = (pane_size(app, left, "left"), pane_size(app, right, "right"))
    assert halves[0][0] == halves[1][0] and abs(halves[0][1] - halves[1][1]) <= 1, halves

    for _ in range(3):
        app.key("ctrl+shift+l")
    app.wait_still("the divider")
    uneven = (pane_size(app, left, "left"), pane_size(app, right, "right"))
    assert uneven[0] != halves[0] and uneven[1] != halves[1], (uneven, halves)
    assert uneven[0][1] != uneven[1][1], uneven
    app.snap("the divider moved, the panes are uneven")

    app.palette("equalize panes")
    app.wait_still("the panes to even out")
    again = (pane_size(app, left, "left2"), pane_size(app, right, "right2"))
    assert again == halves, (again, halves)
    app.snap("equalize panes put the divider back where the split had it")


def test_split_left_moves_the_tab_into_a_pane_on_the_left(app_factory):
    """the split_left action takes the focused pane's tab into a new pane on the left and
    focuses it: the lit half is the left one and it holds the shell that was there before"""
    app = app_factory(
        integrated(window_title=["number"], panes={"dim_inactive": 0.6}),
        themes={"dark": THEME}, keybindings={"split_left": "ctrl-shift-h"})
    ready(app)
    send(app, "export WHERE=origin")
    app.key("ctrl+shift+h")
    app.wait(lambda: len(panes(app)) == 2, msg="the second pane")
    app.wait_still("the split")

    w, h = app.size()
    assert lit(app, w // 4, h * 3 // 4), "the left half is not the focused, lit pane"
    assert not lit(app, w * 3 // 4, h * 3 // 4), "the right half is not dimmed"
    app.snap("the new pane is the left, lit one")

    # typed without a pane, so it goes to the focused one: the pane on the left, in the shell
    # that was there before. the other pane got a fresh shell, and which pane id it has says
    # nothing about where it sits, so both are asked and the answers compared as a set
    send(app, "echo WHERE=$WHERE > $T2_TMP/moved")
    assert app.wait_file("moved").strip() == "WHERE=origin"
    seen = []
    for i, pane in enumerate(panes(app)):
        send(app, f"echo WHERE=$WHERE > $T2_TMP/where{i}", pane=pane_tab(app, pane))
        seen.append(app.wait_file(f"where{i}").strip())
    assert sorted(seen) == ["WHERE=", "WHERE=origin"], seen


def test_split_up_moves_the_tab_into_a_pane_above(app_factory):
    """the split_up action takes the focused pane's tab into a new pane above it and focuses
    it: the lit pane is the upper one of that half and it holds the shell that moved"""
    app = app_factory(
        integrated(window_title=["number"], panes={"dim_inactive": 0.6}),
        themes={"dark": THEME}, keybindings={"split_left": "ctrl-shift-h", "split_up": "ctrl-shift-k"})
    ready(app)
    send(app, "export WHERE=origin")
    app.key("ctrl+shift+h")
    app.wait(lambda: len(panes(app)) == 2, msg="the second pane")
    app.wait_still("the first split")

    app.key("ctrl+shift+k")
    app.wait(lambda: len(panes(app)) == 3, msg="the third pane")
    app.wait_still("the second split")

    w, h = app.size()
    assert lit(app, w // 4, h // 4 + 20), "the upper half of the left pane is not the lit one"
    assert not lit(app, w // 4, h * 3 // 4), "the pane below the new one is not dimmed"
    assert not lit(app, w * 3 // 4, h // 4 + 20), "the pane on the right is not dimmed"
    app.snap("the new pane is above the focused one")

    # the shell typed into is the focused one, and only the new pane holds a moved shell
    send(app, "echo WHERE=$WHERE > $T2_TMP/moved")
    assert app.wait_file("moved").strip() == "WHERE=origin"
    seen = []
    for i, pane in enumerate(panes(app)):
        send(app, f"echo WHERE=$WHERE > $T2_TMP/where{i}", pane=pane_tab(app, pane))
        seen.append(app.wait_file(f"where{i}").strip())
    assert sorted(seen) == ["WHERE=", "WHERE=", "WHERE=origin"], seen


# ---- selection -------------------------------------------------------------


def box_around(box: tuple, want: tuple, tol: int = 2) -> bool:
    """a box matching another one within a couple of pixels"""
    return box is not None and all(abs(got - expected) <= tol for got, expected in zip(box, want))


def test_select_all_puts_the_whole_screen_on_the_clipboard(app_factory):
    """"select all" covers the pane from its first row down to the line the shell sits on, the
    blank rows below the cursor are left out, and ctrl+shift+c copies exactly what is shown"""
    app = app_factory(SOCKET, themes={"dark": THEME})
    listening(app)
    # the size first, its long command line is wiped by the clear below
    rows, cols = app.pty_size()
    app.run("clear; printf 'alpha\\nbeta\\n'")

    def three_lines():
        lines = lines_of(text(app))
        return lines if len(lines) == 3 else None

    shown = app.wait(three_lines, msg="the two printed lines and the prompt under them")
    assert shown[:2] == ["alpha", "beta"], shown
    last = len(shown) - 1
    top = app.top()
    want = app.cell_rect(0, 0, cols, 1, top=top)[:2] + app.cell_rect(0, last, cols, 1, top=top)[2:]

    app.palette("select all")
    app.wait(lambda: box_around(mask_bbox(near(app.shot(), C["selection"])), want),
             msg=f"the whole width selected over rows 0 to {last} of {rows}, wanted {want}")
    app.snap("every row down to the line the shell sits on is selected")
    app.key("ctrl+shift+c")
    copied = app.wait(lambda: (c := get_clipboard()) and "alpha" in c and c, msg="the clipboard")
    assert lines_of(copied) == shown


def test_select_last_output_copies_just_the_last_output(app_factory):
    """with shell integration, "select last output" covers the lines the last command printed,
    a copy gives those back and not the output before them"""
    app = app_factory(integrated(), themes={"dark": THEME})
    ready(app)
    send(app, "echo FIRST_MARK")
    wait_block(app, "echo FIRST_MARK")
    send(app, "printf 'second\\nthird\\n'")
    wait_block(app, "printf")
    app.palette("select last output")
    app.wait_still("the selection")
    app.snap("the last command's output is selected")
    app.key("ctrl+shift+c")
    copied = app.wait(lambda: (c := get_clipboard()) and "second" in c and c, msg="the clipboard")
    assert lines_of(copied) == ["second", "third"]


# ---- tabs, settings, mouse -------------------------------------------------


def numbers(app: App) -> list:
    """the position number of every open tab, left to right"""
    return [tab["number"] for tab in tabs(app)]


def test_close_tabs_to_the_sides_from_the_socket_and_a_bound_key(app_factory):
    """kuterm msg run-command runs the bundled "close tabs to the right", a keymap entry bound
    to the command "close tabs to the left" closes the other side"""
    app = app_factory(
        {**SOCKET, "window_title": ["number"], "hide_bar_for_one_tab": False,
         "keymap": [{"keys": "ctrl-shift-j", "action": {"run": "close tabs to the left"}}]},
        themes={"dark": THEME})
    listening(app)
    for expected in ("2", "3", "4"):
        app.key("ctrl+shift+t")
        app.wait_title(expected)
    app.key("alt+3")
    app.wait_title("3")

    assert msg(app, "run-command", "close tabs to the right").returncode == 0
    app.wait(lambda: numbers(app) == [1, 2, 3] and numbers(app), msg="the tabs on the right closed")
    app.wait_title("3")
    app.snap("every tab right of the active one closed over the socket")

    app.key("ctrl+shift+j")
    app.wait(lambda: len(tabs(app)) == 1, msg="the tabs on the left closed")
    app.wait_title("1")
    assert numbers(app) == [1]
    app.snap("only the active tab is left")


def test_open_settings_hands_the_file_to_the_system_opener(app_factory):
    """the palette's "open settings" gives settings.jsonc to the desktop opener: a fake
    xdg-open on the app's PATH writes down the path it was asked to open"""
    app = app_factory({"theme": {"mode": "dark"}}, themes={"dark": THEME}, start=False)
    os.makedirs(app.file("bin"), exist_ok=True)
    opener = app.file("bin/xdg-open")
    opener.write_text('#!/bin/sh\nprintf "%s" "$1" > "$T2_TMP/opened"\n')
    opener.chmod(0o755)
    app.env_extra["PATH"] = f"{app.file('bin')}:{os.environ['PATH']}"
    app.start()

    app.palette("open settings")
    assert app.wait_file("opened") == str(app.config / "settings.jsonc")
    app.snap("the settings file went to the system opener")


def test_hide_mouse_when_typing_keeps_the_grid_intact(app_factory):
    """with "hide_mouse_when_typing" the pointer goes away on key presses and comes back on a
    mouse move. the pointer is a window attribute the compositor paints and no screenshot
    holds it, so what is pinned here is the app side: typing still lands in the grid and a
    mouse move over the grid changes none of the app's pixels"""
    app = app_factory({"theme": {"mode": "dark"}, "terminal": {"hide_mouse_when_typing": True}},
                      themes={"dark": THEME})
    app.mouse(450, 300)
    app.run("clear; echo TYPED_MARKER")
    app.wait_line(0, 12)
    settled = app.wait_still("the grid after typing")
    app.mouse(200, 400)
    app.wait_still("the grid after the mouse moved")
    assert np.array_equal(app.shot(), settled), "the mouse move changed pixels of the app"
    app.snap("typed with hide_mouse_when_typing on, the grid held no pointer")
