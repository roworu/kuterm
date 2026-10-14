"""status bar: ports, cpu and memory of the active tab and of all tabs, under the panes"""

import stat
import tempfile
import time
from pathlib import Path

import numpy as np

from harness import App, as_rgb, ink, mask_bbox, unique_theme

FEATURE = "status bar"

THEME = unique_theme(0x74)
C = as_rgb(THEME)


def opener() -> tuple:
    """a program for links.open_with that writes what it was given into a file"""
    folder = Path(tempfile.mkdtemp(prefix="e2e-opener-"))
    script = folder / "open.sh"
    out = folder / "opened"
    script.write_text(f'#!/bin/sh\nprintf "%s" "$1" > "{out}.tmp"; mv "{out}.tmp" "{out}"\n')
    script.chmod(script.stat().st_mode | stat.S_IXUSR)
    return script, out


def status_text(app: App, img, side: str) -> tuple | None:
    """bbox of the text on one side of the status bar, in window pixels"""
    # 1.5 rems, its top border row left out
    h = round(round(1.5 * app.ui_font_size()) * app.scale) - 2
    top = img.shape[0] - h
    half = img.shape[1] // 2
    x0 = 0 if side == "left" else half
    box = mask_bbox(ink(img[top:, x0 : x0 + half], C["tab_bar_background"]))
    return box and (box[0] + x0, box[1] + top, box[2] + x0, box[3] + top)


def title_text(app: App, img) -> tuple | None:
    """bbox of the first word in the active tab, left of its close button"""
    h = app.bar_height()
    columns = np.flatnonzero(ink(img[2 : h - 2], C["tab_active_background"]).any(axis=0))
    if not len(columns):
        return None
    first = np.split(columns, np.flatnonzero(np.diff(columns) > 8 * app.scale) + 1)[0]
    return int(first[0]), 2, int(first[-1]) + 1, h - 2


def test_ports_show_and_open(app_factory):
    """a server in the tab shows ":8123" in the status bar and in the tab title, a click on the
    bar or the link modifier and a click on the title opens http://localhost:8123"""
    script, out = opener()
    settings = {
        "theme": {"mode": "dark"},
        "hide_bar_for_one_tab": False,
        "tab_title": ["ports"],
        "status_bar": {"enable": True, "left": ["ports"], "right": ["total_cpu", "total_memory"]},
        "links": {"open_with": str(script)},
    }
    app = app_factory(settings, themes={"dark": THEME}, script="exec python3 -m http.server 8123 --bind 127.0.0.1")
    # keep the pointer away from the bars, hover changes their colors
    app.mouse(450, 300)
    app.wait(lambda: status_text(app, app.shot(), "right"), msg="totals in the status bar")
    box = app.wait(lambda: status_text(app, app.shot(), "left"), msg="port in the status bar", timeout=15)
    # the same refresh builds the tab title
    time.sleep(0.5)
    app.snap("server listening on 8123, its port in the status bar and the tab title")
    app.click((box[0] + box[2]) // 2, (box[1] + box[3]) // 2)
    app.wait(out.exists, msg="port opened from the status bar")
    assert out.read_text() == "http://localhost:8123"
    out.unlink()
    x0, y0, x1, y1 = title_text(app, app.shot())
    with app.hold("ctrl"):
        app.mouse((x0 + x1) // 2, (y0 + y1) // 2)
        time.sleep(0.3)
        app.button(1, True)
        app.button(1, False)
    app.wait(out.exists, msg="port opened from the tab title")
    assert out.read_text() == "http://localhost:8123"
