"""config reload: saved settings, imported and theme files apply live, broken ones keep the last good config"""

import time

from harness import App, as_rgb, bundled_theme, close_to, dominant, near, unique_theme

FEATURE = "config reload"

DARK = bundled_theme(True)
LIGHT = bundled_theme(False)
THEME = unique_theme(0xb0)
C = as_rgb(THEME)


def reload_app(app_factory, **settings) -> App:
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["number"], **settings})
    app.mouse(100, 100)
    return app


def bg(app: App) -> tuple:
    return dominant(app.shot())


def error_shown(app: App, color: tuple) -> bool:
    """an error notification has a border in the danger color in the bottom right corner"""
    return near(app.shot()[300:, 400:], color).sum() > 100


def test_saved_settings_apply_live(app_factory):
    """saving settings.jsonc applies it without a command or a restart"""
    app = reload_app(app_factory)
    app.wait(lambda: close_to(bg(app), DARK["terminal_background"]), msg="dark")
    app.write("settings.jsonc", {**app.settings, "theme": {"mode": "light"}, "window_title": [{"text": "reloaded"}]})
    app.wait(lambda: close_to(bg(app), LIGHT["terminal_background"]), msg="light after save")
    app.wait_title("reloaded")
    app.snap("settings applied on save")


def test_saved_theme_applies_live(app_factory):
    """saving a changed theme in settings recolors the terminal"""
    app = reload_app(app_factory)
    app.wait(lambda: close_to(bg(app), DARK["terminal_background"]), msg="dark")
    theme = {**app.settings["theme"], "themes": {"dark": {"terminal_background": "#204060"}}}
    app.write("settings.jsonc", {**app.settings, "theme": theme})
    app.wait(lambda: close_to(bg(app), (0x20, 0x40, 0x60)), msg="new background")
    app.snap("theme applied on save")


def test_saved_theme_file_applies_live(app_factory):
    """saving a theme file picked by path recolors the terminal"""
    app = app_factory({"theme": {"mode": "dark", "dark": "mine.jsonc"}},
                      files={"mine.jsonc": {"terminal_background": "#102030"}})
    app.wait(lambda: close_to(bg(app), (0x10, 0x20, 0x30)), msg="file background")
    app.write("mine.jsonc", {"terminal_background": "#204060"})
    app.wait(lambda: close_to(bg(app), (0x20, 0x40, 0x60)), msg="new background")


def test_saved_keybindings_apply_live(app_factory):
    """keybindings in settings.jsonc move actions to their new keys on save"""
    app = reload_app(app_factory)
    app.write("settings.jsonc", {**app.settings, "keybindings": {"new_tab": "ctrl-shift-y"}})
    # saves are polled, then read once they settle
    time.sleep(1)
    app.key("ctrl+shift+y")
    app.wait_title("2")
    app.key("ctrl+shift+t")
    time.sleep(1.5)
    assert app.title() == "2"


def test_imported_file_applies_live(app_factory):
    """settings split into an imported file, saving that file applies it"""
    app = app_factory({"import": ["looks.jsonc"], "window_title": ["number"]},
                      files={"looks.jsonc": {"theme": {"mode": "dark"}}})
    app.wait(lambda: close_to(bg(app), DARK["terminal_background"]), msg="dark from the import")
    app.write("looks.jsonc", {"theme": {"mode": "light"}})
    app.wait(lambda: close_to(bg(app), LIGHT["terminal_background"]), msg="light after saving the import")
    app.snap("imported file applied on save")


def test_broken_file_keeps_config_and_shows_error(app_factory):
    """a broken save keeps the running config and shows the error in the window until fixed"""
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["number"], "notifications": {"timeout": 1}},
                      themes={"dark": THEME})
    app.wait(lambda: close_to(bg(app), C["terminal_background"]), msg="unique theme")
    good = dict(app.settings)
    app.write("settings.jsonc", '{ "theme": { "mode": "light" }, "tab_width": "wide" }')
    app.wait(lambda: error_shown(app, C["danger_button"]), msg="error notification")
    app.snap("error for a broken settings file")
    assert "invalid settings" in app.output() and "wide" in app.output()
    # still dark, the error stays past the notification timeout
    time.sleep(2)
    assert close_to(bg(app), C["terminal_background"])
    assert error_shown(app, C["danger_button"])

    app.write("settings.jsonc", {**good, "window_title": [{"text": "fixed"}]})
    app.wait_title("fixed")
    app.wait(lambda: not error_shown(app, C["danger_button"]), msg="error cleared")


def test_reload_config_command(app_factory):
    """"config: reload config" in the palette reads the files again and says so"""
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["number"]}, themes={"dark": THEME})
    app.mouse(100, 100)
    app.palette("config: reload config")
    app.wait(lambda: near(app.shot()[300:, 400:], C["tab_bar_background"]).sum() > 300, msg="reloaded notification")
    app.snap("config reloaded from the palette")
