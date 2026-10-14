"""answers to terminal queries programs rely on: cursor position, device attributes, colors, size, env"""

import pytest

from harness import App, bundled_theme

FEATURE = "terminal protocol"

DARK = bundled_theme(True)

QUERY = (
    "stty raw -echo; printf '{sequence}'; "
    'timeout --foreground 1 cat > "$T2_TMP/got"; stty sane; touch "$T2_TMP/done"'
)


def reply(app_factory, report_note, sequence: str, **settings) -> str:
    app: App = app_factory({"theme": {"mode": "dark"}, **settings}, script=QUERY.format(sequence=sequence))
    app.wait_file("done", timeout=10)
    got = app.file("got").read_text(errors="replace")
    report_note(f"reply to {sequence}", repr(got))
    return got


def color(rgb: tuple) -> str:
    """xterm color reply format"""
    return "rgb:" + "/".join(f"{c:02x}{c:02x}" for c in rgb)


def test_cursor_position_report(app_factory, report_note):
    """CSI 6 n answers with the 1 based cursor row and column"""
    assert "\x1b[3;5R" in reply(app_factory, report_note, "\\033[3;5H\\033[6n")


def test_device_attributes(app_factory, report_note):
    """CSI c answers with primary device attributes"""
    got = reply(app_factory, report_note, "\\033[c")
    assert got.startswith("\x1b[?") and got.endswith("c"), repr(got)


def test_text_area_size_in_cells(app_factory, report_note):
    """CSI 18 t answers with the grid size, the same the pty has"""
    got = reply(app_factory, report_note, "\\033[18t")
    probe = App({"theme": {"mode": "dark"}}, start=False)
    rows, cols = probe.expected_pty(900, 600)
    probe.cleanup()
    assert f"\x1b[8;{rows};{cols}t" in got, repr(got)


@pytest.mark.parametrize("osc,key", [("10", "terminal_foreground"), ("11", "terminal_background")])
def test_color_queries_answer_theme_colors(app_factory, report_note, osc, key):
    """OSC 10 and 11 queries answer the theme foreground and background, used by vim and others"""
    got = reply(app_factory, report_note, f"\\033]{osc};?\\007")
    assert color(DARK[key]) in got, repr(got)


def test_palette_color_query(app_factory, report_note):
    """OSC 4 answers the theme ansi color"""
    got = reply(app_factory, report_note, "\\033]4;1;?\\007")
    assert color(DARK["ansi"][1]) in got, repr(got)


def test_focus_reports(app_factory, report_note):
    """with focus reporting on, switching tabs sends focus out and in"""
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["number"]},
                      # new tabs run this script too, only the first one records
                      script='mkdir "$T2_TMP/first" 2>/dev/null || exec sleep 100000; '
                             "printf '\\033[?1004h'; stty raw -echo; printf ready; "
                             'timeout --foreground 6 cat > "$T2_TMP/got"; stty sane; touch "$T2_TMP/done"')
    # the app read the mode once it shows what came after it, the cursor is one more cell
    app.wait_line(0, 6)
    app.key("ctrl+shift+t")
    app.wait_title("2")
    app.key("alt+1")
    app.wait_title("1")
    app.wait_file("done", timeout=15)
    got = app.file("got").read_text(errors="replace")
    report_note("focus reports", repr(got))
    assert "\x1b[O\x1b[I" in got, repr(got)


def test_environment_of_programs(app_factory, report_note):
    """programs see TERM, COLORTERM and TERM_PROGRAM"""
    app = app_factory({"theme": {"mode": "dark"}},
                      script='env | grep -E "^(TERM|COLORTERM|TERM_PROGRAM)=" | sort > "$T2_TMP/env"')
    env = app.wait_file("env")
    report_note("environment", env)
    assert "TERM=xterm-256color" in env and "COLORTERM=truecolor" in env and "TERM_PROGRAM=kuterm" in env
