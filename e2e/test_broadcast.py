"""broadcast input and synced scroll: keys typed into a pane joined to the broadcast run in every
other joined pane of the window, joined panes wear a red frame and a "broadcast" label; panes
joined to the scroll sync wheel together by the same lines, each stopping at its own history and
marked "synced"; the source gutter says which pane had the command typed by hand"""

import time

from harness import App, as_rgb, count, set_clipboard, unique_theme
from kit import (blocks, integrated, listening, msg, nothing_for, ready, send, tabs, text,
                 wait_block, wait_text)

FEATURE = "broadcast input"

THEME = unique_theme(0x9d)
C = as_rgb(THEME)

KEYS = {"split_right": "ctrl-shift-r", "split_down": "ctrl-shift-o", "close_pane": "ctrl-shift-x",
        "zoom_pane": "ctrl-shift-z", "toggle_broadcast_all": "ctrl-shift-b",
        "toggle_broadcast_pane": "ctrl-shift-g", "toggle_scroll_sync": "ctrl-shift-y"}
# keymap actions that scroll the focused tab, the sync takes their lines along too
SCROLL_KEYS = [{"keys": "ctrl-shift-u", "action": {"scroll_up": 10}},
               {"keys": "ctrl-shift-n", "action": {"scroll_down": 10}}]

QUESTION = "python3 -c \"x = input('continue? [y/N] '); open('$T2_TMP/answer', 'w').write(x)\""


def ask_password(name: str) -> str:
    """a program asking for a secret, writing what it read into its own file"""
    return ("python3 -c \"import getpass; p = getpass.getpass('Password: '); "
            f"open('$T2_TMP/{name}', 'w').write(p)\"")


# ---- apps and oracles -------------------------------------------------------


def two_panes(app_factory, **extra) -> App:
    """an app with two panes side by side, each with its own shell at a prompt, nothing dimmed
    so every pane paints the same colors"""
    extra.setdefault("window_title", ["number"])
    app = app_factory(integrated(hide_bar_for_one_tab=False, panes={"dim_inactive": 0}, **extra),
                      themes={"dark": THEME}, keybindings=KEYS)
    listening(app)
    ready(app)
    assert msg(app, "split", "right").returncode == 0
    app.wait(lambda: len(panes(app)) == 2, msg="the second pane")
    ready(app, pane_tab(app, panes(app)[1]))
    return app


def typed_panes(app_factory) -> App:
    """two panes side by side whose shells have only ever run commands typed by hand, so no
    source mark is in either gutter yet"""
    app = app_factory(integrated(hide_bar_for_one_tab=False, panes={"dim_inactive": 0},
                                 window_title=["number"]),
                      themes={"dark": THEME}, keybindings=KEYS)
    listening(app)
    app.run("true")
    wait_block(app, "true")
    assert msg(app, "split", "right").returncode == 0
    app.wait(lambda: len(panes(app)) == 2, msg="the second pane")
    right = pane_tab(app, panes(app)[1])
    click_pane(app, "right")
    app.run("true")
    wait_block(app, "true", right)
    return app


def three_panes(app_factory, **extra) -> App:
    """two panes side by side and a third one under the right one"""
    app = two_panes(app_factory, **extra)
    assert msg(app, "split", "down").returncode == 0
    app.wait(lambda: len(panes(app)) == 3, msg="the third pane")
    ready(app, pane_tab(app, panes(app)[-1]))
    return app


def panes(app: App) -> list[int]:
    """pane ids in layout order, left and top first"""
    return sorted({t["pane"] for t in tabs(app)})


def pane_tab(app: App, pane: int) -> str:
    """the tab id of a pane"""
    return str(next(t["id"] for t in tabs(app) if t["pane"] == pane))


def pane_tabs(app: App) -> list[str]:
    """the tab of every pane, in pane order, for the one tab per pane helpers"""
    return [pane_tab(app, pane) for pane in panes(app)]


def tab_ids(app: App, pane: int) -> list[str]:
    return [str(t["id"]) for t in sorted(tabs(app), key=lambda t: t["number"]) if t["pane"] == pane]


