"""theme colors as painted on screen: bundled dark and light, named themes, theme files, broken ones, system mode"""

import json
import time

import pytest

from harness import App, Portal, as_rgb, bundled_theme, close_to, dominant, rgb, unique_theme

FEATURE = "theme"

# 16 blocks of 3 cells with ansi background colors, then 8 blocks of dim foreground colors
PALETTE = (
    "for i in 0 1 2 3 4 5 6 7; do printf '\\033[4%sm   ' $i; done; printf '\\033[0m'\n"
    "for i in 0 1 2 3 4 5 6 7; do printf '\\033[10%sm   ' $i; done; printf '\\033[0m\\n'\n"
    "for i in 0 1 2 3 4 5 6 7; do printf '\\033[2;3%sm\\342\\226\\210\\342\\226\\210\\342\\226\\210' $i; done;"
    " printf '\\033[0m\\n'\n"
    "printf '\\033[2m\\342\\226\\210\\342\\226\\210\\342\\226\\210\\033[0m\\n'\n"
)

NONE, PREFER_DARK, PREFER_LIGHT = 0, 1, 2


def check_palette(app: App, theme: dict) -> None:
    """every ansi, dim and background color of the theme is on screen where printed"""
    app.wait(lambda: close_to(app.cells_color(45, 0, 3), theme["ansi"][15], 3), msg="palette printed")
    # the system color scheme may arrive after the first frame
    app.wait(lambda: close_to(dominant(app.shot()), theme["terminal_background"]), msg="theme background")
    img = app.shot()
    assert close_to(dominant(img), theme["terminal_background"])
    for i in range(16):
        assert close_to(app.cells_color(i * 3, 0, 3, img), theme["ansi"][i], 3), f"ansi {i}"
    for i in range(8):
        assert close_to(app.cells_color(i * 3, 1, 3, img), theme["ansi_dim"][i], 3), f"ansi_dim {i}"
    assert close_to(app.cells_color(0, 2, 3, img), theme["dim_foreground"], 3), "dim_foreground"


@pytest.mark.parametrize("mode", ["dark", "light"])
def test_bundled_theme(app_factory, mode):
    """bundled theme colors: background, 16 ansi colors, dim colors"""
    app = app_factory({"theme": {"mode": mode}}, script=PALETTE)
    check_palette(app, bundled_theme(mode == "dark"))


