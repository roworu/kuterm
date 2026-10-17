"""shell_integration.click_moves_cursor: a plain click on the command line being typed moves
the shell's cursor to the clicked cell, so what you type next lands where you clicked. the
pixels show the shell cursor moving, the block the shell reports proves where the text went.
clicks that are not on the line (the prompt, past its end, while a command runs) leave it alone,
and the setting turns the whole thing off"""

import time

import numpy as np

from harness import App, as_rgb, near, unique_theme
from kit import blocks, integrated, ready, send, text, wait_block

FEATURE = "click moves the shell cursor"

THEME = unique_theme(0x7a)
C = as_rgb(THEME)

LINE = "echo abcXYZ"  # typed at the prompt, its characters sit after the prompt

# a solid block that never blinks and looks the same unfocused, so the cell it sits on is a
# rectangle of the cursor color a screenshot can read
CURSOR = {"cursor_shape": "block", "cursor_blink": "off", "cursor": {"unfocused": "same"}}


def cursor_at(app: App) -> tuple:
    """(row, col) of the block cursor in the grid, (-1, -1) when it is not on screen"""
    img = app.shot()
    ys, xs = np.nonzero(near(img[app.top() :], C["cursor"], 3))
    if not len(ys):
        return -1, -1
    # the middle of the block, back in row and column numbers
    x0, y0 = app.cell_rect(0, 0, top=app.top())[:2]
    row = int(np.mean(ys) + app.top() - y0) // int(app.line_height())
    col = int(np.mean(xs) - x0) // int(app.cell_width())
    return row, col