def click_pane(app: App, side: str) -> str:
    """click into one of the panes these helpers lay out and give back its tab id"""
    w, h = app.size()
    x, y = {"left": (w // 4, h // 2), "right": (w * 3 // 4, h // 3), "lower": (w * 3 // 4, h * 3 // 4)}[side]
    app.click(x, y)
    time.sleep(0.2)
    return pane_tab(app, panes(app)[{"left": 0, "right": 1, "lower": -1}[side]])


def region(app: App, side: str) -> tuple:
    """(x0, x1, y) of the top of a pane's terminal, where its broadcast frame runs"""
    w, h = app.size()
    bar = app.bar_height()
    return {"left": (4, w // 2 - 4, bar),
            "right": (w // 2 + 4, w - 4, bar),
            "lower": (w // 2 + 4, w - 4, h // 2 + bar)}[side]


def frame_px(app: App, side: str) -> int:
    """red pixels along the top edge of a pane: its broadcast frame"""
    x0, x1, y = region(app, side)
    return count(app.shot()[y : y + 9, x0:x1], C["danger_button"], 3)


def label_px(app: App, side: str) -> int:
    """red pixels in the top right corner of a pane below its frame: its "broadcast" label"""
    x0, x1, y = region(app, side)
    return count(app.shot()[y + 5 : y + 30, x1 - 160 : x1 - 3], C["danger_button"], 3)


def synced_px(app: App, side: str) -> int:
    """tab active colored pixels in the bottom right corner of a pane: its "synced" label"""
    w, h = app.size()
    edge = w // 2 if side == "left" else w
    return count(app.shot()[h - 26 : h - 2, edge - 160 : edge - 3], C["tab_active_background"], 3)


def framed(app: App, *sides: str) -> bool:
    return all(frame_px(app, side) > 200 for side in sides)


def rows(app: App, pane: str, line: str) -> None:
    """fill a pane with history: clear the screen and print a line per number"""
    send(app, f"clear; seq 1 {line}", pane)


def pane_lines(app: App, side: str) -> bytes:
    """pixels of the first lines of a pane's grid, the lines it is scrolled to right now (the
    text query always tells the bottom of the screen, so scrolls show in pixels)"""
    w, _ = app.size()
    x0, y0, x1, y1 = app.cell_rect(0, 0, cols=4, rows=8, top=app.bar_height())
    dx = 0 if side == "left" else w // 2
    return app.shot()[y0:y1, x0 + dx : x1 + dx].tobytes()


def pty_columns(app: App, pane: str, name: str) -> int:
    """columns the shell of a pane sees, two fewer while a source gutter is drawn in it"""
    app.focus()
    line = f"stty size > $T2_TMP/{name}.part && mv $T2_TMP/{name}.part $T2_TMP/{name}"
    app.run(line)
    return int(app.wait_file(name).split()[1])


# ---- broadcast input --------------------------------------------------------


def test_broadcast_all_types_into_every_pane_and_marks_them(app_factory):
    """toggle_broadcast_all: one key starts typing into every pane of the window at once, both
    panes wear the red frame and the label so nobody types into the wrong machine by accident"""
    app = two_panes(app_factory)
    left, right = pane_tabs(app)
    assert frame_px(app, "left") == frame_px(app, "right") == 0
    click_pane(app, "left")
    app.key("ctrl+shift+b")
    app.wait(lambda: framed(app, "left", "right"), msg="both panes framed red")
    assert label_px(app, "left") > 150 and label_px(app, "right") > 150, "broadcast label"
    app.snap("both panes joined to the broadcast, a red frame and a label each")
    app.run("echo everywhere")
    app.wait(lambda: "everywhere" in text(app, left) and "everywhere" in text(app, right),
             msg="the typed command ran in both panes", timeout=15)


def test_toggling_again_stops_the_broadcast(app_factory):
    """the same key switches it off: the frame goes and typing lands in the focused pane only"""
    app = two_panes(app_factory)
    left, right = pane_tabs(app)
    click_pane(app, "left")
    app.key("ctrl+shift+b")
    app.wait(lambda: framed(app, "left", "right"), msg="joined")
    app.run("echo both_ways")
    app.wait(lambda: "both_ways" in text(app, right), msg="the second pane ran it")
    app.key("ctrl+shift+b")
    app.wait(lambda: not framed(app, "left", "right"), msg="frames gone")
    app.snap("broadcast off again, no frame left on either pane")
    app.run("echo just_here")
    app.wait(lambda: "just_here" in text(app, left), msg="the focused pane ran it")
    nothing_for(2, lambda: "just_here" in text(app, right))


def test_only_the_active_tab_of_a_pane_takes_the_typing(app_factory):
    """"every pane of the window" is their active tabs: a second tab of a joined pane is written
    into only once it becomes that pane's active one"""
    app = two_panes(app_factory)
    first_pane = panes(app)[0]
    first = pane_tab(app, first_pane)
    right = pane_tab(app, panes(app)[1])
    click_pane(app, "left")
    app.key("ctrl+shift+t")
    app.wait(lambda: len(tab_ids(app, first_pane)) == 2, msg="a second tab in the left pane")
    second = next(str(t["id"]) for t in tabs(app) if t["pane"] == first_pane and t["active"])
    app.key("ctrl+shift+b")
    app.wait(lambda: framed(app, "left", "right"), msg="joined")
    app.snap("a second tab in the left pane while the broadcast is on")
    app.run("echo active_tabs_only")
    wait_block(app, "echo active_tabs_only", second)
    wait_block(app, "echo active_tabs_only", right)
    # the tab that is not the pane's active one was never written into
    nothing_for(1.5, lambda: any("active_tabs_only" in b["command"] for b in blocks(app, first)))


def test_another_window_is_never_broadcast_into(app_factory):
    """a broadcast is one window's panes: a second window of the app keeps its own typing"""
    app = two_panes(app_factory)
    left, right = pane_tabs(app)
    window = next(t["window"] for t in tabs(app) if str(t["id"]) == left)
    click_pane(app, "left")
    app.key("ctrl+shift+b")
    app.wait(lambda: framed(app, "left", "right"), msg="joined")
    assert msg(app, "new-window").returncode == 0
    other = app.wait(lambda: next((str(t["id"]) for t in tabs(app) if t["window"] != window), None),
                     msg="the second window")
    app.run("echo first_window_only")
    app.wait(lambda: "first_window_only" in text(app, right), msg="the joined pane ran it", timeout=15)
    nothing_for(2, lambda: "first_window_only" in text(app, other))
    app.snap("the panes of the first window joined, the second window untouched")


def test_broadcast_pane_joins_only_the_panes_picked(app_factory):
    """toggle_broadcast_pane: only the panes that joined take the typing, the third one keeps
    its own line"""
    app = three_panes(app_factory)
    left, right, lower = pane_tabs(app)
    click_pane(app, "left")
    app.key("ctrl+shift+g")
    click_pane(app, "right")
    app.key("ctrl+shift+g")
    app.wait(lambda: framed(app, "left", "right"), msg="the two joined panes")
    assert frame_px(app, "lower") == 0, "the pane left out is not marked"
    app.snap("two panes joined, the lower one left out")
    click_pane(app, "left")
    app.run("echo joined_twice")
    app.wait(lambda: "joined_twice" in text(app, right) and "joined_twice" in text(app, left),
             msg="both joined panes ran it", timeout=15)
    nothing_for(2, lambda: "joined_twice" in text(app, lower))


def test_joining_a_pane_twice_takes_it_out(app_factory):
    """the toggle works as a switch: the pane that joined again stops taking the typing, and
    once one pane is left no frame is drawn anywhere"""
    app = two_panes(app_factory)
    left, right = pane_tabs(app)
    click_pane(app, "left")
    app.key("ctrl+shift+g")
    click_pane(app, "right")
    app.key("ctrl+shift+g")
    app.wait(lambda: framed(app, "left", "right"), msg="both joined")
    app.key("ctrl+shift+g")
    app.wait(lambda: not framed(app, "left", "right"), msg="the last frame gone with the last join")
    app.snap("the pane joined again left the broadcast, nothing is marked")
    click_pane(app, "left")
    app.run("echo alone_again")
    app.wait(lambda: "alone_again" in text(app, left), msg="the focused pane ran it")
    nothing_for(2, lambda: "alone_again" in text(app, right))


def test_typing_in_an_unjoined_pane_stays_there(app_factory):
    """a pane that is not joined is no source either: its keystrokes reach the joined panes only
    when it joins them"""
    app = three_panes(app_factory)
    left, right, lower = pane_tabs(app)
    click_pane(app, "right")
    app.key("ctrl+shift+g")
    click_pane(app, "lower")
    app.key("ctrl+shift+g")
    app.wait(lambda: framed(app, "right", "lower"), msg="the two joined panes")
    assert frame_px(app, "left") == 0
    click_pane(app, "left")
    app.run("echo from_the_side")
    app.wait(lambda: "from_the_side" in text(app, left), msg="the unjoined pane ran it")
    nothing_for(2, lambda: "from_the_side" in text(app, right) or "from_the_side" in text(app, lower))


def test_a_split_made_later_is_not_joined(app_factory):
    """a pane opened while the broadcast runs is not in it until it joins, so the frame always
    says which panes a keystroke reaches - even the fresh tab the split opens"""
    app = two_panes(app_factory)
    click_pane(app, "right")
    app.key("ctrl+shift+b")
    app.wait(lambda: framed(app, "left", "right"), msg="joined")
    assert msg(app, "split", "down").returncode == 0
    app.wait(lambda: len(panes(app)) == 3, msg="the new pane")
    lower = pane_tab(app, panes(app)[-1])
    ready(app, lower)
    assert frame_px(app, "lower") == 0, "the new pane is not marked"
    assert framed(app, "left", "right")
    app.snap("a pane split while the broadcast runs, the two old panes still joined")
    click_pane(app, "left")
    app.run("echo two_old_panes")
    app.wait(lambda: "two_old_panes" in text(app, pane_tab(app, panes(app)[1])),
             msg="the pane that was joined took it", timeout=15)
    nothing_for(2, lambda: "two_old_panes" in text(app, lower))


def test_closing_a_joined_pane_leaves_no_frame_behind(app_factory):
    """the frame promises that a keystroke reaches another pane: with the other pane closed there
    is nothing left to broadcast to, so the last pane loses the frame too"""
    app = two_panes(app_factory)
    click_pane(app, "left")
    app.key("ctrl+shift+b")
    app.wait(lambda: framed(app, "left", "right"), msg="joined")
    click_pane(app, "right")
    app.key("ctrl+shift+x")
    app.wait(lambda: len(panes(app)) == 1, msg="the pane closed")
    app.snap("the other pane closed while the broadcast was on")
    assert frame_px(app, "left") == 0, "the pane left alone still wears the broadcast frame"
    # joining all again notices there is only one pane left and drops it
    app.key("ctrl+shift+b")
    app.key("ctrl+shift+b")
    app.wait(lambda: frame_px(app, "left") == 0, msg="the frame gone after toggling twice")


def test_a_pane_closing_does_not_stop_the_typing(app_factory):
    """the keyboard keeps working in the pane that is left, whatever the broadcast holds"""
    app = two_panes(app_factory)
    left, right = pane_tabs(app)
    click_pane(app, "left")
    app.key("ctrl+shift+b")
    app.wait(lambda: framed(app, "left", "right"), msg="joined")
    click_pane(app, "right")
    app.key("ctrl+shift+x")
    app.wait(lambda: len(panes(app)) == 1, msg="the pane closed")
    app.run("echo still_typed")
    app.wait(lambda: "still_typed" in text(app, left), msg="the last pane still takes the keyboard")


def test_zooming_a_pane_keeps_broadcasting_to_the_hidden_ones(app_factory):
    """zooming hides the other panes but does not unjoin them: the frame stays and the typing
    still reaches every joined pane"""
    app = two_panes(app_factory)
    left, right = pane_tabs(app)
    click_pane(app, "left")
    app.key("ctrl+shift+b")
    app.wait(lambda: framed(app, "left", "right"), msg="joined")
    app.key("ctrl+shift+z")
    time.sleep(0.5)
    app.run("echo hidden_but_joined")
    app.wait(lambda: "hidden_but_joined" in text(app, right), msg="the hidden pane ran it", timeout=15)
    app.snap("zoomed into one pane while the broadcast still reaches the hidden one")
    app.key("ctrl+shift+z")
    app.wait(lambda: framed(app, "left", "right"), msg="both panes shown again, still joined")


def test_a_config_reload_keeps_the_broadcast_on(app_factory):
    """reloading the config re-reads the files: the panes joined stay joined and keep taking the
    typing"""
    app = two_panes(app_factory)
    left, right = pane_tabs(app)
    click_pane(app, "left")
    app.key("ctrl+shift+b")
    app.wait(lambda: framed(app, "left", "right"), msg="joined")
    app.write("settings.jsonc", {**app.settings, "window_title": [{"text": "reloaded"}]})
    app.palette("config: reload config")
    app.wait_title("reloaded")
    assert framed(app, "left", "right"), "still joined after the reload"
    app.snap("config reloaded with the broadcast still on")
    app.run("echo after_reload")
    app.wait(lambda: "after_reload" in text(app, right), msg="the joined pane still runs it", timeout=15)


def test_broadcast_typing_reaches_a_program_that_is_not_a_shell(app_factory):
    """cat in the other pane echoes what the broadcast types: the keys go to the program, not
    to a shell"""
    app = two_panes(app_factory)
    left, right = pane_tabs(app)
    send(app, "cat", right)
    wait_block(app, "cat", right, finished=False)
    click_pane(app, "left")
    app.key("ctrl+shift+b")
    app.wait(lambda: framed(app, "left", "right"), msg="joined")
    app.type("hello_from_the_other_pane")
    wait_text(app, "hello_from_the_other_pane", right)
    assert "hello_from_the_other_pane" in text(app, left), "the shell of the typed pane echoed it too"
    app.snap("cat in the right pane echoing what the left pane typed")
    app.key("Return")
    wait_text(app, "hello_from_the_other_pane\n", right)
    # the broadcast off, so the end of file goes to cat alone and not to the other shell
    app.key("ctrl+shift+b")
    click_pane(app, "right")
    app.key("ctrl+d")
    wait_block(app, "cat", right)


def test_broadcast_keys_reach_a_full_screen_program(app_factory):
    """less in the other pane takes the typing as its own keys: G jumps to the last line"""
    app = two_panes(app_factory)
    left, right = pane_tabs(app)
    send(app, "seq 1 400 > $T2_TMP/numbers && less $T2_TMP/numbers", right)
    wait_text(app, "numbers", right)
    time.sleep(1)
    assert "400" not in text(app, right), "less starts at the first line"
    click_pane(app, "left")
    app.key("ctrl+shift+b")
    app.wait(lambda: framed(app, "left", "right"), msg="joined")
    app.type("G")
    app.wait(lambda: "400" in text(app, right), msg="less jumped to the last line")
    app.snap("less in the right pane jumped to the end by a key typed in the left one")
    app.type("q")
    wait_block(app, "seq 1 400", right)


def test_broadcast_never_answers_a_password_prompt(app_factory):
    """a program reading a secret refuses input it cannot see typed: the pane typed into takes
    the password, the broadcast copy is dropped"""
    app = two_panes(app_factory)
    left, right = pane_tabs(app)
    send(app, ask_password("secret_left"), left)
    send(app, ask_password("secret_right"), right)
    wait_text(app, "Password:", left)
    wait_text(app, "Password:", right)
    click_pane(app, "left")
    app.key("ctrl+shift+b")
    app.wait(lambda: framed(app, "left", "right"), msg="joined")
    app.snap("both panes asking for a password, the broadcast on")
    app.type("kb")
    app.key("Return")
    app.wait(lambda: app.file("secret_left").exists(), msg="the pane typed into took the password")
    assert app.file("secret_left").read_text() == "kb"
    nothing_for(3, app.file("secret_right").exists)
    app.snap("the other prompt still waiting with the broadcast copy dropped")
    # the keyboard alone answers the other prompt once it is focused
    app.key("ctrl+shift+b")
    click_pane(app, "right")
    app.type("by_hand")
    app.key("Return")
    app.wait(lambda: app.file("secret_right").exists(), msg="the other prompt took the keyboard")
    assert app.file("secret_right").read_text() == "by_hand"


def test_a_broadcast_answers_a_question_that_is_no_secret(app_factory):
    """a program asking a question takes the broadcast: only a prompt with echo off refuses it"""
    app = two_panes(app_factory)
    left, right = pane_tabs(app)
    send(app, QUESTION, right)
    wait_text(app, "continue?", right)
    click_pane(app, "left")
    app.key("ctrl+shift+b")
    app.wait(lambda: framed(app, "left", "right"), msg="joined")
    app.type("sure")
    app.key("Return")
    app.wait(lambda: app.file("answer").exists, msg="the question took the broadcast answer", timeout=15)
    assert app.file("answer").read_text() == "sure"
    app.snap("the waiting program answered by a keystroke typed in the other pane")


def test_paste_is_broadcast_like_typing(app_factory):
    """pasting is input the view wrote itself, so the joined panes take it too: the same command
    ends up on the line of every joined pane"""
    app = two_panes(app_factory)
    left, right = pane_tabs(app)
    set_clipboard("echo pasted_everywhere")
    click_pane(app, "left")
    app.key("ctrl+shift+b")
    app.wait(lambda: framed(app, "left", "right"), msg="joined")
    app.key("ctrl+shift+v")
    app.wait(lambda: "pasted_everywhere" in text(app, left) and "pasted_everywhere" in text(app, right),
             msg="the pasted line on both prompts")
    app.snap("the pasted command waiting on the line of both joined panes")
    app.key("Return")
    wait_text(app, "pasted_everywhere\n", right)


def test_remote_and_palette_text_stay_in_one_pane(app_factory):
    """only the keyboard broadcasts: kuterm msg send-text and a palette step write into their
    own pane while the others are joined"""
    app = two_panes(app_factory, commands=[{"name": "macro it",
                                            "actions": [{"type": "echo from_macro\n"}]}])
    left, right = pane_tabs(app)
    click_pane(app, "left")
    app.key("ctrl+shift+b")
    app.wait(lambda: framed(app, "left", "right"), msg="joined")
    assert msg(app, "send-text", "--pane", left, "echo by_remote\n").returncode == 0
    wait_block(app, "echo by_remote", left)
    nothing_for(2, lambda: any("by_remote" in b["command"] for b in blocks(app, right)))
    app.palette("macro it")
    wait_block(app, "echo from_macro", left)
    nothing_for(2, lambda: any("from_macro" in b["command"] for b in blocks(app, right)))
    app.snap("the remote and the palette command typed into the left pane only")


def test_the_pane_typed_by_hand_has_no_mark_the_other_one_does(app_factory):
    """shell_integration.mark_sources: the pane that only took the broadcast carries the gutter
    mark in front of the command, the pane typed into by hand does not"""
    app = typed_panes(app_factory)
    left, right = pane_tabs(app)
    click_pane(app, "right")
    plain = pty_columns(app, right, "plain")
    click_pane(app, "left")
    app.key("ctrl+shift+b")
    app.wait(lambda: framed(app, "left", "right"), msg="joined")
    app.run("echo marked_where")
    assert wait_block(app, "echo marked_where", right)["source"] == ["broadcast"]
    assert wait_block(app, "echo marked_where", left)["source"] == ["keyboard"]
    app.key("ctrl+shift+b")
    app.wait(lambda: not framed(app, "left", "right"), msg="unjoined")
    app.snap("the command the other pane took carries a mark in its gutter")
    click_pane(app, "right")
    marked = pty_columns(app, right, "marked")
    assert marked == plain - 2, f"the pane that took the broadcast has a gutter: {marked} columns, plain {plain}"
    click_pane(app, "left")
    unmarked = pty_columns(app, left, "unmarked")
    assert unmarked == plain, f"the pane typed into by hand has no gutter: {unmarked} columns, plain {plain}"


# ---- synced scroll ----------------------------------------------------------


def join_scroll(app: App, *sides: str) -> None:
    for side in sides:
        click_pane(app, side)
        app.key("ctrl+shift+y")
        time.sleep(0.2)


def test_synced_panes_scroll_together_and_say_so(app_factory):
    """toggle_scroll_sync: wheeling in one pane scrolls the other by the same lines, both wear
    the "synced" label so the pair is never a surprise"""
    app = two_panes(app_factory, shell_integration={"mark_sources": "none"})
    left, right = pane_tabs(app)
    rows(app, left, 300)
    rows(app, right, 300)
    app.wait(lambda: "300" in text(app, left) and "300" in text(app, right)
             and pane_lines(app, "left") == pane_lines(app, "right"),
             msg="both panes at the bottom of the same numbers")
    before = pane_lines(app, "left")
    join_scroll(app, "left", "right")
    app.wait(lambda: synced_px(app, "left") > 100 and synced_px(app, "right") > 100, msg="synced labels")
    app.snap("both panes joined to the scroll sync, a label each")
    w, h = app.size()
    app.wheel(w // 4, h // 2, up=True, clicks=6)
    app.wait(lambda: pane_lines(app, "left") != before
             and pane_lines(app, "left") == pane_lines(app, "right"),
             msg="both scrolled up by the same lines")


def test_a_keymap_scroll_action_scrolls_the_synced_panes(app_factory):
    """scrolling from a keymap action is scrolling: the lines it moves are taken along by the
    other joined panes"""
    app = two_panes(app_factory, keymap=SCROLL_KEYS, shell_integration={"mark_sources": "none"})
    left, right = pane_tabs(app)
    rows(app, left, 300)
    rows(app, right, 300)
    app.wait(lambda: "300" in text(app, left) and pane_lines(app, "left") == pane_lines(app, "right"),
             msg="both panes at the bottom")
    before = pane_lines(app, "left")
    join_scroll(app, "left", "right")
    click_pane(app, "left")
    app.key("ctrl+shift+u")
    app.wait(lambda: pane_lines(app, "right") != before, msg="the other pane followed the action",
             timeout=15)
    assert pane_lines(app, "left") == pane_lines(app, "right"), "both moved by the same lines"
    app.snap("a scroll_up keymap action taken along by the synced pane")
    app.key("ctrl+shift+n")
    app.wait(lambda: pane_lines(app, "left") == before, msg="scrolled back down together", timeout=15)


def test_synced_panes_stop_at_their_own_history_ends(app_factory):
    """each pane stops at its own end: the pane with less history stays while the other keeps
    scrolling, and one pane without history at all breaks nothing"""
    app = two_panes(app_factory, shell_integration={"mark_sources": "none"})
    left, right = pane_tabs(app)
    rows(app, left, 300)
    # a pane with no history at all: the prompt line is all it ever wrote
    send(app, r"printf '\033[3J'", right)
    app.wait(lambda: "300" in text(app, left), msg="the left pane filled its history")
    time.sleep(1)
    empty = pane_lines(app, "right")
    before = pane_lines(app, "left")
    join_scroll(app, "left", "right")
    app.wait(lambda: synced_px(app, "left") > 100, msg="synced")
    w, h = app.size()
    app.wheel(w // 4, h // 2, up=True, clicks=20)
    app.wait(lambda: pane_lines(app, "left") != before, msg="the left pane scrolled")
    time.sleep(0.5)
    assert pane_lines(app, "right") == empty, "the pane without history has nothing to scroll"
    rest = pane_lines(app, "left")
    app.wheel(w // 4, h // 2, up=True, clicks=40)
    app.wait_still("the left pane reached the top of its history")
    assert pane_lines(app, "left") != rest, "the left pane kept going to its own top"
    app.snap("the synced pair at two different ends of their histories")
    app.wheel(w // 4, h // 2, up=False, clicks=200)
    app.wait(lambda: pane_lines(app, "left") == before, msg="back at the bottom")


def test_a_pane_with_a_short_history_stops_early(app_factory):
    """panes with different amounts of history each stop at their own end and stay joined"""
    app = two_panes(app_factory, shell_integration={"mark_sources": "none"})
    left, right = pane_tabs(app)
    rows(app, left, 300)
    rows(app, right, 40)
    app.wait(lambda: "300" in text(app, left) and "40" in text(app, right), msg="both panes filled")
    left_bottom, right_bottom = pane_lines(app, "left"), pane_lines(app, "right")
    join_scroll(app, "left", "right")
    click_pane(app, "left")
    w, h = app.size()
    app.wheel(w // 4, h // 2, up=True, clicks=40)
    app.wait(lambda: pane_lines(app, "left") != left_bottom, msg="the long pane scrolled")
    app.wait(lambda: pane_lines(app, "right") != right_bottom, msg="the short pane scrolled to its own top")
    stopped = pane_lines(app, "right")
    app.wheel(w // 4, h // 2, up=True, clicks=20)
    time.sleep(1)
    assert pane_lines(app, "right") == stopped, "the short pane stopped at its own top"
    assert pane_lines(app, "left") != left_bottom, "the long pane kept going up its own history"
    app.snap("the short pane at its top, the long one further up")


# ---- both together ----------------------------------------------------------


def test_broadcast_and_scroll_sync_at_once(app_factory):
    """the two switches are independent: the same panes take the typing and follow each other's
    scrolling, each carrying both marks"""
    app = two_panes(app_factory, shell_integration={"mark_sources": "none"})
    left, right = pane_tabs(app)
    rows(app, left, 300)
    rows(app, right, 300)
    app.wait(lambda: "300" in text(app, left) and pane_lines(app, "left") == pane_lines(app, "right"),
             msg="both at the bottom")
    click_pane(app, "left")
    app.key("ctrl+shift+b")
    join_scroll(app, "left", "right")
    app.wait(lambda: framed(app, "left", "right"), msg="both joined to the broadcast")
    app.wait(lambda: synced_px(app, "left") > 100 and synced_px(app, "right") > 100, msg="synced labels")
    app.snap("two panes joined to the broadcast and to the scroll sync")
    before = pane_lines(app, "left")
    app.run("echo both_at_once")
    wait_block(app, "echo both_at_once", right)
    w, h = app.size()
    app.wheel(w // 4, h // 2, up=True, clicks=6)
    app.wait(lambda: pane_lines(app, "left") != before
             and pane_lines(app, "left") == pane_lines(app, "right"),
             msg="scrolled together after typing together")


def test_a_queued_command_is_typed_into_its_own_pane_only(app_factory):
    """the queue writes into one pty: a command queued while the broadcast runs never follows it
    into the other pane"""
    app = two_panes(app_factory, window_title=["number", {"text": " "}, "queue"])
    left, right = pane_tabs(app)
    click_pane(app, "left")
    app.key("ctrl+shift+b")
    app.wait(lambda: framed(app, "left", "right"), msg="joined")
    send(app, "sleep 4", left)
    wait_block(app, "sleep 4", left, finished=False)
    assert msg(app, "queue", "--pane", left, "echo from_queue").returncode == 0
    app.wait(lambda: "⧗" in app.title(), msg="the queue listed in the title")
    app.snap("a queued command waiting while both panes are joined")
    wait_block(app, "echo from_queue", left)
    nothing_for(2, lambda: any("from_queue" in b["command"] for b in blocks(app, right)))
    assert "from_queue" not in text(app, right)


def test_a_failure_pauses_only_the_queue_of_its_own_pane(app_factory):
    """with terminal.queue_stop_on_failure the failing pane waits for a resume while the typing
    still reaches every joined pane"""
    app = two_panes(app_factory, terminal={"queue_stop_on_failure": True},
                    window_title=["number", {"text": " "}, "queue"])
    left, right = pane_tabs(app)
    click_pane(app, "left")
    app.key("ctrl+shift+b")
    app.wait(lambda: framed(app, "left", "right"), msg="joined")
    send(app, "sleep 2; false", left)
    wait_block(app, "sleep 2", left, finished=False)
    assert msg(app, "queue", "--pane", left, "touch $T2_TMP/after_failure").returncode == 0
    app.wait(lambda: "⧗" in app.title(), msg="the queue listed in the title")
    nothing_for(4, app.file("after_failure").exists)
    app.snap("the queue paused by the failure, the panes still joined")
    app.run("echo still_joined")
    app.wait(lambda: "still_joined" in text(app, right), msg="the broadcast still works while paused",
             timeout=15)
    app.palette("resume queue")
    app.wait(app.file("after_failure").exists, msg="the resumed entry ran")


def test_mark_sources_none_draws_no_gutter_for_a_broadcast(app_factory):
    """with shell_integration.mark_sources "none" the receiving pane keeps its columns: no mark
    is drawn for the broadcast either"""
    app = two_panes(app_factory, shell_integration={"mark_sources": "none"})
    left, right = pane_tabs(app)
    click_pane(app, "right")
    plain = pty_columns(app, right, "plain")
    click_pane(app, "left")
    app.key("ctrl+shift+b")
    app.wait(lambda: framed(app, "left", "right"), msg="joined")
    app.run("echo no_gutter")
    wait_block(app, "echo no_gutter", right)
    app.key("ctrl+shift+b")
    app.wait(lambda: not framed(app, "left", "right"), msg="unjoined")
    click_pane(app, "right")
    assert pty_columns(app, right, "after") == plain, "no gutter for the broadcast command"
