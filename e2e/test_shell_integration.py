"""shell integration: bash gets kuterm's marks, prompts can be jumped to, output copied,
new tabs open in the folder of the shell, programs can send notifications"""

from harness import App, Notifications, as_rgb, dominant, get_clipboard, ink_bbox, mask_bbox, near, unique_theme

FEATURE = "shell integration"

THEME = unique_theme(0x5e)
C = as_rgb(THEME)

# plain bash without arguments, so kuterm injects its script
BASH = {"program": "/bin/bash"}


def prompt_shown(app: App, row: int) -> bool:
    """the injected script sets bash's own prompt, wider than the cursor alone"""
    x0, y0, _, y1 = app.cell_rect(0, row)
    band = app.shot()[y0:y1, x0:]
    box = ink_bbox(band, dominant(band))
    return box is not None and box[2] > app.cell_rect(3, row)[0] - x0


def integrated(app_factory, **settings) -> App:
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["number", {"text": " "}, "folder"],
                       "profiles": [{"name": "bash", "command": BASH, "env": {"PS1": "$ "}}],
                       **settings}, themes={"dark": THEME})
    # bash prints its prompt once the injected script ran
    app.wait(lambda: prompt_shown(app, 0), msg="prompt")
    return app


def test_copy_last_output_and_jump_to_prompts(app_factory):
    """the output of the last command is copied whole, ctrl-shift-up scrolls to older prompts"""
    app = integrated(app_factory)
    app.run("for i in $(seq 1 60); do echo filler_$i; done")
    app.run("printf 'first line\\nsecond line\\n'")
    # the output, then the next prompt
    rows = app.layout_pty()[0]
    app.wait_line(rows - 2, 11)
    app.wait(lambda: prompt_shown(app, rows - 1), msg="prompt after the output")
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


def test_click_moves_the_cursor_in_the_command_line(app_factory):
    """a click on the typed command line moves the shell's cursor there"""
    app = integrated(app_factory)
    # the system bashrc sets its own prompt, a short one puts the h of "echo hello" at column 7
    app.run("PS1='$ '; clear")

    def ink_end(row: int) -> int:
        x0, y0, _, y1 = app.cell_rect(0, row)
        band = app.shot()[y0:y1, x0:]
        box = ink_bbox(band, dominant(band))
        return 0 if box is None else x0 + box[2]

    app.wait(lambda: 0 < ink_end(0) < app.cell_rect(4, 0)[0] and ink_end(1) == 0, msg="short prompt")
    app.type("echo hello world>$T2_TMP/out")
    # the cursor after the last char
    app.wait(lambda: ink_end(0) > app.cell_rect(30, 0)[0], msg="command line typed")
    x0, y0, x1, y1 = app.cell_rect(7, 0)
    app.click((x0 + x1) // 2, (y0 + y1) // 2)
    app.type("X")
    app.snap("X typed where the command line was clicked")
    app.key("Return")
    app.wait_file("out", "Xhello world")


def test_desktop_notification_buttons_run_the_command_again(app_factory):
    """the desktop notification of a failed command offers show, run again and open first error,
    "run again" types the command into its tab again"""
    server = Notifications()
    try:
        app = integrated(app_factory, shell_integration={"notify_after": 1}, notifications={"actions": True},
                         hints={"patterns": [{"name": "rust", "regex": "[\\w/]+\\.rs", "action": "copy"}]})
        app.run("echo run >> $T2_TMP/runs; sleep 4; echo src/main.rs:12: oops; false")
        # another window takes the focus, so the end is told on the desktop
        app_factory({"theme": {"mode": "dark"}})
        shown = server.wait(lambda n: "failed" in n["body"], msg="desktop notification of the failed command")
        assert shown["actions"][::2] == ["default", "run-again", "open-error"], shown
        server.invoke(shown["id"], "run-again")
        app.wait_file("runs", "run\nrun", timeout=15)
    finally:
        server.stop()
