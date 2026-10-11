"""config files at startup and command line options: created defaults, broken files, partial files, cli flags"""

import subprocess
import time

import pytest

from harness import ASSETS, BIN, BASH, App, bundled_theme, close_to, dominant, near, session_env

FEATURE = "config files and command line"

DARK = bundled_theme(True)
LIGHT = bundled_theme(False)

# config file name to the bundled asset it is created from
CREATED = {
    "settings.jsonc": "default_settings.jsonc",
}



def background(app: App) -> tuple:
    return dominant(app.shot())


def test_missing_config_folder_is_created_with_bundled_files(app_factory):
    """first launch writes only settings.jsonc with the bundled defaults"""
    app = app_factory(None)
    for name, asset in CREATED.items():
        path = app.config / name
        app.wait(path.exists, msg=f"{name} to be created")
        assert path.read_text() == (ASSETS / asset).read_text(), name
    assert [p.name for p in app.config.iterdir()] == ["settings.jsonc"]


def test_missing_config_home_uses_home_dot_config(app_factory):
    """without XDG_CONFIG_HOME the config goes to ~/.config/kuterm"""
    app = app_factory(None, env={"XDG_CONFIG_HOME": None})
    path = app.home / ".config/kuterm/settings.jsonc"
    app.wait(path.exists, msg="settings in ~/.config")
    assert not (app.config / "settings.jsonc").exists()


@pytest.mark.parametrize("content", ["{", "not json", "[]", '{"terminal": {"cursor_shape": "triangle"}}',
                                     '{"theme": {"mode": "auto"}}'])
def test_broken_settings_file_starts_with_defaults(app_factory, content):
    """an invalid settings file prints an error, keeps the file and runs with defaults"""
    app = app_factory(content)
    app.wait(lambda: "invalid settings" in app.output(), msg="error on stderr")
    assert (app.config / "settings.jsonc").read_text() == content
    assert app.alive()


def test_lenient_syntax_and_partial_settings(app_factory):
    """comments, trailing commas and a partial file apply, other values keep defaults"""
    text = """{
      // only the theme mode and the shell are set
      "theme": { "mode": "dark", },
      "profiles": [{ "name": "bash", "command": %s }],
    }""" % __import__("json").dumps(BASH)
    app = app_factory(text)
    app.wait(lambda: close_to(background(app), DARK["terminal_background"]), msg="dark background")
    assert app.pty_size() == app.expected_pty(900, 600)


def test_valid_settings_file_is_not_rewritten(app_factory):
    """a user file is read, never overwritten with defaults"""
    app = app_factory({"theme": {"mode": "light"}})
    before = (app.config / "settings.jsonc").read_text()
    app.wait(lambda: close_to(background(app), LIGHT["terminal_background"]), msg="light background")
    assert (app.config / "settings.jsonc").read_text() == before


@pytest.mark.parametrize("value,expected", [(1, "default"), (500, 72)])
def test_out_of_range_font_size_is_limited(app_factory, value, expected):
    """font sizes below the limit use the default, above it are capped, with a message"""
    app = app_factory({"terminal": {"font_size": value}})
    app.wait(lambda: "terminal.font_size" in app.output(), msg="limit message")
    probe = App({"terminal": {"font_size": expected}} if expected != "default" else {}, start=False)
    assert app.pty_size() == probe.expected_pty(900, 600)
    probe.cleanup()


def test_help_prints_usage(report_note):
    """--help prints the options and exits without a window"""
    res = subprocess.run([str(BIN), "--help"], capture_output=True, text=True, timeout=10, env=session_env())
    report_note("kuterm --help", res.stdout)
    assert res.returncode == 0
    assert "usage: kuterm" in res.stdout and "--config-file" in res.stdout


