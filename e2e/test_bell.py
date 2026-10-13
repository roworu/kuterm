"""bell rung by programs: visual flash, tab indicator"""

import time

from harness import App, bundled_theme, ink, near

FEATURE = "bell"

DARK = bundled_theme(True)
MAGENTA = (255, 0, 255)
RING = "step 1; printf '\\a'"


def flashing(app: App) -> bool:
    """the middle of the terminal is tinted magenta"""
    patch = app.shot()[250:350, 400:500]
    return near(patch, MAGENTA, 60).mean() > 0.5


def test_visual_bell_flashes_and_fades(app_factory):
    """bell.visual paints a flash in its color over the terminal, which fades out"""
    app = app_factory({"theme": {"mode": "dark"},
                       "bell": {"visual": {"enable": True, "color": "#ff00ff", "duration": 1500}}}, script=RING)
    time.sleep(0.5)
    assert not flashing(app)
    app.step(1)
    app.wait(lambda: flashing(app), msg="magenta flash")
    app.snap("bell rang, flash shown")
    app.wait(lambda: near(app.shot()[250:350, 400:500], DARK["terminal_background"], 3).mean() > 0.9,
             msg="flash faded", timeout=5)


def test_visual_bell_off(app_factory):
    """with bell.visual.enable false the terminal does not flash"""
    app = app_factory({"theme": {"mode": "dark"},
                       "bell": {"visual": {"enable": False, "color": "#ff00ff", "duration": 1500}}}, script=RING)
    time.sleep(0.5)
    app.step(1)
    deadline = time.monotonic() + 1
    while time.monotonic() < deadline:
        assert not flashing(app)
        time.sleep(0.05)


def bell_ink(app: App, background: tuple) -> int:
    """ink at the right end of the first tab, where its bell icon sits"""
    return int(ink(app.shot()[: app.bar_height() - 2, 150:195], background).sum())


def test_tab_indicator(app_factory):
    """a background tab that rang shows a bell icon until it is opened"""
    for indicator in (True, False):
        app = app_factory({"theme": {"mode": "dark"}, "window_title": ["number"], "tab_width": 200,
                           "tab_title": [{"text": "t"}],
                           "show_tab_close_button": False,
                           "bell": {"tab_indicator": indicator, "visual": {"enable": False}}},
                          script=RING, name=f"tab_indicator_{indicator}")
        app.mouse(450, 500)
        app.key("ctrl+shift+t")
        app.wait_title("2")
        time.sleep(0.5)
        background = DARK["tab_bar_background"]
        before = bell_ink(app, background)
        # both tabs run the script, the bell of the first one rings in the background
        app.step(1)
        if indicator:
            app.wait(lambda: bell_ink(app, background) > before + 20, msg="bell icon on the first tab")
            app.snap("background tab rang")
            app.click(100, app.bar_height() // 2)
            app.wait_title("1")
            app.wait(lambda: bell_ink(app, DARK["tab_active_background"]) <= before + 5, msg="bell icon cleared")
            app.snap("tab opened, bell icon gone")
        else:
            time.sleep(1)
            assert bell_ink(app, background) <= before + 5
        app.close()
