"""the themes kuterm brings: every bundled theme by name in either slot, a theme of your own
shadowing one, and what the theme picker lists, previews and writes back into settings.jsonc"""

import json
import time
from pathlib import Path

import pytest

from harness import (ASSETS, App, Portal, as_rgb, bundled_theme, close_to, jsonc, unique_theme)
from test_theme import PALETTE, check_palette

FEATURE = "bundled themes and the theme picker"

# the names kuterm brings, themes/<name>.jsonc ships them
GALLERY = ["catppuccin-latte", "catppuccin-mocha", "dracula", "gruvbox-dark", "gruvbox-light",
           "nord", "solarized-dark", "solarized-light", "tokyo-night"]
DARK_ONLY = ["catppuccin-mocha", "dracula", "gruvbox-dark", "nord", "solarized-dark", "tokyo-night"]
LIGHT_ONLY = ["catppuccin-latte", "gruvbox-light", "solarized-light"]


def bundled(name: str) -> dict:
    """the colors of a bundled theme file, as rgb tuples"""
    return as_rgb(jsonc(Path(ASSETS) / "themes" / f"{name}.jsonc"))


def background(app: App) -> tuple:
    return tuple(app.shot()[500, 100])


@pytest.fixture
def portal():
    portals = []

    def start(scheme: int) -> Portal:
        portals.append(Portal(scheme))
        return portals[-1]

    yield start


def settings_text(app: App) -> str:
    return (app.config / "settings.jsonc").read_text()


@pytest.mark.parametrize("name", DARK_ONLY)
def test_a_bundled_theme_in_the_dark_slot(app_factory, name):
    """every theme kuterm ships paints all of its colors when named as the dark theme"""
    app = app_factory({"theme": {"mode": "dark", "dark": name}}, script=PALETTE)
    check_palette(app, bundled(name))
    app.snap(f"{name} as the dark theme")


@pytest.mark.parametrize("name", LIGHT_ONLY)
def test_a_bundled_theme_in_the_light_slot(app_factory, name):
    """the light themes of the gallery paint the same way in light mode"""
    app = app_factory({"theme": {"mode": "light", "light": name}}, script=PALETTE)
    check_palette(app, bundled(name))
    app.snap(f"{name} as the light theme")


def test_your_own_theme_shadows_a_bundled_name(app_factory):
    """a theme in "themes" wins over the bundled theme of the same name"""
    mine = unique_theme(0x44)
    app = app_factory({"theme": {"mode": "dark", "dark": "dracula"}}, themes={"dracula": mine},
                      script=PALETTE)
    check_palette(app, as_rgb(mine))
    app.snap("dracula from settings, not the bundled one")


def test_the_picker_lists_the_bundled_themes_and_previews_each(app_factory):
    """typing a bundled name in the picker previews every theme that matches"""
    app = app_factory({"theme": {"mode": "dark"}, "hide_bar_for_one_tab": False})
    app.palette("pick theme")
    app.type("gruvbox")
    app.wait(lambda: close_to(background(app), bundled("gruvbox-dark")["terminal_background"], 3),
             msg="gruvbox-dark previewed")
    app.snap("gruvbox-dark previewed from the picker")
    app.key("Down")
    app.wait(lambda: close_to(background(app), bundled("gruvbox-light")["terminal_background"], 3),
             msg="gruvbox-light previewed")
    app.snap("gruvbox-light previewed, one arrow down")


def test_the_picker_starts_on_the_theme_in_use(app_factory):
    """the picker puts the current theme first, so enter without moving keeps it"""
    app = app_factory({"theme": {"mode": "dark", "dark": "nord"}}, script=PALETTE)
    check_palette(app, bundled("nord"))
    before = settings_text(app)
    app.palette("pick theme")
    time.sleep(1)
    app.key("Return")
    app.wait_still(msg="after keeping the current theme")
    assert close_to(background(app), bundled("nord")["terminal_background"], 3)
    assert settings_text(app) == before, "keeping the same theme rewrote the file"


def test_the_picker_writes_the_slot_of_the_mode_in_use(app_factory):
    """in light mode the picker writes theme.light and leaves theme.dark alone"""
    app = app_factory({"theme": {"mode": "light", "dark": "nord"}})
    app.palette("pick theme")
    app.type("tokyo-night")
    app.wait(lambda: close_to(background(app), bundled("tokyo-night")["terminal_background"], 3),
             msg="tokyo-night previewed")
    app.key("Return")
    app.wait(lambda: json.loads(settings_text(app))["theme"].get("light") == "tokyo-night",
             msg="light slot written")
    saved = json.loads(settings_text(app))["theme"]
    assert saved["dark"] == "nord", "the dark slot was touched in light mode"
    assert app.settings["theme"]["mode"] == "light"