def test_unknown_option_fails(report_note):
    """an unknown option prints the error and usage and exits with 2"""
    res = subprocess.run([str(BIN), "--nope"], capture_output=True, text=True, timeout=10, env=session_env())
    report_note(f"kuterm --nope, exit code {res.returncode}", res.stderr)
    assert res.returncode == 2
    assert "--nope" in res.stderr and "usage: kuterm" in res.stderr


def test_config_file_option(app_factory):
    """--config-file reads settings from another file"""
    app = app_factory({"theme": {"mode": "light"}}, start=False)
    other = app.write("other/settings.jsonc", {"theme": {"mode": "dark"}, "profiles": [
        {"name": "bash", "command": BASH}]})
    app.args = ["--config-file", str(other)]
    app.start()
    app.wait(lambda: close_to(background(app), DARK["terminal_background"]), msg="dark from the other file")


@pytest.mark.parametrize("mode", ["dark", "light"])
def test_theme_mode_option_overrides_settings(app_factory, mode):
    """--theme-mode wins over theme.mode in settings"""
    other = "light" if mode == "dark" else "dark"
    app = app_factory({"theme": {"mode": other}}, args=["--theme-mode", mode])
    theme = DARK if mode == "dark" else LIGHT
    app.wait(lambda: close_to(background(app), theme["terminal_background"]), msg=f"{mode} background")


def test_theme_file_options(app_factory):
    """--dark-theme-file and --light-theme-file replace the theme files from settings"""
    app = app_factory({"theme": {"mode": "dark"}}, start=False)
    path = app.write("elsewhere/mine.jsonc", {"terminal_background": "#123456"})
    app.args = ["--dark-theme-file", str(path)]
    app.start()
    app.wait(lambda: close_to(background(app), (0x12, 0x34, 0x56)), msg="background from the option file")


def test_keybindings_file_option(app_factory):
    """--keybindings-file reads keys from another file"""
    app = app_factory({"window_title": ["number"]}, start=False)
    path = app.write("elsewhere/keys.jsonc", {"new_tab": "ctrl-shift-y"})
    app.args = ["--keybindings-file", str(path)]
    app.start()
    app.key("ctrl+shift+y")
    app.wait_title("2")


def test_recreate_confs_rewrites_defaults(app_factory):
    """--recreate-confs replaces changed config files with the bundled ones"""
    app = app_factory({"theme": {"mode": "dark"}}, keybindings={"new_tab": None}, args=["--recreate-confs"])
    settings = app.config / "settings.jsonc"
    app.wait(lambda: settings.read_text() == (ASSETS / "default_settings.jsonc").read_text(),
             msg="settings rewritten")


def test_broken_settings_error_is_shown_in_the_window(app_factory):
    """an invalid settings file at launch shows its error inside the window too"""
    app = app_factory('{"theme": {"mode": "dark"}, "tab_width": "wide"}')
    # defaults run, so the error border has the bundled theme's danger color
    theme = DARK if close_to(background(app), DARK["terminal_background"]) else LIGHT
    app.wait(lambda: near(app.shot()[300:, 400:], theme["danger_button"]).sum() > 100, msg="error notification")
    app.snap("launch error for a broken settings file")


def test_old_split_files_are_ignored(app_factory):
    """keybindings.jsonc from older versions is not read, keys live in settings.jsonc only"""
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["number"]},
                      files={"keybindings.jsonc": {"new_tab": "ctrl-shift-y"}})
    app.key("ctrl+shift+y")
    time.sleep(1.5)
    assert app.title() == "1"
    app.key("ctrl+shift+t")
    app.wait_title("2")


def test_os_block_overrides_on_this_os(app_factory):
    """the "os" block of the running system is merged last, other systems are ignored"""
    app = app_factory({"theme": {"mode": "light"}, "window_title": ["number"], "os": {
        "linux": {"theme": {"mode": "dark"}}, "macos": {"window_title": ["folder"]}}})
    app.wait(lambda: close_to(background(app), DARK["terminal_background"]), msg="dark from the linux block")
    app.key("ctrl+shift+t")
    app.wait_title("2")
