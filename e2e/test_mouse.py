"""mouse reporting: programs that ask for the mouse get clicks, wheel and motion as escape sequences"""

import time

import pytest

from harness import App, bundled_theme, near

FEATURE = "mouse reporting"

DARK = bundled_theme(True)

# turns on the modes, then keeps every byte the terminal sends for a few seconds
# "ready" shows the modes were read, the cursor after it is one more cell
READ = ("printf '{modes}'; stty raw -echo; printf ready; touch \"$T2_TMP/ready\"; "
        'timeout --foreground {seconds} cat > "$T2_TMP/got.tmp"; stty sane; mv "$T2_TMP/got.tmp" "$T2_TMP/got"')

SGR = "\\033[?1000h\\033[?1006h"


def reading(app_factory, modes: str, seconds: int = 3, **settings) -> App:
    """app whose program turned on `modes` and reads raw input, read it back with got()"""
    app = app_factory({"theme": {"mode": "dark"}, **settings}, script=READ.format(modes=modes, seconds=seconds))
    app.wait_file("ready")
    app.wait_line(0, 6)
    return app


def got(app: App) -> bytes:
    app.wait_file("got", timeout=10)
    return app.file("got").read_bytes()


def center(app: App, col: int, row: int) -> tuple:
    x0, y0, x1, y1 = app.cell_rect(col, row)
    return (x0 + x1) // 2, (y0 + y1) // 2


@pytest.mark.parametrize("button, code", [(1, 0), (2, 1), (3, 2)], ids=["left", "middle", "right"])
def test_sgr_click(app_factory, button, code):
    """a click is a press and a release at the 1 based cell, sgr keeps the button on release"""
    app = reading(app_factory, SGR)
    app.click(*center(app, 4, 2), button=button)
    app.snap(f"clicked button {button} in cell (4, 2)")
    assert got(app) == f"\x1b[<{code};5;3M\x1b[<{code};5;3m".encode()


@pytest.mark.parametrize("up, code", [(True, 64), (False, 65)], ids=["up", "down"])
def test_sgr_wheel(app_factory, up, code):
    """the wheel sends presses of 64 up and 65 down, one per scrolled line"""
    app = reading(app_factory, SGR)
    app.wheel(*center(app, 1, 1), up=up, clicks=1)
    data = got(app)
    one = f"\x1b[<{code};2;2M".encode()
    assert data and data == one * (len(data) // len(one)), data


def test_drag_reports_motion_with_button(app_factory):
    """1002 adds motion while a button is held, the button plus 32"""
    app = reading(app_factory, "\\033[?1002h\\033[?1006h")
    app.drag(center(app, 1, 1), center(app, 6, 1))
    data = got(app)
    assert data.startswith(b"\x1b[<0;2;2M") and data.endswith(b"\x1b[<0;7;2m"), data
    assert b"\x1b[<32;4;2M" in data, data


def test_all_motion_without_button(app_factory):
    """1003 reports moves with no button held as 35"""
    app = reading(app_factory, "\\033[?1003h\\033[?1006h")
    app.focus()
    for col in range(1, 6):
        app.mouse(*center(app, col, 1))
        time.sleep(0.05)
    data = got(app)
    assert b"\x1b[<35;4;2M" in data, data


def test_no_motion_in_click_mode(app_factory):
    """1000 alone reports no motion"""
    app = reading(app_factory, SGR)
    app.drag(center(app, 1, 1), center(app, 6, 1))
    assert got(app) == b"\x1b[<0;2;2M\x1b[<0;7;2m"


def test_ctrl_click_adds_16(app_factory):
    """ctrl held on a click adds 16 to the button"""
    app = reading(app_factory, SGR)
    with app.hold("ctrl"):
        app.click(*center(app, 4, 2))
    assert got(app) == b"\x1b[<16;5;3M\x1b[<16;5;3m"


def test_x10_encoding(app_factory):
    """without 1006 the button and cell are single bytes offset by 32, a release is button 3"""
    app = reading(app_factory, "\\033[?1000h")
    app.click(*center(app, 4, 2))
    assert got(app) == b"\x1b[M\x20\x25\x23\x1b[M\x23\x25\x23"


def test_utf8_encoding(app_factory):
    """1005 sends columns past 95 as two byte utf-8 characters"""
    app = reading(app_factory, "\\033[?1000h\\033[?1005h", terminal={"font_size": 6})
    assert app.expected_pty(900, 600)[1] > 110
    app.click(*center(app, 100, 2))
    # column 101 + 32 is U+0085
    assert got(app) == "\x1b[M\x20\u0085\x23\x1b[M\x23\u0085\x23".encode()


def test_shift_drag_selects_locally(app_factory):
    """shift keeps the mouse for selection, the program gets nothing"""
    app = reading(app_factory, SGR, seconds=4)
    x0, y0, x1, y1 = app.cell_rect(0, 0, 10)
    with app.hold("shift"):
        app.drag((x0 + 1, (y0 + y1) // 2), (x1 - 1, (y0 + y1) // 2))
    app.wait(lambda: near(app.shot()[y0:y1, x0:x1], DARK["selection"], 3).mean() > 0.3, msg="selection painted")
    app.snap("shift drag selected text although the program asked for the mouse")
    assert got(app) == b""


def test_no_reports_without_mouse_mode(app_factory):
    """a program that did not ask gets no bytes for clicks"""
    app = reading(app_factory, "")
    for button in (1, 2, 3):
        app.click(*center(app, 4, 2), button=button)
    assert got(app) == b""


@pytest.mark.parametrize("app_cursor, arrow", [(False, "\x1b[A"), (True, "\x1bOA")], ids=["normal", "app-cursor"])
def test_alternate_scroll_sends_arrows(app_factory, app_cursor, arrow):
    """on the alternate screen the wheel sends up arrows, in the cursor key mode the program set"""
    modes = "\\033[?1049h\\033[?1007h" + ("\\033[?1h" if app_cursor else "")
    app = reading(app_factory, modes)
    app.wheel(*center(app, 1, 1), up=True, clicks=1)
    data, up = got(app), arrow.encode()
    assert data and data == up * (len(data) // len(up)), data