def test_the_picker_keeps_the_comments_of_the_settings_file(app_factory):
    """picking a theme edits the settings file in place: comments and other keys stay"""
    app = app_factory("""
{
  // how the window looks
  "theme": {
    "mode": "dark",
    // the theme in use
    "dark": "nord"
  },
  "terminal": { "font_size": 15 },
}
""")
    app.palette("pick theme")
    app.type("solarized-dark")
    app.wait(lambda: close_to(background(app), bundled("solarized-dark")["terminal_background"], 3),
             msg="solarized-dark previewed")
    app.key("Return")
    app.wait(lambda: '"dark": "solarized-dark"' in settings_text(app), msg="written")
    text = settings_text(app)
    assert "// how the window looks" in text and "// the theme in use" in text
    assert '"font_size": 15' in text
    saved = jsonc(app.config / "settings.jsonc")
    assert saved["theme"]["dark"] == "solarized-dark"
    assert saved["terminal"]["font_size"] == 15


def test_the_picker_preview_does_not_write_anything(app_factory):
    """escape after previewing leaves the settings file and the colors as they were"""
    app = app_factory({"theme": {"mode": "dark", "dark": "nord"}})
    before = settings_text(app)
    app.palette("pick theme")
    app.type("catppuccin")
    app.wait(lambda: close_to(background(app), bundled("catppuccin-latte")["terminal_background"], 3),
             msg="catppuccin-latte previewed")
    app.snap("catppuccin-latte previewed, then escape puts nord back")
    app.key("Escape")
    app.wait(lambda: close_to(background(app), bundled("nord")["terminal_background"], 3),
             msg="nord back")
    assert settings_text(app) == before


def test_the_picker_writes_the_slot_the_desktop_mode_points_at(app_factory, portal):
    """with theme.mode system the picker keeps the theme for the mode the desktop shows"""
    p = portal(1)  # the fake portal prefers dark
    app = app_factory({"theme": {"mode": "system", "light": "gruvbox-light"}})
    app.wait(lambda: close_to(background(app), bundled_theme(True)["terminal_background"], 3),
             msg="bundled dark")
    app.palette("pick theme")
    app.type("solarized")
    app.wait(lambda: close_to(background(app), bundled("solarized-dark")["terminal_background"], 3),
             msg="solarized-dark previewed")
    app.key("Return")
    app.wait(lambda: json.loads(settings_text(app))["theme"].get("dark") == "solarized-dark",
             msg="dark slot written")
    assert json.loads(settings_text(app))["theme"]["light"] == "gruvbox-light"
    p.set(2)
    app.wait(lambda: close_to(background(app), bundled("gruvbox-light")["terminal_background"], 3),
             msg="desktop light shows the light theme")
    app.snap("the desktop switched to light, the running theme follows")


def test_a_theme_of_your_own_is_listed_and_kept(app_factory):
    """a theme from settings is in the picker next to the bundled ones and can be kept"""
    mine = unique_theme(0x51)
    app = app_factory({"theme": {"mode": "dark", "themes": {"mine": mine}}})
    app.palette("pick theme")
    app.type("mine")
    app.wait(lambda: close_to(background(app), as_rgb(mine)["terminal_background"], 3),
             msg="own theme previewed")
    app.key("Return")
    app.wait(lambda: json.loads(settings_text(app))["theme"].get("dark") == "mine", msg="kept")
    app.snap("a theme of your own kept from the picker")


def test_the_picker_opens_from_a_key_and_runs_over_the_socket(app_factory):
    """pick_theme is a normal action: a key opens it, kuterm msg can run it too"""
    from kit import integrated, msg
    app = app_factory(integrated(keybindings={"pick_theme": "ctrl-shift-y"},
                                 window_title=["number"]), themes={"dark": unique_theme(0x61)})
    before = settings_text(app)
    closed = app.shot()[app.top():, 400:500]
    app.key("ctrl+shift+y")
    app.wait(lambda: (app.shot()[app.top():, 400:500] != closed).any(), msg="picker opens", timeout=5)
    app.key("Escape")
    app.wait_still(msg="closed again")
    assert settings_text(app) == before
    assert msg(app, "run-command", "pick theme").returncode == 0
    time.sleep(1)
    app.key("Escape")
    app.wait_still(msg="closed the picker the socket opened")
    # an unknown command name is refused, the picker does not open
    assert msg(app, "run-command", "pick a colour scheme").returncode != 0
