"""search bar: highlights matches in the history, walks them, scrolls to them, closes"""

from harness import App, bundled_theme, near

FEATURE = "search"

DARK = bundled_theme(True)

# numbered lines, so "row_7" is near the top of a long history
SCRIPT = "i=0; while [ $i -lt 300 ]; do echo row_$i; i=$((i+1)); done; echo all_printed"


def highlighted(app: App, key: str) -> int:
    """pixels painted in a search color of the theme"""
    return int(near(app.shot(), DARK[key], 2).sum())


def searching(app_factory, **search) -> App:
    app = app_factory({"theme": {"mode": "dark"}, "search": search}, script=SCRIPT)
    app.wait(lambda: near(app.shot(), DARK["terminal_foreground"], 30).any(), msg="output")
    return app


def test_search_highlights_and_walks_matches(app_factory):
    """ctrl-shift-f opens the bar, matches get the search colors, enter moves, escape closes"""
    app = searching(app_factory)
    app.key("ctrl+shift+f")
    app.type("row_29")
    app.wait(lambda: highlighted(app, "search_match_active") > 50, msg="current match")
    app.wait(lambda: highlighted(app, "search_match") > 50, msg="other matches")
    app.snap("matches of row_29, the current one in its own color")
    before = app.shot()
    app.key("Return")
    app.wait(lambda: (app.shot() != before).any(), msg="moved to the match above")
    app.key("Escape")
    app.wait(lambda: highlighted(app, "search_match_active") == 0 and highlighted(app, "search_match") == 0,
             msg="highlights gone")


def test_search_scrolls_to_history(app_factory):
    """a match far up in the history scrolls the view to it"""
    app = searching(app_factory, whole_word=True)
    app.key("ctrl+shift+f")
    app.type("row_7")
    app.wait(lambda: highlighted(app, "search_match_active") > 50, msg="match scrolled into view")
    app.snap("view scrolled up to row_7")
    # whole words only, so row_70 and the others are not matches
    assert highlighted(app, "search_match") == 0
