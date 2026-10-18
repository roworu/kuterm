"""commands that ask for values before they run: a field to type into, a list to pick from, the
lines a command prints, or one of the built in values ({selection}, {last_output}, {cwd},
{clipboard}, {hint}); every value fills {name} in the steps, quoted unless asked raw; a step can name
the pane, split or window it works on; and such a command can be run from the palette, a keymap
entry or kuterm msg run-command"""

import time

import numpy as np
import pytest

from harness import (App, as_rgb, bundled_theme, close_to, dominant, mask_bbox, median, near,
                     set_clipboard, unique_theme)
from kit import (integrated, msg, nothing_for, notification_box, ready, send, tabs, text,
                 wait_block, wait_text)

FEATURE = "command inputs"

THEME = unique_theme(0x77)
C = as_rgb(THEME)
DARK = bundled_theme(True)
LIGHT = bundled_theme(False)

# what a "pane:" step target is written as, by how the tab is named
SPECS = {"name": "work", "title": "titled", "number": "2"}


def app_with(app_factory, commands: list, **extra) -> App:
    """app with bash integration, a numbered title and these palette commands"""
    settings = integrated(window_title=["number", {"text": " "}, "icon", "queue"],
                          icons={"default": "D"}, **extra)
    app = app_factory(settings, themes={"dark": THEME}, commands={"commands": commands})
    ready(app)
    app.mouse(450, 560)
    return app


def touch(name: str = "ask one", **over) -> dict:
    """command asking for a value, touching a file named after it"""
    command = {"name": name, "inputs": {"v": {"text": "value"}},
               "actions": [{"type": "touch $T2_TMP/{v}\n"}]}
    command.update(over)
    return command


