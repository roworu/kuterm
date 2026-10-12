"""open_scrollback: the history goes to a pager in a new tab, colors kept"""

from harness import bundled_theme, near

FEATURE = "scrollback in a pager"

DARK = bundled_theme(True)


def test_history_opens_in_less(app_factory):
    """the palette command opens less on the history in a new tab, red text stays red"""
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["number", {"text": " "}, "command"]},
                      script="i=0; while [ $i -lt 80 ]; do echo line_$i; i=$((i+1)); done; "
                             "printf '\\033[31mred_text\\033[0m\\n'")
    app.wait(lambda: near(app.shot(), DARK["ansi"][1], 30).sum() > 20, msg="red text")
    app.palette("open history in pager")
    app.wait_title("2 less")
    # less starts at the end of the file, where the red line is
    app.wait(lambda: near(app.shot(), DARK["ansi"][1], 30).sum() > 20, msg="red text in less")
    app.snap("history in less, colors kept")
