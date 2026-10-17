"""split panes: dimmed inactive panes, zoom, focus moves by direction"""

from harness import bundled_theme, close_to, median

FEATURE = "split panes"

DARK = bundled_theme(True)


def test_dim_inactive_and_zoom(app_factory):
    """the pane without focus is darkened, zooming shows the focused pane alone"""
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["number"],
                       "panes": {"dim_inactive": 0.6, "divider_width": 4}},
                      keybindings={"zoom_pane": "ctrl-shift-z", "focus_pane_left": "ctrl-shift-h",
                                   "split_right": "ctrl-shift-r"})
    app.key("ctrl+shift+t")
    app.wait_title("2")
    app.key("ctrl+shift+r")
    app.wait(lambda: not close_to(median(app.shot()[300:400, 100:300]), DARK["terminal_background"], 3),
             msg="left pane dimmed")
    right = app.shot()[300:400, 600:800]
    app.snap("split right, the left pane is dimmed")
    assert close_to(median(right), DARK["terminal_background"], 3), median(right)
    app.key("ctrl+shift+h")
    app.wait(lambda: close_to(median(app.shot()[300:400, 100:300]), DARK["terminal_background"], 3),
             msg="left pane focused and lit")
    app.key("ctrl+shift+z")
    # the zoomed pane covers the whole window, nothing dimmed is left
    app.wait(lambda: close_to(median(app.shot()[300:400, 600:800]), DARK["terminal_background"], 3),
             msg="left pane zoomed")
    app.snap("left pane zoomed")