@pytest.mark.parametrize("mode", ["dark", "light"])
def test_custom_theme(app_factory, mode):
    """a full custom theme in settings paints every color, tab bar included"""
    theme = unique_theme(0x30 if mode == "dark" else 0x90)
    app = app_factory({"theme": {"mode": mode}, "hide_bar_for_one_tab": False}, script=PALETTE,
                      themes={mode: theme})
    colors = as_rgb(theme)
    top = app.bar_height()
    app.wait(lambda: close_to(app.cells_color(45, 0, 3, top=top), colors["ansi"][15], 3), msg="palette")
    img = app.shot()
    for i in range(16):
        assert close_to(app.cells_color(i * 3, 0, 3, img, top=top), colors["ansi"][i], 3), f"ansi {i}"
    # the only tab is active, the rest of the bar is bar background
    assert close_to(tuple(img[top // 2, 900 - 5]), colors["tab_bar_background"])
    assert close_to(tuple(img[top // 2, 3]), colors["tab_active_background"])
    assert close_to(tuple(img[top - 1, 450]), colors["border"])


@pytest.mark.parametrize("mode", ["dark", "light"])
def test_partial_theme_keeps_bundled_colors(app_factory, mode):
    """a theme with a few colors keeps the bundled ones for the rest"""
    app = app_factory({"theme": {"mode": mode}}, script=PALETTE,
                      themes={mode: {"terminal_background": "#102030"}})
    theme = dict(bundled_theme(mode == "dark"), terminal_background=(0x10, 0x20, 0x30))
    check_palette(app, theme)


@pytest.mark.parametrize("mode", ["dark", "light"])
def test_theme_of_other_mode_is_not_used(app_factory, mode):
    """the custom theme of the other mode does not leak into this one"""
    other = "light" if mode == "dark" else "dark"
    app = app_factory({"theme": {"mode": mode}}, themes={other: unique_theme(0x50)})
    theme = bundled_theme(mode == "dark")
    app.wait(lambda: close_to(dominant(app.shot()), theme["terminal_background"]), msg="bundled background")


@pytest.mark.parametrize("mode", ["dark", "light"])
def test_null_theme_path_uses_bundled(app_factory, mode):
    """a null theme path uses the bundled theme and creates no file"""
    app = app_factory({"theme": {"mode": mode, mode: None}})
    theme = bundled_theme(mode == "dark")
    app.wait(lambda: close_to(dominant(app.shot()), theme["terminal_background"]), msg="bundled background")
    assert not (app.config / f"themes/{mode}.jsonc").exists()


@pytest.mark.parametrize("mode", ["dark", "light"])
def test_theme_file(app_factory, mode):
    """a path instead of a theme name reads that file, relative to the config folder"""
    theme = unique_theme(0x60)
    app = app_factory({"theme": {"mode": mode, mode: "mine/theme.jsonc"}}, script=PALETTE,
                      files={"mine/theme.jsonc": theme})
    check_palette(app, as_rgb(theme))


def test_theme_name_from_themes_folder(app_factory):
    """a name not in "themes" is looked up as a file in the themes folder"""
    theme = unique_theme(0x61)
    app = app_factory({"theme": {"mode": "dark", "dark": "nord-ish"}}, script=PALETTE,
                      files={"themes/nord-ish.jsonc": theme})
    check_palette(app, as_rgb(theme))


@pytest.mark.parametrize("content", ["{", '{"ansi": ["#fff"]}'])
def test_broken_theme_file_falls_back_to_bundled(app_factory, content):
    """an invalid theme file prints an error and paints the bundled theme"""
    app = app_factory({"theme": {"mode": "dark", "dark": "mine.jsonc"}}, script=PALETTE,
                      files={"mine.jsonc": content})
    check_palette(app, bundled_theme(True))
    assert "invalid theme" in app.output()


@pytest.mark.parametrize("colors", [{"cursor": "red"}])
def test_broken_named_theme_falls_back_to_bundled(app_factory, colors):
    """an invalid theme in settings prints an error naming it and paints the bundled theme"""
    app = app_factory({"theme": {"mode": "dark"}}, script=PALETTE, themes={"dark": colors})
    check_palette(app, bundled_theme(True))
    assert 'invalid theme "dark"' in app.output()


def test_unknown_theme_name_falls_back_to_bundled(app_factory):
    """a theme name missing from "themes" is reported, the bundled theme is used"""
    app = app_factory({"theme": {"mode": "dark", "dark": "nope"}}, script=PALETTE)
    check_palette(app, bundled_theme(True))
    assert 'unknown theme "nope"' in app.output()


def test_missing_theme_file_is_reported_not_created(app_factory):
    """a missing theme file is an error, kuterm writes no theme files"""
    app = app_factory({"theme": {"mode": "dark", "dark": "t/d.jsonc"}})
    app.wait(lambda: "t/d.jsonc" in app.output(), msg="error naming the file")
    assert not (app.config / "t/d.jsonc").exists()


@pytest.mark.parametrize("value,expected", [("#f0f", "#ff00ff"), ("#f0f8", "#ff00ff"),
                                            ("#123456", "#123456"), ("#12345678", "#123456")])
def test_hex_color_formats(app_factory, value, expected):
    """#rgb, #rgba, #rrggbb and #rrggbbaa colors are accepted"""
    app = app_factory({"theme": {"mode": "dark"}}, themes={"dark": {"terminal_background": value}})
    # alpha blends with black, so alpha values keep the hue but get darker
    got = dominant(app.shot())
    if len(value) in (5, 9):
        full = rgb(expected)
        assert all(g < f or f == 0 for g, f in zip(got, full)), (got, full)
        assert all((g == 0) == (f == 0) for g, f in zip(got, full)), (got, full)
    else:
        assert close_to(got, rgb(expected))


@pytest.fixture
def portal():
    portals = []

    def start(scheme: int) -> Portal:
        portals.append(Portal(scheme))
        return portals[-1]

    yield start
    for p in portals:
        p.stop()


@pytest.mark.parametrize("scheme,dark", [(PREFER_DARK, True), (PREFER_LIGHT, False)], ids=["dark", "light"])
def test_system_mode_follows_desktop(app_factory, portal, scheme, dark):
    """theme.mode system picks dark or light from the desktop color scheme"""
    portal(scheme)
    app = app_factory({"theme": {"mode": "system"}}, script=PALETTE)
    check_palette(app, bundled_theme(dark))


def test_system_mode_switches_live(app_factory, portal):
    """switching the desktop color scheme recolors the running terminal"""
    p = portal(PREFER_DARK)
    app = app_factory({"theme": {"mode": "system"}}, script=PALETTE)
    check_palette(app, bundled_theme(True))
    app.snap("desktop prefers dark")
    p.set(PREFER_LIGHT)
    check_palette(app, bundled_theme(False))
    app.snap("desktop switched to light")
    p.set(PREFER_DARK)
    check_palette(app, bundled_theme(True))


@pytest.mark.parametrize("mode", ["dark", "light"])
def test_fixed_mode_ignores_desktop(app_factory, portal, mode):
    """theme.mode dark or light stays put when the desktop switches"""
    p = portal(PREFER_LIGHT if mode == "dark" else PREFER_DARK)
    app = app_factory({"theme": {"mode": mode}})
    theme = bundled_theme(mode == "dark")
    app.wait(lambda: close_to(dominant(app.shot()), theme["terminal_background"]), msg="fixed theme")
    p.set(PREFER_DARK if mode == "dark" else PREFER_LIGHT)
    p.set(PREFER_LIGHT if mode == "dark" else PREFER_DARK)
    # the switch arrives over dbus a moment later, give it time to be (wrongly) applied
    time.sleep(1)
    assert close_to(dominant(app.shot()), theme["terminal_background"])


def test_pick_theme_previews_keeps_and_goes_back(app_factory):
    """the theme picker shows the highlighted theme, escape puts the old one back, enter writes it to settings"""
    dark = bundled_theme(True)
    app = app_factory({"theme": {"mode": "dark"}})
    background = lambda: tuple(app.shot()[500, 100])
    app.wait(lambda: close_to(background(), dark["terminal_background"]), msg="bundled dark")
    app.palette("pick theme")
    app.type("nord")
    app.wait(lambda: close_to(background(), rgb("#2e3440")), msg="nord previewed")
    app.snap("nord previewed in the theme picker")
    app.key("Escape")
    app.wait(lambda: close_to(background(), dark["terminal_background"]), msg="back to the bundled theme")
    app.palette("pick theme")
    app.type("dracula")
    app.wait(lambda: close_to(background(), rgb("#282a36")), msg="dracula previewed")
    app.key("Return")
    app.wait(lambda: '"dracula"' in (app.config / "settings.jsonc").read_text(), msg="dracula kept in settings")
    assert json.loads((app.config / "settings.jsonc").read_text())["theme"]["dark"] == "dracula"
    # the reload from the saved file keeps it
    time.sleep(1)
    assert close_to(background(), rgb("#282a36"))
