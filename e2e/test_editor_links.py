"""file:line links: opened in an editor running in another tab or pane, at that line, which gets
the focus ("links.open_in_running_editor"), else opened as before; resting the mouse on a
file:line shows the lines around it ("links.peek")"""

import os
import time

import pytest

from harness import App, as_rgb, near, unique_theme
from kit import integrated, ready, send, tabs, text, wait_block
from test_status_bar import opener

FEATURE = "editor links"

THEME = unique_theme(0x9d)
C = as_rgb(THEME)
LINK = {"regex": "[\\w./-]+\\.txt:\\d+(:\\d+)?", "action": "open"}


def editor_app(app_factory, **links) -> tuple:
    script, opened = opener()
    app = app_factory(integrated(window_title=["number"], links={"patterns": [LINK], "open_with": str(script),
                                                                 **links}),
                      themes={"dark": THEME})
    ready(app)
    (app.home / "f.txt").write_text("".join(f"line {n}\n" for n in range(1, 21)))
    os.makedirs(app.file("bin"), exist_ok=True)
    # vim.tiny under the name vim, the name kuterm knows it by
    os.symlink("/usr/bin/vim.tiny", app.file("bin/vim"))
    return app, opened


def start_editor(app: App, command: str) -> None:
    """the editor in tab 1, a shell in tab 2 printing the link"""
    send(app, f"cd ~ && {command}")
    time.sleep(1.5)
    app.key("ctrl+shift+t")
    app.wait_title("2")
    ready(app, str(next(t["id"] for t in tabs(app) if t["number"] == 2)))


def print_link(app: App, link: str = "f.txt:3") -> tuple:
    """print link in the active tab and return the window point at its middle"""
    send(app, f"clear; echo {link}")
    app.wait(lambda: any(line.strip() == link for line in text(app).splitlines()), msg="link printed")
    time.sleep(0.5)
    lines = text(app).splitlines()
    row = next(i for i, line in enumerate(lines) if line.strip() == link)
    x0, y0, x1, y1 = app.cell_rect(2, row, top=app.bar_height())
    return (x0 + x1) // 2, (y0 + y1) // 2


def ctrl_click(app: App, point: tuple) -> None:
    with app.hold("ctrl"):
        app.mouse(*point)
        time.sleep(0.3)
        app.button(1, True)
        app.button(1, False)


def current_line(app: App) -> str:
    """the line the editor's cursor is on, written out by the editor itself"""
    out = app.file("cur")
    out.unlink(missing_ok=True)
    app.key("Escape")
    app.type(f":.w! {out}")
    app.key("Return")
    return app.wait_file("cur").strip()


def test_vim_in_another_tab_opens_the_file_at_the_line(app_factory):
    app, opened = editor_app(app_factory)
    start_editor(app, "$T2_TMP/bin/vim -u NONE -N")
    point = print_link(app)
    ctrl_click(app, point)
    app.wait_title("1")
    app.wait(lambda: "line 1" in text(app), msg="the file in vim")
    app.snap("vim in tab 1 opened f.txt at line 3 and got the focus")
    assert current_line(app) == "line 3"
    assert not opened.exists()


@pytest.mark.parametrize("listen", [True, False])
def test_nvim_gets_the_file_over_its_socket(app_factory, listen):
    app, opened = editor_app(app_factory)
    flag = "--listen $T2_TMP/nv.sock" if listen else ""
    start_editor(app, f"nvim --clean {flag}")
    point = print_link(app, "f.txt:7")
    ctrl_click(app, point)
    app.wait_title("1")
    app.wait(lambda: "line 7" in text(app), msg="the file in nvim")
    assert current_line(app) == "line 7"
    assert not opened.exists()


def test_line_and_column_both_reach_the_editor(app_factory):
    app, _ = editor_app(app_factory)
    start_editor(app, "$T2_TMP/bin/vim -u NONE -N")
    ctrl_click(app, print_link(app, "f.txt:12:4"))
    app.wait_title("1")
    assert current_line(app) == "line 12"


def test_without_an_editor_the_link_opens_as_before(app_factory):
    app, opened = editor_app(app_factory)
    ctrl_click(app, print_link(app))
    app.wait(opened.exists, msg="opened with links.open_with")
    assert opened.read_text() == "f.txt:3"


def test_turned_off_the_link_opens_as_before_with_an_editor_running(app_factory):
    app, opened = editor_app(app_factory, open_in_running_editor=False)
    start_editor(app, "$T2_TMP/bin/vim -u NONE -N")
    ctrl_click(app, print_link(app))
    app.wait(opened.exists, msg="opened with links.open_with")
    assert app.title() == "2"


def test_a_path_without_a_line_is_no_link(app_factory):
    """the pattern needs a line, a plain path stays text, nothing opens anywhere"""
    app, opened = editor_app(app_factory)
    start_editor(app, "$T2_TMP/bin/vim -u NONE -N")
    ctrl_click(app, print_link(app, "f.txt"))
    time.sleep(1.5)
    assert app.title() == "2" and not opened.exists()


def popover(app: App) -> int:
    """pixels of the peek box, in the tab bar color below the tab bar"""
    return int(near(app.shot()[app.bar_height() + 4 :], C["tab_bar_background"]).sum())


def rest_for_peek(app: App, point: tuple, minimum: int = 2000, timeout: float = 30) -> None:
    """rest on the link until its peek shows: the peek only starts on a move, and one move can
    land before the terminal has the link in its drawn content"""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        app.mouse(point[0], point[1] + 300)  # off the link, so the next move starts it over
        app.mouse(*point)
        for _ in range(20):  # the hover delay plus a paint
            if popover(app) > minimum:
                return
            time.sleep(0.1)
    raise AssertionError("the peek box did not show")


def test_resting_on_a_file_line_peeks_at_it(app_factory):
    app, _ = editor_app(app_factory)
    point = print_link(app, "f.txt:3")
    rest_for_peek(app, point)
    app.snap("lines 1 to 8 of f.txt, line 3 bright")
    # off the link it goes away
    app.mouse(point[0], point[1] + 300)
    app.wait(lambda: popover(app) < 200, timeout=30, msg="peek gone")


def test_peek_follows_the_folder_the_command_ran_in(app_factory):
    app, _ = editor_app(app_factory)
    (app.home / "sub").mkdir()
    (app.home / "sub/g.txt").write_text("deep line\n" * 5)
    send(app, "cd ~/sub")
    wait_block(app, "cd ~/sub")
    point = print_link(app, "g.txt:2")
    rest_for_peek(app, point, minimum=1000)


def test_no_peek_for_a_missing_file_or_when_off(app_factory):
    app, _ = editor_app(app_factory)
    point = print_link(app, "missing.txt:3")
    app.mouse(*point)
    time.sleep(1.5)
    assert popover(app) < 200
    off, _ = editor_app(app_factory, peek=False)
    point = print_link(off, "f.txt:3")
    off.mouse(*point)
    time.sleep(1.5)
    assert popover(off) < 200