def panel(app: App) -> tuple | None:
    """bbox of the palette, the value list or the field over the pane, None when none is up"""
    img = app.shot()
    w = img.shape[1]
    # the panel is centered, the tab bar and the notification corner never reach these columns
    band = img[app.top() : 3 * img.shape[0] // 4, w * 2 // 5 : w * 3 // 5]
    mask = near(band, C["tab_bar_background"], 2)
    return mask_bbox(mask) if mask.sum() > 500 else None


def ask(app: App, name: str) -> tuple:
    """pick the command in the palette and wait for its input to ask for a value"""
    app.palette(name)
    return app.wait(lambda: panel(app), msg=f"the input of {name!r}")


def run(app: App, name: str, value: str) -> None:
    """pick the command in the palette, type the value into its field, press enter"""
    ask(app, name)
    app.type(value)
    app.key("Return")


def focused_half(app: App) -> str:
    """half of the pane area that is not dimmed: the split's new pane is the focused one"""
    img = app.shot()
    top = app.bar_height()
    y0, y1, x0, x1 = top + 60, img.shape[0] - 80, 30, img.shape[1] - 30
    w, h = (x0 + x1) // 2, (y0 + y1) // 2
    halves = {"left": img[y0:y1, x0:w], "right": img[y0:y1, w:x1],
              "top": img[y0:h, x0:x1], "bottom": img[h:y1, x0:x1]}
    lit = [name for name, part in halves.items() if close_to(median(part), C["terminal_background"], 3)]
    assert lit, f"no half shows the pane background: {[(n, median(p)) for n, p in halves.items()]}"
    return lit[0]


def tab_named(app: App, name: str) -> dict:
    return next(tab for tab in tabs(app) if tab["name"] == name)


def test_a_text_input_is_asked_before_the_command_runs(app_factory):
    """{"text": ...} opens a field first, only enter with a value runs the action"""
    app = app_with(app_factory, [touch("make it")])
    ask(app, "make it")
    app.snap("the command asks for its value in a field before anything runs")
    nothing_for(1.5, lambda: app.file("widget").exists())
    app.type("widget")
    app.key("Return")
    app.wait(lambda: app.file("widget").exists(), msg="the command ran with the typed value")
    app.wait(lambda: panel(app) is None, msg="the field closed after enter")


def test_escape_in_the_field_cancels_the_command(app_factory):
    """escape closes the field, nothing runs, and the command can be asked for again"""
    app = app_with(app_factory, [touch("make it")])
    ask(app, "make it")
    app.key("Escape")
    app.wait(lambda: panel(app) is None, msg="the field closed")
    nothing_for(2, lambda: app.file("widget").exists())
    run(app, "make it", "widget")
    app.wait(lambda: app.file("widget").exists(), msg="the command ran on the second try")
    assert app.alive()


def test_an_empty_answer_fills_an_empty_value(app_factory):
    """enter on an empty field fills the value in as '' and runs the action with it"""
    app = app_with(app_factory, [{"name": "empty value", "inputs": {"v": {"text": "value"}},
                                  "actions": [{"type": "printf '<%s>' {v} > $T2_TMP/empty\n"}]}])
    ask(app, "empty value")
    app.key("Return")
    app.wait_file("empty", "<>")
    # the quoting keeps the empty value one argument
    app.wait(lambda: "printf '<%s>' ''" in text(app), msg="the empty value quoted on screen")
    app.snap("an empty answer was filled in as ''")


def test_a_pick_input_lists_its_values_and_escape_cancels(app_factory):
    """{"pick": [...]} lists its values in the palette; escape runs nothing, enter picks one"""
    app = app_with(app_factory, [{"name": "pick one", "inputs": {"v": {"pick": ["alpha", "bravo"]}},
                                  "actions": [{"type": "touch $T2_TMP/{v}\n"}]}])
    ask(app, "pick one")
    app.snap("the values of the pick input, listed to pick from")
    app.key("Escape")
    app.wait(lambda: panel(app) is None, msg="the list closed")
    nothing_for(2, lambda: app.file("alpha").exists() or app.file("bravo").exists())
    # asking again works, enter takes the selected first value
    ask(app, "pick one")
    app.key("Return")
    app.wait(lambda: app.file("alpha").exists(), msg="the first value was picked")
    assert not app.file("bravo").exists()


def test_typing_filters_the_value_list_and_the_arrows_move_in_it(app_factory):
    """typing in the value list narrows it down, the arrows move the selection, enter picks it"""
    app = app_with(app_factory, [{"name": "pick one",
                                  "inputs": {"v": {"pick": ["alpha", "bravo", "gamma"]}},
                                  "actions": [{"type": "touch $T2_TMP/{v}\n"}]}])
    ask(app, "pick one")
    app.key("Down", "Down")
    app.snap("the third value selected with the arrows")
    app.key("Return")
    app.wait(lambda: app.file("gamma").exists(), msg="the third value was picked")
    assert not app.file("alpha").exists() and not app.file("bravo").exists()
    # typing filters the list instead of moving in it
    ask(app, "pick one")
    app.type("b")
    app.key("Return")
    app.wait(lambda: app.file("bravo").exists(), msg="only bravo matches a b")


def test_pick_from_lists_the_lines_its_command_prints(app_factory):
    """{"pick_from": ...} runs the command and lists every line it printed"""
    app = app_with(app_factory, [{"name": "from lines",
                                  "inputs": {"v": {"pick_from": "printf '%s\\n' one two three"}},
                                  "actions": [{"type": "touch $T2_TMP/{v}\n"}]}])
    ask(app, "from lines")
    app.snap("the three lines the command printed, listed to pick from")
    app.type("tw")
    app.key("Return")
    app.wait(lambda: app.file("two").exists(), msg="the picked line was used")
    assert not app.file("one").exists() and not app.file("three").exists()


def test_pick_from_runs_in_the_folder_of_the_tab(app_factory):
    """the pick_from command runs in the folder of the tab, so "pwd" picks that folder"""
    app = app_with(app_factory, [{"name": "where am i", "inputs": {"v": {"pick_from": "pwd"}},
                                  "actions": [{"type": "echo {v} > $T2_TMP/where\n"}]}])
    send(app, "mkdir -p $T2_TMP/deep && cd $T2_TMP/deep")
    wait_block(app, "mkdir -p")
    ask(app, "where am i")
    app.key("Return")
    app.wait_file("where", str(app.tmp / "deep"))


@pytest.mark.xdist_group("timed")  # it measures how long the app took to answer
def test_pick_from_that_prints_nothing_runs_nothing(app_factory):
    """a pick_from command with no output says so at once instead of opening an empty list"""
    app = app_with(app_factory, [{"name": "nothing here", "inputs": {"v": {"pick_from": "true"}},
                                  "actions": [{"type": "touch $T2_TMP/ran\n"}]},
                                 touch("after it")])
    start = time.monotonic()
    app.palette("nothing here")
    app.wait(lambda: notification_box(app, C), msg="the notice about the empty list")
    # the app must say so at once, not sit out the seconds a pick_from waits for the program
    assert time.monotonic() - start < 3.0, "the empty list did not wait for the timeout"
    app.snap("nothing to pick, the app says so instead of opening an empty list")
    assert panel(app) is None
    nothing_for(2, lambda: app.file("ran").exists())
    # the command is not left pending, another one still runs
    run(app, "after it", "fine")
    app.wait(lambda: app.file("fine").exists(), msg="the app is ready for the next command")


def test_pick_from_that_takes_too_long_gives_up(app_factory):
    """a pick_from command still running when its timeout is up says so and runs nothing"""
    app = app_with(app_factory, [{"name": "slow list", "inputs": {"v": {"pick_from": "sleep 5"}},
                                  "actions": [{"type": "touch $T2_TMP/ran\n"}]}])
    app.palette("slow list")
    nothing_for(1.0, lambda: notification_box(app, C))
    app.wait(lambda: notification_box(app, C), msg="the notice about the timeout")
    app.snap("the pick_from command took too long, the app gave up on it")
    assert panel(app) is None
    nothing_for(2, lambda: app.file("ran").exists())
    assert app.alive()


def test_values_are_quoted_for_the_shell_unless_asked_raw(app_factory):
    """{name} is quoted so the shell keeps it one word, {name:raw} is filled in as typed"""
    app = app_with(app_factory, [{"name": "quoting", "inputs": {"v": {"text": "value"}},
                                  "actions": [{"type": "echo {v} > $T2_TMP/quoted\n"},
                                              {"type": "echo {v:raw} > $T2_TMP/raw\n"}]}])
    run(app, "quoting", "$HOME")
    app.wait_file("quoted", "$HOME")
    app.wait_file("raw", str(app.home))
    app.wait(lambda: "echo '$HOME'" in text(app), msg="the quoted value on screen")
    app.snap("the same value quoted in one step and raw in the other")


def test_unknown_placeholders_are_left_alone(app_factory):
    """a {name} no input and no built in knows stays in the text, so awk scripts survive"""
    app = app_with(app_factory, [{"name": "left alone",
                                  "actions": [{"type": "echo '{print $1}' {nope} > $T2_TMP/unknown\n"}]}])
    app.palette("left alone")
    app.wait_file("unknown", "{print $1} {nope}")
    # the shell can write the file before the closed palette is repainted
    app.wait(lambda: panel(app) is None, msg="palette closed")
    assert app.alive()


def test_the_selection_built_in_fills_the_selected_text(app_factory):
    """{selection} fills what is selected in the pane, without an input asking for it"""
    app = app_factory({"theme": {"mode": "dark"}},
                      themes={"dark": THEME},
                      commands={"commands": [{"name": "use selection",
                                              "actions": [{"type": "selection={selection}\n"}]}]},
                      script="printf 'alpha bravo charlie\\n'\nIFS= read -r line\n"
                             "printf '%s' \"$line\" > \"$T2_TMP/selection\"\n")
    app.wait_line(0, 19)
    # the word in the middle of the printed line
    x0, y0, x1, y1 = app.cell_rect(8, 0, top=app.top())
    app.click((x0 + x1) // 2, (y0 + y1) // 2, repeat=2)
    app.snap("a word of the output selected with a double click")
    app.palette("use selection")
    app.wait_file("selection", "selection=bravo")
    # the built in needs no field of its own: the command ran straight from the palette
    app.wait(lambda: panel(app) is None, msg="the panel of the finished command to close")


def test_the_last_output_built_in_fills_what_the_command_printed(app_factory):
    """{last_output} fills what the last finished command of the tab printed"""
    app = app_with(app_factory, [{"name": "reuse the output",
                                  "actions": [{"type": "echo {last_output} > $T2_TMP/last\n"}]}])
    send(app, "echo hello_from_last")
    wait_block(app, "echo hello_from_last")
    app.palette("reuse the output")
    app.wait_file("last", "hello_from_last")


def test_the_cwd_built_in_fills_the_folder_of_the_tab(app_factory):
    """{cwd} fills the folder the tab's shell is in"""
    app = app_with(app_factory, [{"name": "where", "actions": [{"type": "echo {cwd} > $T2_TMP/cwd\n"}]}])
    send(app, "mkdir -p $T2_TMP/deep && cd $T2_TMP/deep")
    wait_block(app, "mkdir -p")
    app.palette("where")
    app.wait_file("cwd", str(app.tmp / "deep"))


def test_the_clipboard_built_in_fills_the_clipboard(app_factory):
    """{clipboard} fills what the clipboard holds, so something has to be copied first"""
    app = app_with(app_factory, [{"name": "use the clipboard",
                                  "actions": [{"type": "echo {clipboard} > $T2_TMP/clip\n"}]}])
    set_clipboard("clip text 42")
    app.palette("use the clipboard")
    app.wait_file("clip", "clip text 42")


def test_the_hint_built_in_starts_hints_mode_and_the_picked_match_fills_it(app_factory):
    """{hint} labels the matches on screen; picking one runs the command with it, escape does not"""
    app = app_with(app_factory, [{"name": "pick the tag",
                                  "actions": [{"type": "touch $T2_TMP/hint-{hint}\n"}]}],
                   # one pattern of its own, so the tag is the only match on screen and its
                   # label is the first letter; a still cursor keeps the pixel count steady
                   hints={"patterns": [{"name": "tag", "regex": "zz[0-9a-f]{8}", "action": "copy"}]},
                   terminal={"cursor_blink": "off"})
    send(app, "echo tag zz1234abcd")
    wait_block(app, "echo tag")

    def labelled() -> int:
        """cursor coloured pixels: the blocks the hint labels are drawn on, plus the cursor"""
        return int(near(app.shot(), C["cursor"], 2).sum())

    plain = labelled()
    app.palette("pick the tag")
    app.wait(lambda: labelled() > plain + 150, msg="the hint label over the match")
    app.snap("hints mode started by a command waiting for {hint}, the match labelled")
    app.key("Escape")
    app.wait(lambda: labelled() < plain + 60, msg="the label went away")
    nothing_for(1.5, lambda: app.file("hint-zz1234abcd").exists())
    app.palette("pick the tag")
    app.wait(lambda: labelled() > plain + 150, msg="the hint label again")
    app.key("a")
    app.wait(lambda: app.file("hint-zz1234abcd").exists(), msg="the command ran with the picked match")
    app.wait(lambda: panel(app) is None, msg="the panel of the finished command to close")


def test_a_value_fills_the_step_of_another_pane(app_factory):
    """a placeholder is filled in a step that targets another tab, asked for in the current one"""
    app = app_with(app_factory, [{"name": "into the other", "inputs": {"v": {"text": "value"}},
                                  "actions": [{"target": "pane:elsewhere",
                                               "action": {"type": "touch $T2_TMP/{v}\n"}}]}])
    assert msg(app, "new-tab", "--name", "elsewhere").returncode == 0
    other = tab_named(app, "elsewhere")
    run(app, "into the other", "poured")
    app.wait(lambda: app.file("poured").exists(), msg="the value ran in the other tab")
    wait_text(app, "touch $T2_TMP/poured", pane=str(other["id"]))
    first = next(tab for tab in tabs(app) if tab["id"] != other["id"])
    assert "touch" not in text(app, str(first["id"]))


@pytest.mark.parametrize("side,expected", [("left", "left"), ("right", "right"),
                                           ("up", "top"), ("down", "bottom")])
def test_a_step_can_open_a_split_in_each_direction(app_factory, side, expected):
    """{"target": "new_split_<side>"} splits the focused pane and works in the new one"""
    app = app_with(app_factory, [{"name": "split away", "inputs": {"v": {"text": "value"}},
                                  "actions": [{"target": f"new_split_{side}",
                                               "action": {"type": "touch $T2_TMP/{v}\n"}}]}],
                   panes={"dim_inactive": 0.6})
    rows, cols = app.layout_pty()
    run(app, "split away", side)
    app.wait(lambda: app.file(side).exists(), msg=f"the step ran in the new {side} pane")
    app.wait(lambda: focused_half(app) == expected, msg=f"the new pane is the {side} one")
    app.snap(f"the pane split to the {side}, the new one focused")
    split_rows, split_cols = app.pty_size()
    # the split takes room off its own axis, the other one only loses a cell or two
    if expected in ("left", "right"):
        assert split_cols < cols and split_rows >= rows - 3, (split_cols, split_rows)
    else:
        assert split_rows < rows and split_cols >= cols - 3, (split_cols, split_rows)


@pytest.mark.parametrize("by", ["name", "title", "number"])
def test_a_step_can_name_another_pane(app_factory, by):
    """a "pane:" target finds its tab by its rename_pane name, its title or its pane number"""
    app = app_with(app_factory, [{"name": "to the named", "inputs": {"v": {"text": "value"}},
                                  "actions": [{"target": f"pane:{SPECS[by]}",
                                               "action": {"type": "touch $T2_TMP/reached\n"}}]}])
    assert msg(app, "new-tab", "--name", "work").returncode == 0
    work = tab_named(app, "work")
    # a fresh app hands out pane ids from 1, so a second tab is pane number 2
    assert str(work["id"]) == "2", work
    assert msg(app, "set-tab-title", "--pane", "work", "titled").returncode == 0
    run(app, "to the named", "reached")
    app.wait(lambda: app.file("reached").exists(), msg=f"the step ran in the pane named by its {by}")
    wait_text(app, "touch $T2_TMP/reached", pane=str(work["id"]))
    first = next(tab for tab in tabs(app) if tab["id"] != work["id"])
    assert "touch" not in text(app, str(first["id"]))


def test_a_step_naming_no_pane_says_so_and_runs_nothing(app_factory):
    """an unknown "pane:" target shows a notice, its step and the ones after it do not run"""
    app = app_with(app_factory, [{"name": "to nowhere", "inputs": {"v": {"text": "value"}},
                                  "actions": [{"target": "pane:nope", "action": {"type": "touch $T2_TMP/{v}\n"}},
                                              {"type": "touch $T2_TMP/after\n"}]}])
    ask(app, "to nowhere")
    app.type("lost")
    app.key("Return")
    app.wait(lambda: notification_box(app, C), msg="the notice about the unknown pane")
    app.snap("a step naming a pane that is not there")
    nothing_for(2, lambda: app.file("lost").exists() or app.file("after").exists())
    assert app.alive()


def test_a_step_can_work_in_a_new_window(app_factory):
    """{"target": "new_window"} opens a window of its own and runs the step in it"""
    app = app_with(app_factory, [{"name": "open there", "inputs": {"v": {"text": "value"}},
                                  "actions": [{"target": "new_window",
                                               "action": {"type": "touch $T2_TMP/{v}\n"}}]}])
    run(app, "open there", "elsewhere")
    app.wait(lambda: app.file("elsewhere").exists(), msg="the step ran in the new window's shell")
    app.wait(lambda: len({tab["window"] for tab in tabs(app)}) == 2, msg="a second window")


def test_a_keymap_entry_runs_a_command_with_its_inputs(app_factory):
    """a keymap entry with {"run": ...} asks for the inputs just like the palette does"""
    app = app_with(app_factory, [touch("bound")],
                   keymap=[{"keys": "ctrl-shift-j", "action": {"run": "bound"}}])
    app.key("ctrl+shift+j")
    app.wait(lambda: panel(app), msg="the field of the bound command")
    app.snap("a keymap entry asked for the command's value")
    app.type("keyed")
    app.key("Return")
    app.wait(lambda: app.file("keyed").exists(), msg="the key ran the command")
    app.wait(lambda: panel(app) is None, msg="the panel of the finished command to close")


def test_run_command_over_the_socket_fills_inputs_and_names_bad_ones(app_factory):
    """kuterm msg run-command fills a value, and says what is missing or unknown"""
    app = app_with(app_factory, [touch("socket one")])
    done = msg(app, "run-command", "socket one", "--input", "v=made")
    assert done.returncode == 0, done.stderr
    app.wait(lambda: app.file("made").exists(), msg="the value from the socket")
    missing = msg(app, "run-command", "socket one")
    assert missing.returncode != 0 and "--input v=<value>" in missing.stderr + missing.stdout
    unknown = msg(app, "run-command", "socket one", "--input", "v=x", "--input", "nope=x")
    assert unknown.returncode != 0 and 'has no input "nope"' in unknown.stderr + unknown.stdout
    gone = msg(app, "run-command", "no such thing", "--input", "v=x")
    assert gone.returncode != 0 and 'no command named "no such thing"' in gone.stderr + gone.stdout
    # only the one good call typed anything into the tab
    assert text(app).count("touch $T2_TMP") == 1
    app.snap("the value the socket filled in, typed into the tab")


def test_an_input_named_like_a_built_in_is_a_config_error(app_factory):
    """an input shadowing a built in value makes the whole settings file invalid, defaults run"""
    app = app_factory('{"theme": {"mode": "dark"}, "remote_control": "socket", "commands": ['
                      '{"name": "bad", "inputs": {"cwd": {"text": "x"}}, "actions": ["about"]}]}')
    app.wait(lambda: 'command "bad"' in app.output() and "built in value" in app.output(),
             msg="the config error on stderr")
    assert "invalid settings" in app.output()
    assert app.alive()
    # defaults run, so the error is shown in the bundled theme inside the window too
    theme = DARK if close_to(dominant(app.shot()), DARK["terminal_background"], 6) else LIGHT
    app.wait(lambda: near(app.shot()[300:, 400:], theme["danger_button"]).sum() > 100,
             msg="the error shown in the window")
    before = app.shot()
    app.key("ctrl+shift+p")
    app.wait(lambda: not np.array_equal(app.shot(), before), msg="the palette still opens on defaults")
    app.snap("a broken commands list: the error, and the app running defaults")


def test_a_split_step_runs_in_the_folder_of_the_tab(app_factory):
    """a pane opened by a step runs the value in the folder the command was run from, like
    pick_from runs in that folder too"""
    app = app_with(app_factory, [{"name": "split here", "inputs": {"v": {"text": "value"}},
                                  "actions": [{"target": "new_split_right",
                                               "action": {"type": "echo $PWD > $T2_TMP/where\n"}}]}])
    send(app, "mkdir -p $T2_TMP/deep && cd $T2_TMP/deep")
    wait_block(app, "mkdir -p")
    run(app, "split here", "x")
    app.wait_file("where")
    assert app.file("where").read_text().strip() == str(app.tmp / "deep")


def test_inputs_with_a_queued_command(app_factory):
    """a command with inputs whose step queues one: the value is asked for first, the queue prompt
    comes next, and the queued line runs once the prompt is back"""
    app = app_with(app_factory, [{"name": "queue a run", "inputs": {"v": {"text": "value"}},
                                  "actions": [{"notify": "{v}\n"}, "queue_command"]}])
    send(app, "sleep 6")
    wait_block(app, "sleep 6", finished=False)
    ask(app, "queue a run")
    app.type("soon")
    app.key("Return")
    app.wait(lambda: notification_box(app, C), msg="the notice with the value")
    app.wait(lambda: panel(app), msg="the queue prompt after the input")
    app.snap("the value was asked for, now the queue asks for the command line")
    app.type("touch $T2_TMP/queued")
    app.key("Return")
    app.wait(lambda: "⧗" in app.title(), msg="the entry waits in the queue")
    nothing_for(1, lambda: app.file("queued").exists())
    app.wait(lambda: app.file("queued").exists(), msg="the queued line ran at the prompt", timeout=20)
    app.wait(lambda: "⧗" not in app.title(), msg="the queue is empty again")


def test_a_pinned_command_with_inputs_comes_first_in_the_palette(app_factory):
    """a pinned command is listed before the others, so enter picks it and asks for its value"""
    app = app_with(app_factory, [
        {"name": "aaa would be first", "actions": [{"type": "touch $T2_TMP/unpinned\n"}]},
        touch("zzz pinned later", pinned=True),
    ])
    app.palette("", run=False)
    app.wait(lambda: panel(app), msg="the palette with the pinned command first")
    app.snap("the pinned command listed above the one the name would sort first")
    app.key("Return")
    app.wait(lambda: panel(app), msg="the field of the pinned command")
    app.type("chosen")
    app.key("Return")
    app.wait(lambda: app.file("chosen").exists(), msg="the pinned command ran with the value")
    assert not app.file("unpinned").exists()


def test_a_command_can_answer_a_program_waiting_for_input(app_factory):
    """a command sending text into a program waiting for an answer gives it the value, and the
    tab's waiting mark goes away with it"""
    app = app_with(app_factory, [{"name": "answer it", "inputs": {"v": {"text": "value"}},
                                  "actions": [{"send_text": "{v:raw}\n"}]}],
                   notifications={"program_waiting": True})
    send(app, "python3 -c \"open('$T2_TMP/answer','w').write(input('continue? [y/N] '))\"")
    wait_block(app, "python3 -c", finished=False)
    app.wait(lambda: near(app.shot()[: app.bar_height()], C["attention"], 3).sum() > 20,
             msg="the tab marked as waiting")
    app.snap("the program waits for an answer, its tab is marked")
    run(app, "answer it", "yes")
    app.wait_file("answer", "yes")
    app.wait(lambda: near(app.shot()[: app.bar_height()], C["attention"], 3).sum() < 20,
             msg="the mark gone once the program got its answer")
