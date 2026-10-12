"""shell integration: bash gets kuterm's marks, prompts can be jumped to, output copied,
new tabs open in the folder of the shell, programs can send notifications"""

import time

from harness import App, as_rgb, get_clipboard, mask_bbox, near, unique_theme

FEATURE = "shell integration"

THEME = unique_theme(0x5e)
C = as_rgb(THEME)

# plain bash without arguments, so kuterm injects its script
BASH = {"program": "/bin/bash"}


def integrated(app_factory, **settings) -> App:
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["number", {"text": " "}, "folder"],
                       "profiles": [{"name": "bash", "command": BASH, "env": {"PS1": "$ "}}],
                       **settings}, themes={"dark": THEME})
    time.sleep(1)
    return app


def test_copy_last_output_and_jump_to_prompts(app_factory):
    """the output of the last command is copied whole, ctrl-shift-up scrolls to older prompts"""
    app = integrated(app_factory)
    app.run("for i in $(seq 1 60); do echo filler_$i; done")
    app.run("printf 'first line\\nsecond line\\n'")
    time.sleep(0.5)
    app.palette("copy last output")
    app.wait(lambda: get_clipboard() == "first line\nsecond line", msg=f"clipboard, got {get_clipboard()!r}")
    before = app.shot()
    app.key("ctrl+shift+up")
    app.key("ctrl+shift+up")
    app.wait(lambda: (app.shot() != before).any(), msg="view scrolled to an older prompt")
    app.snap("two prompts up, the filler command is at the top")


def test_new_tab_opens_in_the_reported_folder(app_factory):
    """a new tab starts in the folder the shell of the active tab reported"""
    app = integrated(app_factory)
    app.run("cd /usr/share")
    app.wait_title("1 share")
    app.key("ctrl+shift+t")
    app.wait_title("2 share")


def notes(app: App):
    mask = near(app.shot()[300:, 400:], C["tab_bar_background"])
    return None if mask.sum() < 300 else mask_bbox(mask)


def test_program_notification(app_factory):
    """osc 9 from a program shows a notification when from_programs is "always\""""
    app = integrated(app_factory, notifications={"from_programs": "always", "timeout": 0})
    app.run("printf '\\033]9;build finished\\007'")
    app.wait(lambda: notes(app), msg="notification")
    app.snap("notification sent by a program")
