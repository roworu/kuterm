"""profiles: command, working directory, env, theme, icon, default profile and the profile menu"""

import time

import numpy as np

from harness import BASH, App, as_rgb, close_to, dominant, ink, mask_bbox, near, unique_theme

FEATURE = "profiles"

THEME = unique_theme(0xa0)
C = as_rgb(THEME)


def profile(name: str, **kwargs) -> dict:
    return {"name": name, "command": BASH, **kwargs}


def profiled(app_factory, profiles: list, **settings) -> App:
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["number", {"text": " "}, "folder"],
                       "profiles": profiles, **settings}, themes={"dark": THEME})
    app.mouse(450, 500)
    return app


def test_profile_command_with_arguments(app_factory):
    """the profile command runs with its arguments"""
    app = app_factory({"theme": {"mode": "dark"}, "profiles": [{"name": "script", "command": {
        "with_arguments": {"program": "/bin/sh", "args": ["-c", 'echo "$0 $1" | tee "$T2_TMP/out"; sleep 100', "a", "b"]}}}]})
    app.wait_file("out", "a b")
    app.wait_line(0, 3)
    app.snap('the profile printed its arguments "a b"')


def test_profile_program(app_factory):
    """{"program": ...} runs that program without arguments"""
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["command"],
                       "profiles": [{"name": "sh", "command": {"program": "/bin/sh"}}]})
    app.wait_title("sh")


def test_system_shell_is_the_login_shell(app_factory):
    """"system" runs the login shell of the user"""
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["command"],
                       "profiles": [{"name": "system", "command": "system"}]})
    app.wait_title("bash")


def test_working_directory_and_env(app_factory):
    """working_directory with ~ and env are applied to the shell"""
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["folder"],
                       "profiles": [profile("work", working_directory="~/work",
                                            env={"KUTERM_E2E": "from profile"})]}, start=False)
    (app.home / "work").mkdir()
    app.start()
    app.wait_title("work")
    app.run(f'echo "$KUTERM_E2E $TERM_PROGRAM $TERM" > {app.file("env")}')
    app.wait_file("env", "from profile kuterm xterm-256color")


def test_default_profile_opens_on_new_tab(app_factory):
    """the profile marked default opens with ctrl-shift-t"""
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["number", {"text": " "}, "folder"],
                       "profiles": [profile("first"), profile("second", default=True, working_directory="/tmp")]})
    app.wait_title("1 tmp")
    app.key("ctrl+shift+t")
    app.wait_title("2 tmp")


def test_profile_theme_colors_its_tab(app_factory):
    """a profile theme colors its own tabs, others keep the global theme"""
    other = unique_theme(0x20)
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["number"], "tab_width": 200,
                       "profiles": [profile("plain"), profile("red", default=False,
                                                              theme={"mode": "dark", "dark": "other"})],
                       "command_palette": {"enable": True}},
                      themes={"dark": THEME, "other": other},
                      commands={"commands": [{"name": "open red", "actions": [{"new_tab_with_profile": "red"}]}]})
    app.wait(lambda: close_to(dominant(app.shot()), C["terminal_background"]), msg="global theme")
    app.snap("default profile, global theme")
    app.palette("open red")
    app.wait_title("2")
    app.wait(lambda: close_to(dominant(app.shot()), as_rgb(other)["terminal_background"]), msg="profile theme")
    h = app.bar_height()

    def tabs_are(first: tuple, second: tuple) -> bool:
        img = app.shot()
        return close_to(dominant(img[2 : h - 2, 10:190]), first) and close_to(dominant(img[2 : h - 2, 210:390]), second)

    app.wait(lambda: tabs_are(C["tab_bar_background"], as_rgb(other)["terminal_background"]),
             msg="active profile tab in its background")
    app.snap("profile tab, its own theme")
    app.key("alt+1")
    app.wait_title("1")
    app.wait(lambda: close_to(dominant(app.shot()), C["terminal_background"]), msg="global theme again")
    app.wait(lambda: tabs_are(C["tab_active_background"], as_rgb(other)["terminal_background"]),
             msg="inactive profile tab in its background")
    app.snap("profile tab stays in its theme while inactive")


def test_profile_icon_is_kept(app_factory):
    """a profile icon stays whatever runs in the tab"""
    app = app_factory({"theme": {"mode": "dark"}, "hide_bar_for_one_tab": False, "tab_title": ["icon", {"text": "t"}],
                       "profiles": [profile("iconic", icon="")]},
                      themes={"dark": THEME},
                      tab_icons={"groups": [{"icon": "", "commands": ["sleep"]}]})
    app.mouse(450, 500)
    h = app.bar_height()
    app.wait(lambda: mask_bbox(ink(app.shot()[1 : h - 2, :100], C["tab_active_background"])), msg="icon")
    time.sleep(1.5)
    before = app.shot()[1 : h - 2, :100].copy()
    app.run("sleep 30")
    time.sleep(2.5)
    assert (app.shot()[1 : h - 2, :100] == before).all()


def test_right_click_on_plus_picks_a_profile(app_factory):
    """with several profiles a right click on "+" lists them, picking one opens it"""
    app = profiled(app_factory, [profile("home"), profile("temp", working_directory="/tmp")],
                   hide_bar_for_one_tab=False, tab_width=200)
    h = app.bar_height()
    app.wait_title("1 ~")
    # "+" and its profile hint sit right after the only tab
    plus = mask_bbox(ink(app.shot()[1 : h - 2, 200:300], C["tab_bar_background"]))
    assert plus is not None
    app.click(200 + (plus[0] + plus[2]) // 2, h // 2, button=3)

    def menu():
        img = app.shot()
        below = near(img[h:], C["tab_bar_background"], 2)
        return mask_bbox(below)

    box = app.wait(menu, msg="profile menu")
    app.snap("profile menu open")
    x0, y0, x1, y1 = box
    # inside the menu border
    img = app.shot()[h + y0 + 2 : h + y1 - 2, x0 + 2 : x1 - 2]
    rows = np.flatnonzero(ink(img, C["tab_bar_background"]).any(axis=1)) + 2
    # two text lines, split where the rows of ink have a gap
    gap = int(np.argmax(np.diff(rows))) + 1
    second = rows[gap:]
    app.click(x0 + (x1 - x0) // 2, h + y0 + int(second.mean()))
    app.wait_title("2 tmp")
