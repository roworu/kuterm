"""links and hints: modifier hover underlines a url, a click with it opens it, hints label matches"""

import stat
import tempfile
import time
from pathlib import Path

from harness import App, bundled_theme, get_clipboard, ink, near

FEATURE = "links and hints"

DARK = bundled_theme(True)


def opener() -> tuple:
    """a program for links.open_with that writes what it was given into a file"""
    folder = Path(tempfile.mkdtemp(prefix="e2e-opener-"))
    script = folder / "open.sh"
    out = folder / "opened"
    script.write_text(f'#!/bin/sh\nprintf "%s" "$1" > "{out}.tmp"; mv "{out}.tmp" "{out}"\n')
    script.chmod(script.stat().st_mode | stat.S_IXUSR)
    return script, out


def underline_ink(app: App, row: int, col: int, cols: int) -> int:
    """text colored pixels in the bottom two pixel rows of some cells, where the underline goes"""
    x0, _, x1, y1 = app.cell_rect(col, row, cols)
    return int(ink(app.shot()[y1 - 3 : y1, x0:x1], DARK["terminal_background"]).sum())


def test_ctrl_click_opens_url(app_factory):
    """holding ctrl over a url underlines it, ctrl and click opens it with links.open_with"""
    script, out = opener()
    app = app_factory({"theme": {"mode": "dark"}, "links": {"open_with": str(script)}},
                      script="printf 'visit https://kuterm.test/page now\\n'")
    app.wait(lambda: near(app.shot()[:60], DARK["terminal_foreground"], 30).any(), msg="url printed")
    x0, y0, x1, y1 = app.cell_rect(10, 0)
    app.mouse((x0 + x1) // 2, (y0 + y1) // 2)
    time.sleep(0.3)
    plain = underline_ink(app, 0, 6, 26)
    with app.hold("ctrl"):
        app.mouse((x0 + x1) // 2 + 1, (y0 + y1) // 2)
        app.wait(lambda: underline_ink(app, 0, 6, 26) > plain + 100, msg="underline under the url")
        app.snap("ctrl held over a url, it is underlined")
        app.button(1, True)
        app.button(1, False)
    app.wait(out.exists, msg="url opened")
    assert out.read_text() == "https://kuterm.test/page"


def test_osc8_hyperlink_opens_its_target(app_factory):
    """an osc 8 hyperlink opens its target, not the text it shows"""
    script, out = opener()
    app = app_factory({"theme": {"mode": "dark"}, "links": {"open_with": str(script)}},
                      script="printf '\\033]8;;https://hidden.test/x\\033\\\\click here\\033]8;;\\033\\\\\\n'")
    app.wait(lambda: near(app.shot()[:60], DARK["terminal_foreground"], 30).any(), msg="link printed")
    x0, y0, x1, y1 = app.cell_rect(2, 0)
    plain = underline_ink(app, 0, 0, 10)
    with app.hold("ctrl"):
        app.mouse((x0 + x1) // 2, (y0 + y1) // 2)
        app.wait(lambda: underline_ink(app, 0, 0, 10) > plain + 50, msg="underline under the link")
        app.button(1, True)
        app.button(1, False)
    app.wait(out.exists, msg="hyperlink opened")
    assert out.read_text() == "https://hidden.test/x"


def test_hints_label_and_copy(app_factory):
    """ctrl-shift-e labels the hash on screen, typing its label copies it"""
    app = app_factory({"theme": {"mode": "dark"}}, script="printf 'commit 1a2b3c4d5e\\n'")
    app.wait_line(0, 17)
    app.key("ctrl+shift+e")
    x0, y0, x1, y1 = app.cell_rect(7, 0)
    app.wait(lambda: near(app.shot()[y0:y1, x0:x1], DARK["cursor"], 2).sum() > 30, msg="label over the hash")
    app.snap("hint label over the hash")
    # the first letter of the alphabet labels the only match
    app.key("a")
    app.wait(lambda: get_clipboard() == "1a2b3c4d5e", msg=f"clipboard, got {get_clipboard()!r}")
    app.wait(lambda: near(app.shot()[y0:y1, x0:x1], DARK["cursor"], 2).sum() < 5, msg="labels gone")