def click_cell(app: App, col: int, row: int) -> None:
    """click the middle of a cell of the grid"""
    x0, y0, x1, y1 = app.cell_rect(col, row, top=app.top())
    app.click((x0 + x1) // 2, (y0 + y1) // 2)


def steady_cursor(app: App) -> tuple | None:
    """the cursor of a window that is not mid paint: a screenshot can catch the app one frame
    behind its own text, and two shots a moment apart that agree are one that did not"""
    first = cursor_at(app)
    time.sleep(0.15)  # pacing, so the two shots are different frames
    second = cursor_at(app)
    return second if first == second else None


def typing_app(app_factory, **extra) -> tuple:
    """an integrated bash with LINE typed at the prompt but not run yet, and the row and the
    cell of the line's first character (the prompt is as long as the shell makes it)"""
    app = app_factory(integrated(terminal=CURSOR, **extra), themes={"dark": THEME})
    ready(app)
    app.mouse(450, 300)
    row, col = app.wait(lambda: (at := steady_cursor(app)) and at[1] > 0 and at, timeout=20,
                        msg="the cursor at the prompt")
    app.type(LINE)
    try:
        # the shell echoes what it got, so the line the clicks are aimed at is the line it holds
        app.wait(lambda: text(app).split("\n")[row].rstrip().endswith(LINE), timeout=20, msg="the typed line")
        app.wait(lambda: steady_cursor(app) == (row, col + len(LINE)), timeout=20,
                 msg="the typed line drawn with the cursor after it")
    except AssertionError as error:
        raise AssertionError(f"{error}\nrow {row} reads {text(app).split(chr(10))[row]!r}, "
                             f"cursor at {cursor_at(app)}") from None
    return app, row, col


def test_a_click_in_the_command_line_moves_the_cursor(app_factory):
    """clicking a character of the line being typed puts the shell cursor on it, so the next
    characters are inserted there and the command that runs is the edited one"""
    app, row, start = typing_app(app_factory)
    click_cell(app, start + 7, row)  # the c of abc
    app.wait(lambda: steady_cursor(app) == (row, start + 7), timeout=20, msg="the cursor on the clicked cell")
    app.snap("the shell cursor moved to the clicked cell of the line")
    app.type("def")
    app.key("Return")
    assert wait_block(app, "echo")["command"] == "echo abdefcXYZ"


def test_a_click_past_the_line_goes_to_its_end(app_factory):
    """clicking to the right of what is typed is not a move, the cursor stays after the text"""
    app, row, start = typing_app(app_factory)
    click_cell(app, start + 40, row)
    app.wait_still(msg="the click was handled")
    assert steady_cursor(app) == (row, start + len(LINE))
    app.type("def")
    app.key("Return")
    assert wait_block(app, "echo")["command"] == "echo abcXYZdef"


def test_a_click_on_the_prompt_leaves_the_cursor_alone(app_factory):
    """the prompt is not part of the command line, clicking it does not move the cursor"""
    app, row, start = typing_app(app_factory)
    click_cell(app, 0, row)
    app.wait_still(msg="the click was handled")
    assert steady_cursor(app) == (row, start + len(LINE))
    app.type("def")
    app.key("Return")
    assert wait_block(app, "echo")["command"] == "echo abcXYZdef"


def test_the_setting_turns_click_moving_off(app_factory):
    """with shell_integration.click_moves_cursor off a click on the line moves nothing"""
    app, row, start = typing_app(app_factory, shell_integration={"click_moves_cursor": False})
    click_cell(app, start + 7, row)
    app.wait_still(msg="the click was handled")
    assert steady_cursor(app) == (row, start + len(LINE))
    app.snap("a click that moved nothing, the setting is off")
    app.type("def")
    app.key("Return")
    assert wait_block(app, "echo")["command"] == "echo abcXYZdef"


def test_a_modified_click_does_not_move_the_cursor(app_factory):
    """a click with a modifier kept down is not a plain click, the cursor does not move"""
    app, row, start = typing_app(app_factory)
    with app.hold("alt"):
        click_cell(app, start + 7, row)
    app.wait_still(msg="the click was handled")
    assert steady_cursor(app) == (row, start + len(LINE))
    app.type("def")
    app.key("Return")
    assert wait_block(app, "echo")["command"] == "echo abcXYZdef"


def test_a_click_while_a_command_runs_does_not_move_the_cursor(app_factory):
    """nothing is being typed while a command runs, so a click on its line moves nothing and
    types nothing into the shell waiting for it"""
    app = app_factory(integrated(terminal=CURSOR), themes={"dark": THEME})
    ready(app)
    app.mouse(450, 300)
    row, start = app.wait(lambda: (at := steady_cursor(app)) and at[1] > 0 and at, timeout=20,
                          msg="the cursor at the prompt")
    send(app, "sleep 10")
    app.wait(lambda: (at := steady_cursor(app)) and at[0] == row + 1, timeout=20,
             msg="the shell busy on the next row")
    app.wait_still(msg="the busy shell to settle")
    busy = steady_cursor(app)
    click_cell(app, start + 2, row)  # on the line of the running command
    app.wait_still(msg="the click was handled")
    assert steady_cursor(app) == busy
    app.type("echo after_click")
    app.key("Return")
    assert wait_block(app, "echo after_click")["command"] == "echo after_click"


def test_a_click_on_a_wrapped_line_moves_across_rows(app_factory):
    """a command line longer than the window wraps, a click on its first row moves the cursor
    back up there and the typed characters land on that row"""
    app, row, start = typing_app(app_factory)
    cols = app.layout_pty()[1]
    app.type("x" * cols)
    app.wait(lambda: (at := steady_cursor(app)) and at[0] == row + 1, timeout=20,
             msg="the line wrapped to the second row")
    click_cell(app, start + 1, row)
    app.wait(lambda: steady_cursor(app) == (row, start + 1), timeout=20, msg="the cursor back on the first row")
    app.snap("a wrapped command line, the cursor moved up to the first row")
    app.type("NOW")
    app.key("Return")
    # the edited line does not start with "echo" any more, so ask for the newest block itself
    command = app.wait(lambda: (b := blocks(app)) and b[-1]["seconds"] is not None and b[-1]["command"],
                       msg="the edited wrapped command to finish")
    assert command == "eNOW" + LINE[1:] + "x" * cols
