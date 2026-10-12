"""keyboard selection mode: vi keys move a cursor over the history, select and copy"""

from harness import bundled_theme, get_clipboard, near

FEATURE = "keyboard selection"

DARK = bundled_theme(True)


def test_vi_mode_selects_and_copies(app_factory):
    """ctrl-shift-space starts vi mode, gg w v e selects a word, y copies it and leaves"""
    app = app_factory({"theme": {"mode": "dark"}}, script="printf 'alpha bravo charlie\\n'")
    app.wait(lambda: near(app.shot()[:60], DARK["terminal_foreground"], 30).any(), msg="text")
    app.key("ctrl+shift+space")
    for key in ["g", "g", "w", "v", "e"]:
        app.key(key)
    app.wait(lambda: near(app.shot(), DARK["selection"], 2).sum() > 100, msg="selected word")
    app.snap("vi mode selection of a word")
    app.key("y")
    app.wait(lambda: get_clipboard() == "bravo", msg=f"clipboard, got {get_clipboard()!r}")
    app.wait(lambda: near(app.shot(), DARK["selection"], 2).sum() == 0, msg="selection gone")
