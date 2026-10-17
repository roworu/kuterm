"""the new settings used together, not one at a time: sleeping tabs next to search, the command
queue, broadcast, a stopped job and incognito tabs, every way a command can be typed next to the
source gutter, keep print width with search, a selection and a held frame, the completion popup
with its keys, its keymap and the shell, a question and an agent notifying together, and one app
started with every new setting away from its default"""

import os
import subprocess
import tempfile
import time
from pathlib import Path

import numpy as np
import pytest

from harness import (BACKEND, BIN, App, Notifications, as_rgb, close_to, count, dominant,
                     get_clipboard, ink, ink_bbox, mask_bbox, near, set_clipboard, unique_theme)
from kit import (blocks, executable, integrated, listening, msg, nothing_for, notification_buttons,
                 ready, send, socket_of, tabs, text, wait_block, wait_buttons, wait_text)

FEATURE = "new settings together"

THEME = {**unique_theme(0xc1), "search_match": "#ffd400", "search_match_active": "#ff4000"}
C = as_rgb(THEME)

# keys for the features in this file, a test passes the ones it uses
KEYS = {"split_right": "ctrl-shift-r", "toggle_broadcast_all": "ctrl-shift-b",
        "tab_overview": "ctrl-shift-o"}

# a shell command that asks and waits, and a program printing lines forever
QUESTION = "printf 'continue? '; IFS= read -r x; echo \"$x\" > $T2_TMP/answer"
LOOP = "i=0; while :; do i=$((i+1)); echo tick_$i; sleep 0.15; done"


def combo_app(app_factory, *, keybindings=None, commands=None, env=None, tab_icons=None,
              **settings) -> App:
    """bash with kuterm's shell integration, remote control and a readable title, with the
    settings of the test merged over it"""
    base = integrated(window_title=["number", {"text": " "}, "icon"], icons={"default": "D"})
    for key, value in settings.items():
        if isinstance(value, dict) and isinstance(base.get(key), dict):
            base[key] = {**base[key], **value}
        else:
            base[key] = value
    return app_factory(base, themes={"dark": THEME}, keybindings=keybindings,
                       commands={"commands": commands} if commands else None, env=env,
                       tab_icons=tab_icons)


def sleeping_app(app_factory, minutes: float, **settings) -> App:
    """the same with hidden tabs sleeping after `minutes`, keeping all their history"""
    terminal = {"max_history_length": 0, **settings.pop("terminal", {})}
    return combo_app(app_factory, sleep_tabs_after_minutes=minutes, terminal=terminal, **settings)


def first_tab(app: App) -> str:
    """id and pane of tab 1, what the sleeping helpers and `--pane` resolve"""
    return str(next(t["id"] for t in tabs(app) if t["number"] == 1))


def tab_title(app: App, number: int) -> str:
    """title of a tab, built from its "tab_title" blocks"""
    return next(t["title"] for t in tabs(app) if t["number"] == number)


def active_pane(app: App) -> str:
    return str(next(t["id"] for t in tabs(app) if t["active"]))


def pane_front(app: App, pane: int) -> str:
    """id of the tab a pane shows"""
    return str(next(t["id"] for t in tabs(app) if t["pane"] == pane and t["active"]))


def pane_ids(app: App) -> list:
    return sorted({t["pane"] for t in tabs(app)})


def asleep(app: App, pane: str) -> bool:
    return next(t["asleep"] for t in tabs(app) if str(t["id"]) == pane)


def wait_asleep(app: App, pane: str) -> None:
    app.wait(lambda: asleep(app, pane), msg="the hidden tab asleep", timeout=15)


def fill_and_hide(app: App, lines: int = 1200, marker: str = "printed_all") -> str:
    """tab 1 filled with history and a new tab 2 in front of it, so tab 1 can fall asleep"""
    send(app, f"seq 1 {lines}; echo {marker}")
    pane = first_tab(app)
    wait_text(app, marker, pane)
    app.key("ctrl+shift+t")
    app.wait_title("2 D")
    return pane


def queue(app: App, command: str, pane: str | None = None) -> None:
    args = ["queue"] + (["--pane", pane] if pane else []) + [command]
    done = msg(app, *args)
    assert done.returncode == 0, done.stderr


def columns_seen(app: App, name: str, pane: str | None = None) -> int:
    """columns the shell of a pane sees, asked over remote control"""
    send(app, f"stty size > $T2_TMP/{name}.part && mv $T2_TMP/{name}.part $T2_TMP/{name}", pane)
    return int(app.wait_file(name).split()[1])


def marked(app: App) -> int:
    """attention colored pixels in the tab bar"""
    return count(app.shot()[: app.bar_height()], C["attention"], 3)


def bottom_banner(app: App):
    """bbox of the attention colored banner in the bottom right corner of the pane"""
    img = app.shot()
    h, w = img.shape[:2]
    mask = near(img[h * 3 // 4 :, w // 2 :], C["attention"])
    return mask_bbox(mask) if mask.sum() > 100 else None


def text_rows(app: App) -> int:
    """rows of the grid holding any text"""
    img = app.shot()[app.bar_height() + 2 :, : -30]
    rows = ink(img, C["terminal_background"]).any(axis=1)
    lh = round(app.line_height())
    return int(sum(rows[i : i + lh].any() for i in range(0, len(rows), lh)))


def row_runs(app: App) -> list:
    """widest run of ink pixels on each row of the grid, top first, with the gaps between the
    glyphs of one word bridged"""
    rows = ink(grid_above_cursor(app), C["terminal_background"])
    lh = round(app.line_height())
    widest = []
    for i in range(0, rows.shape[0] - lh, lh):
        spans = runs(np.flatnonzero(rows[i : i + lh].any(axis=0)))
        widest.append(max((end - start for start, end in spans), default=0))
    return widest


def wide_rows(app: App, cells: int = 6) -> int:
    """grid rows holding an unbroken run of ink at least `cells` wide, like a long line of A's,
    the short seq lines of the test are well under that"""
    return sum(1 for run in row_runs(app) if run >= cells * app.cell_width())


def kept_row(app: App, cells: int = 6) -> int:
    """grid row holding the kept line, the only one with a run that long"""
    for row, run in enumerate(row_runs(app)):
        if run >= cells * app.cell_width():
            return row
    raise AssertionError("no long line on screen")


def wait_narrowed(app: App, width: int = 450, height: int = 600) -> None:
    """after narrowing, the frame is drawn at the new width, no line wider than its cells"""
    cols = app.layout_pty(width, height)[1]
    app.wait(lambda: max(row_runs(app), default=0) <= (cols + 2) * app.cell_width(),
             msg="a frame drawn at the narrowed width")
    app.wait_still()


def wait_prompt(app: App) -> None:
    """the shell drew its prompt again, so the command before it ended"""
    app.wait(lambda: text(app).rstrip().endswith(("$", "#")),
             msg="the prompt back after the command")


def grid_above_cursor(app: App) -> np.ndarray:
    """the grid without the last line, where the blinking cursor sits, and without the scrollbar,
    which follows the output on its own"""
    img = app.shot()
    lh = round(app.line_height())
    return img[app.bar_height() + 4 : img.shape[0] - lh - 8][:, 8:-34]


def runs(cols) -> list:
    """(first, last) of each run of column numbers, gaps of up to 3 columns are bridged"""
    if not len(cols):
        return []
    return [(int(run[0]), int(run[-1]))
            for run in np.split(cols, np.flatnonzero(np.diff(cols) > 4) + 1)]


def chips(app: App, img=None) -> list:
    """bboxes of the group chips in the tab bar, left to right"""
    img = app.shot() if img is None else img
    h = app.bar_height()
    found = []
    for ix in range(1, 7):
        mask = near(img[1:h], C["ansi"][ix], 2)
        for start, end in runs(np.flatnonzero(mask.sum(axis=0) >= 8)):
            rows = np.flatnonzero(mask[:, start : end + 1].mean(axis=1) > 0.5)
            found.append((start, int(rows[0]) + 1, end + 1, int(rows[-1]) + 2))
    return sorted(found)


def chip_colour(app: App, box: tuple, img=None) -> int:
    """ansi index of the fill of a group chip"""
    img = app.shot() if img is None else img
    x0, y0, x1, y1 = box
    fill = dominant(img[y0 + 2 : y1 - 2, x0 + 2 : x1 - 2])
    for ix in range(1, 7):
        if close_to(fill, C["ansi"][ix], 4):
            return ix
    raise AssertionError(f"the chip is not in a group color: {fill}")


def project(app: App, name: str) -> Path:
    """a folder that looks like a git project to the project groups and the git block"""
    root = app.file(name)
    (root / ".git").mkdir(parents=True, exist_ok=True)
    return root


def status_bar_shown(app: App, img=None) -> bool:
    """the bottom band of the window is painted in the status bar color, its text on top"""
    img = app.shot() if img is None else img
    h = round(round(1.5 * app.ui_font_size()) * app.scale)
    # the top border row is the theme border, the bar itself is filled with its own color
    return close_to(dominant(img[img.shape[0] - h + 4 :, 20:-20]), C["tab_bar_background"], 3)


def status_text(app: App, img=None) -> tuple | None:
    """bbox of the text on the status bar, none while the bar is empty"""
    img = app.shot() if img is None else img
    h = round(round(1.5 * app.ui_font_size()) * app.scale) - 2
    box = mask_bbox(ink(img[img.shape[0] - h :], C["tab_bar_background"]))
    return box and (box[0], box[1] + img.shape[0] - h, box[2], box[3] + img.shape[0] - h)


def picker(app: App, img=None) -> bool:
    """the panel a pick opens over the pane, its background filling the middle of the window"""
    img = app.shot() if img is None else img
    w, h = img.shape[1], img.shape[0]
    mid = near(img[app.top() : 3 * h // 4, w * 2 // 5 : 3 * w // 5], C["tab_bar_background"], 2)
    return mid.sum() > 500


def run_link(app: App, command: str) -> None:
    """hand a kuterm://run link to the running app, which types it into a tab of its own"""
    done = subprocess.run([str(BIN), f"kuterm://run?cmd={command}"],
                          env={**app.app_env(), "KUTERM_SOCKET": str(socket_of(app))},
                          capture_output=True, text=True, timeout=10)
    assert done.returncode == 0, done.stderr


# the fake agent reports busy over osc 9;4 like an agent does, says something, then stops being
# busy without finishing and waits. python3 with a script named like an ai cli, and the test only
# lets it go idle once the app saw the program running in front
FAKE_AGENT = '''#!/usr/bin/python3
import os, sys, time
tmp = os.environ["T2_TMP"]
def say(text):
    sys.stdout.write(text)
    sys.stdout.flush()
say("\\033]9;4;3\\007working\\n")
say("\\033]9;the agent says hello\\007")
say("waiting for the tool to be allowed\\n")
open(os.path.join(tmp, "agent_busy"), "w").write("1")
while not os.path.exists(os.path.join(tmp, "agent_go")):
    time.sleep(0.02)
say("\\033]9;4;0\\007allow? ")
open(os.path.join(tmp, "agent_idle"), "w").write("1")
time.sleep(1000)
'''


# a program reading a secret with echo off, like sudo does, so the app can tell it apart from a
# shell's own prompt
SECRET_PROMPT = ("python3 -c \"import getpass,sys; sys.stdout.write('READY\\n'); sys.stdout.flush(); "
                 "s = getpass.getpass('pw: '); open('$T2_TMP/locked','w').write('len %d' % len(s))\"")


def agent_path() -> Path:
    """folder to put on PATH with a fake `claude` in it"""
    folder = Path(tempfile.mkdtemp(prefix="e2e-agent-"))
    executable(folder / "claude", FAKE_AGENT)
    return folder


# ---- the completion popup helpers -------------------------------------------


def line_cells(app: App) -> int:
    """cells the first row has text over"""
    x0, y0, x1, y1 = app.cell_rect(0, 0, cols=60)
    band = app.shot()[y0:y1, x0:x1]
    box = ink_bbox(band, dominant(band))
    return 0 if box is None else round(box[2] / app.cell_width())


def popup(app: App):
    """bbox of the completion popup below the first row, none while it is hidden. the list sits
    at the top left of the pane, the notices in the bottom right corner are not part of it"""
    below = app.cell_rect(0, 1)[1]
    img = app.shot()
    h, w = img.shape[:2]
    area = img[below : h * 3 // 4, : w // 2]
    mask = near(area, C["tab_bar_background"]) | near(area, C["tab_active_background"])
    if mask.sum() < 500:
        return None
    x0, y0, x1, y1 = mask_bbox(mask)
    return x0, y0 + below, x1, y1 + below


def popup_rows(app: App) -> int:
    box = popup(app)
    return 0 if box is None else round((box[3] - box[1]) / app.line_height())


def zsh_app(app_factory, tmp_path, **settings) -> App:
    """zsh on kuterm's completion socket, in a folder with ten files, with its own settings"""
    zdotdir = tmp_path / "zdotdir"
    zdotdir.mkdir(parents=True)
    prompt = tmp_path / "prompt"
    (zdotdir / ".zshrc").write_text(f"PS1='$ '\nunsetopt prompt_sp\nprecmd() {{ : > {prompt} }}\n")
    work = tmp_path / "work"
    work.mkdir(parents=True, exist_ok=True)
    for ix in range(10):
        (work / f"file_{ix}.txt").write_text("")
    profile = {"name": "zsh", "command": {"program": "/usr/bin/zsh"},
               "working_directory": str(work), "env": {"ZDOTDIR": str(zdotdir)}}
    # one tab and no tab bar: the completion helpers measure cells from the top of the window
    app = combo_app(app_factory, profiles=[profile], hide_bar_for_one_tab=True,
                    autocomplete={"enable": True, **settings.pop("autocomplete", {})}, **settings)
    app.wait(prompt.exists, msg="the zsh prompt")
    app.mouse(450, 300)
    return app


# ---- sleeping tabs beside the other features --------------------------------


def test_a_scoped_search_and_find_in_all_tabs_over_a_sleeping_tab(app_factory):
    """a search of this tab scoped down to its failed output, then find in all tabs with the same
    "failed:" prefix picking a match out of a sleeping tab: the pick switches over and wakes it"""
    app = sleeping_app(app_factory, 0.02, tab_title=["number", {"text": " "}, "icon"])
    send(app, "seq 1 1200 | sed 's/^/needle_/'; echo needle_printed; false")
    pane = first_tab(app)
    wait_block(app, "seq 1 1200", pane)
    app.key("ctrl+shift+t")
    app.wait_title("2 D")
    wait_asleep(app, pane)
    send(app, "echo needle_here; false")
    wait_block(app, "echo needle_here")
    app.key("ctrl+shift+f")
    app.type("needle_here")
    app.wait(lambda: near(app.shot(), C["search_match_active"], 2).sum() > 20, msg="the match lit")
    app.key("alt+s", "alt+s")
    app.wait(lambda: near(app.shot(), C["search_match_active"], 2).sum() > 20,
             msg="the failed output alone")
    app.snap("this tab's search, scoped down to what its failed command printed")
    app.key("Escape")
    app.palette("find in all tabs")
    app.type("failed:needle_printed")
    app.wait(lambda: near(app.shot(), C["search_match_active"], 2).sum() > 20,
             msg="the failed output of the sleeping tab listed")
    app.snap("find in all tabs reached a failed output of a tab whose history is packed away")
    app.key("Return")
    app.wait_title("1 D")
    app.wait(lambda: not asleep(app, pane), msg="the picked tab woken")
    assert "needle_printed" in text(app, pane)
    app.snap("switched to the picked tab, its history back")


def test_a_sleeping_tab_keeps_its_print_width_across_a_hidden_resize(app_factory):
    """keep_block_width with sleep_tabs_after_minutes: a wide line printed before the tab fell
    asleep stays on one row when the window is narrowed while it is hidden, and takes a second
    row without the setting. each app is measured before the next opens: on wayland both
    windows land on the same rect and the newer one would cover the older"""
    apps = {}
    for keep in (True, False):
        app = sleeping_app(app_factory, 0.02, shell_integration={"mark_sources": "none"},
                           terminal={"keep_block_width": keep})
        ready(app)
        send(app, "seq 1 3000; printf 'A%.0s' $(seq 70); echo B")
        pane = first_tab(app)
        wait_text(app, "A" * 40, pane)
        app.wait_still()
        before = wide_rows(app)
        app.key("ctrl+shift+t")
        app.wait_title("2 D")
        wait_asleep(app, pane)
        app.resize(450, 600)
        time.sleep(1)
        app.key("ctrl+Tab")
        app.wait_title("1 D")
        app.wait(lambda: not asleep(app, pane), msg="woken")
        app.wait_still()
        apps[keep] = (before, wide_rows(app), text(app, pane))
        app.snap("woken from sleep after a hidden resize: the wide line printed at 900px, one row"
                 if keep else "without keep_block_width the same line is rewrapped over two rows")
    (kept_before, kept_after, kept_text), (plain_before, plain_after, _) = apps[True], apps[False]
    assert kept_before == 1 and plain_before == 1, ("the line at 900px", kept_before, plain_before)
    assert kept_after == 1, "the woken line took more than its one row back"
    assert plain_after == 2, "without keep_block_width the line is not rewrapped"
    assert "A" * 70 + "B" in kept_text.replace("\n", "")


def test_a_sleeping_tab_pauses_its_queue_on_a_failure(app_factory):
    """queue_stop_on_failure with a sleeping tab: the queue keeps running while the tab is
    hidden, a failure pauses it there, the tab title says one entry waits, and resume queue
    from the palette runs the rest"""
    app = sleeping_app(app_factory, 0.02, tab_title=["number", {"text": " "}, "queue"],
                       terminal={"queue_stop_on_failure": True})
    pane = first_tab(app)
    ready(app, pane)
    send(app, "seq 1 3000; echo filled", pane)
    wait_block(app, "seq 1 3000", pane)
    send(app, "sleep 4", pane)
    wait_block(app, "sleep 4", pane, finished=False)
    queue(app, "false", pane)
    queue(app, "touch $T2_TMP/after", pane)
    app.key("ctrl+shift+t")
    app.wait_title("2 D")
    wait_asleep(app, pane)
    app.wait(lambda: "⧗" in tab_title(app, 1), msg="the paused entry in the tab title", timeout=20)
    nothing_for(3, app.file("after").exists)
    assert asleep(app, pane), "the failure woke the sleeping tab"
    app.snap("the hidden sleeping tab paused its queue, its title says one entry waits")
    app.key("ctrl+Tab")
    app.wait_title("1 D")
    app.palette("resume queue")
    app.wait(app.file("after").exists, msg="the resumed entry ran")


def test_a_sleeping_tab_runs_its_queue_through_a_failure(app_factory):
    """with queue_stop_on_failure off the queue goes on inside the sleeping tab: its commands
    run and write their files while the tab stays hidden and asleep"""
    app = sleeping_app(app_factory, 0.02, terminal={"queue_stop_on_failure": False})
    pane = first_tab(app)
    ready(app, pane)
    send(app, "seq 1 3000; echo filled", pane)
    wait_block(app, "seq 1 3000", pane)
    send(app, "sleep 4", pane)
    wait_block(app, "sleep 4", pane, finished=False)
    queue(app, "false", pane)
    queue(app, "touch $T2_TMP/went_on", pane)
    app.key("ctrl+shift+t")
    app.wait_title("2 D")
    wait_asleep(app, pane)
    app.wait(app.file("went_on").exists, msg="the queue went on in the sleeping tab", timeout=25)
    assert asleep(app, pane), "the queue ran without waking the tab"
    app.snap("the queue ran through its failure inside the sleeping tab")


def test_a_stopped_job_is_still_there_after_a_sleep(app_factory):
    """a job stopped with ctrl+z survives the tab sleeping: its tab title block shows it while
    the tab is hidden, and the banner brings it back in front"""
    app = sleeping_app(app_factory, 0.02, window_title=["number", {"text": " "}, "command"],
                       tab_title=["number", {"text": " "}, "stopped_jobs"])
    pane = first_tab(app)
    send(app, "seq 1 3000; echo filled", pane)
    wait_text(app, "filled", pane)
    send(app, "sleep 100")
    app.wait_title("1 sleep")
    app.key("ctrl+z")
    app.wait_title("1 bash")
    app.wait(lambda: "stopped" in tab_title(app, 1), msg="the stopped job in the tab title")
    assert "sleep" in tab_title(app, 1)
    app.key("ctrl+shift+t")
    app.wait_title("2 bash")
    wait_asleep(app, pane)
    assert "stopped" in tab_title(app, 1), "the stopped job fell out of the sleeping tab"
    app.snap("the sleeping tab still has its stopped sleep in the tab title")
    app.key("ctrl+Tab")
    app.wait_title("1 bash")
    box = app.wait(lambda: bottom_banner(app), msg="the stopped job banner", timeout=15)
    app.snap("woken with the stopped job still waiting under the prompt")
    w, h = app.size()
    app.click(w // 2 + (box[0] + box[2]) // 2, h * 3 // 4 + (box[1] + box[3]) // 2)
    app.wait_title("1 sleep", timeout=15)
    app.wait(lambda: bottom_banner(app) is None, msg="the banner gone once it runs again")


def test_broadcast_never_reaches_a_sleeping_tab(app_factory):
    """broadcast in a window of two panes where a tab sleeps behind another one: the typed keys
    reach the front tab of each pane, the sleeping tab gets nothing, keeps its history and stays
    asleep"""
    app = sleeping_app(app_factory, 0.02, keybindings=KEYS,
                       shell_integration={"mark_sources": "none"})
    ready(app)
    # the split moves the front tab into the pane on the right and leaves a fresh shell on the
    # left, so the tab that sleeps later has a front tab of its own still watching it
    app.key("ctrl+shift+r")
    app.wait(lambda: len(pane_ids(app)) == 2, msg="the second pane")
    sleeper = first_tab(app)
    send(app, "seq 1 1200; echo boot_only", sleeper)
    wait_text(app, "boot_only", sleeper)
    # a new tab goes into the focused pane, so the sleeper's pane is focused first: a sleeping
    # tab is the active tab of no pane, or the broadcast would type into it. the split moved the
    # sleeper into the pane on the left
    app.click(225, 300)
    app.key("ctrl+shift+t")
    app.wait(lambda: len(tabs(app)) == 3, msg="the tab in front of the sleeper")
    wait_asleep(app, sleeper)
    app.snap("the tab asleep behind the front one of its pane, the other pane beside it")
    app.key("ctrl+shift+b")
    app.type("echo bcast_marker")
    app.key("Return")
    front = pane_front(app, pane_ids(app)[0])
    wait_text(app, "bcast_marker", front)
    assert asleep(app, sleeper), "the broadcast woke the sleeping tab"
    # reading a sleeping tab unpacks its history, this one may show its own commands only
    assert "bcast_marker" not in text(app, sleeper)
    app.snap("the keys reached both panes, the sleeping tab untouched and asleep")


def test_an_incognito_tab_sleeps_and_wakes_by_being_shown(app_factory):
    """an incognito tab sleeps like any other, a read that is refused is refused, and showing it
    again wakes it with its history back"""
    app = sleeping_app(app_factory, 0.02, window_title=["number", {"text": " "}, "icon"])
    app.palette("new incognito tab")
    app.wait_title(lambda title: title.startswith("2 "))
    pane = active_pane(app)
    # nothing but the keyboard and the pasted text can type into an incognito tab
    app.type("seq 1 1200; echo secret_printed; echo ran > $T2_TMP/incognito_ran")
    app.key("Return")
    app.wait_file("incognito_ran", timeout=20)
    app.key("ctrl+Tab")
    app.wait_title("1 D")
    wait_asleep(app, pane)
    done = msg(app, "get-text", "--pane", pane)
    assert done.returncode != 0 and "incognito" in done.stderr + done.stdout, done
    app.snap("the sleeping incognito tab cannot be read over remote control")
    app.key("ctrl+Tab")
    app.wait(lambda: not asleep(app, pane), msg="shown, so awake again")
    # its own output cannot be read back either, the filled screen is the sign it was kept
    app.wait(lambda: text_rows(app) > 12, msg="the history of the woken incognito tab on screen")
    app.snap("the incognito tab shown again, its history back on screen")


def test_a_refused_read_does_not_wake_a_sleeping_incognito_tab(app_factory):
    """an incognito tab asleep behind another one: the read the app refuses may not unpack the
    history it was asked to keep out of reach"""
    app = sleeping_app(app_factory, 0.02, window_title=["number", {"text": " "}, "icon"])
    app.palette("new incognito tab")
    app.wait_title(lambda title: title.startswith("2 "))
    pane = active_pane(app)
    app.type("seq 1 1200; echo secret_printed; echo ran > $T2_TMP/incognito_ran")
    app.key("Return")
    app.wait_file("incognito_ran", timeout=20)
    app.key("ctrl+Tab")
    app.wait_title("1 D")
    wait_asleep(app, pane)
    done = msg(app, "get-text", "--pane", pane)
    assert done.returncode != 0, done
    time.sleep(0.5)
    assert asleep(app, pane), "the refused read woke the incognito tab"


# ---- the source gutter next to the other inputs -----------------------------


def test_mark_sources_all_marks_every_source_with_one_gutter(app_factory):
    """mark_sources all with commands typed by the keyboard, by remote control, by a palette
    command, by the queue and pasted: every one is marked, the blocks say who did it, and the
    gutter stays two cells wide while the marks pile up"""
    plain = combo_app(app_factory, shell_integration={"mark_sources": "none"})
    ready(plain)
    plain_cols = columns_seen(plain, "plain")
    app = combo_app(app_factory, shell_integration={"mark_sources": "all"},
                    terminal={"paste_protection": False},
                    commands=[{"name": "macro it",
                               "actions": [{"type": "touch $T2_TMP/m_macro\n"}]}])
    ready(app)
    send(app, "true")
    wait_block(app, "true")
    assert columns_seen(app, "typed") == plain_cols - 2, "a typed command is marked in all mode"
    app.type("touch $T2_TMP/m_keys")
    app.key("Return")
    app.wait_file("m_keys")
    send(app, "touch $T2_TMP/m_remote")
    app.wait_file("m_remote")
    app.palette("macro it")
    app.wait_file("m_macro")
    queue(app, "touch $T2_TMP/m_queue")
    app.wait_file("m_queue")
    set_clipboard("touch $T2_TMP/m_paste\n")
    app.wait(lambda: get_clipboard() == "touch $T2_TMP/m_paste\n", msg="clipboard owner")
    app.key("ctrl+shift+v")
    # the paste puts the text on the line, the keyboard's return runs it
    app.key("Return")
    app.wait_file("m_paste")
    assert columns_seen(app, "last") == plain_cols - 2, "one gutter, not one per source"
    sources = [tuple(b["source"]) for b in blocks(app)]
    for wanted in (("keyboard",), ("remote",), ("macro",), ("automation",)):
        assert wanted in sources, (wanted, sources)
    assert any("paste" in found for found in sources), sources
    app.snap("keyboard, remote, palette, queue and pasted commands, each with its mark")


def test_marks_none_without_injection_holds_the_queue_until_a_reload(app_factory):
    """shell_integration.inject off with mark_sources none: no gutter, no blocks, and the queue
    cannot know the prompt came back so it waits; a new tab after a reload has injection and runs
    its own queue while the old shell keeps waiting"""
    app = combo_app(app_factory, shell_integration={"inject": False, "mark_sources": "none"},
                    tab_title=["number", {"text": " "}, "queue"])
    listening(app)
    app.mouse(450, 300)
    assert app.pty_size() == app.layout_pty(900, 600), "no marks, no gutter"
    send(app, "true")
    time.sleep(1)
    assert blocks(app) == [], "a shell without the integration has no blocks"
    queue(app, "touch $T2_TMP/never")
    nothing_for(4, app.file("never").exists)
    assert "⧗" in tab_title(app, 1), tab_title(app, 1)
    app.snap("nothing marks this shell, so its queue never knows the prompt is free")
    # the status bar coming on in the same write shrinks the grid: the reload was read before
    # the new tab is spawned, and only new shells get the integration
    size = app.pty_size()
    app.write("settings.jsonc", {**app.settings, "status_bar": {"enable": True},
                                 "shell_integration": {"inject": True, "mark_sources": "none"}})
    app.wait(lambda: app.pty_size() != size, msg="the reload read", timeout=15)
    app.key("ctrl+shift+t")
    app.wait(lambda: len(tabs(app)) == 2, msg="the new tab")
    fresh = active_pane(app)
    ready(app, fresh)
    queue(app, "touch $T2_TMP/freed", fresh)
    app.wait(app.file("freed").exists, msg="the new tab ran its queue", timeout=20)
    nothing_for(2, app.file("never").exists)
    assert "⧗" in tab_title(app, 1), "the queue of the shell without injection still waits"
    app.snap("after the reload a new tab runs its queue, the uninjected shell still waits")


def test_a_run_link_lands_beside_a_sleeping_tab(app_factory):
    """a kuterm://run link while another tab sleeps: the command is typed into a tab of its own,
    the sleeping tab is not woken, and running the line is marked as automation"""
    app = sleeping_app(app_factory, 0.02, shell_integration={"mark_sources": "none"})
    pane = fill_and_hide(app, marker="boot_only")
    wait_asleep(app, pane)
    run_link(app, "echo%20from_link")
    app.wait(lambda: len(tabs(app)) == 3, msg="the tab of the link")
    assert asleep(app, pane), "the link woke the sleeping tab"
    wait_text(app, "echo from_link")
    app.snap("the link typed into a tab of its own, the sleeper left alone")
    app.key("Return")
    link_tab = active_pane(app)
    block = wait_block(app, "echo from_link", link_tab, timeout=20)
    assert block["source"] == ["keyboard", "automation"], block
    assert asleep(app, pane), "reading the block woke the sleeping tab"
    wait_text(app, "boot_only", pane, scrollback=True)


# ---- keep print width with a search and a selection -------------------------


def lit_matches(app: App) -> int:
    """pixels of lit search matches, whichever of the two colors they are drawn in"""
    img = app.shot()
    return int(near(img, C["search_match_active"], 2).sum() + near(img, C["search_match"], 2).sum())


def ticks_in(blob: str) -> list:
    """the tick numbers in some text, ignoring the loop's own command line, which is echoed too"""
    return [int(word[5:]) for word in blob.split() if word.startswith("tick_") and word[5:].isdigit()]


def clipboard_wait(app: App, marker: str, msg: str) -> str:
    """the clipboard once it holds `marker`, one read per poll: it changes hands while a copy
    is in flight"""
    def check() -> str | None:
        copied = get_clipboard()
        return copied if marker in copied else None

    return app.wait(check, msg=msg)


def test_a_kept_line_lights_up_and_copies_whole(app_factory):
    """keep_block_width with the search and a triple click, the held frame off: after the
    window narrowed, what is drawn of the kept line still lights up when searched, and the
    copy of the line is the whole line on one row, the way it was printed"""
    app = combo_app(app_factory, terminal={"keep_block_width": True},
                    selection={"hold_frame": False},
                    shell_integration={"mark_sources": "none"})
    ready(app)
    # six short lines above the long one, so it is not hidden under the search bar
    send(app, "clear; seq 1 6; printf 'KEEP_ME %s\\n' \"$(printf 'A%.0s' $(seq 70))\"")
    wait_text(app, "KEEP_ME")
    # the block has to end before the resize, else there is nothing to copy aside
    wait_prompt(app)
    app.resize(450, 600)
    wait_narrowed(app)
    app.key("ctrl+shift+f")
    app.type("AAAA")
    app.wait(lambda: lit_matches(app) > 20, msg="the match in the drawn part of the kept line lit")
    app.snap("a match inside the kept line is highlighted after the window narrowed")
    app.key("Escape")
    app.wait_still()
    # a triple click takes the whole line, wrapped rows and all; the row comes from the pixels
    # because opening the search bar scrolls the grid up by a line, under the window coordinates
    x0, y0, _, y1 = app.cell_rect(0, kept_row(app), top=app.top())
    app.click(60, (y0 + y1) // 2, repeat=3)
    app.key("ctrl+shift+c")
    copied = clipboard_wait(app, "KEEP_ME", "the copied kept line")
    line = copied.strip()
    assert line.startswith("KEEP_ME ") and "\n" not in line, repr(copied[:120])
    assert line.count("A") == 70, repr(copied[:120])


def test_a_held_frame_keeps_a_line_selection_on_a_kept_row(app_factory):
    """selection.hold_frame on with keep_block_width: a triple click on a kept row takes the
    whole line, so the release hands the selection back on the same text and the copy is the
    line as it was printed, one row without a newline"""
    app = combo_app(app_factory, terminal={"keep_block_width": True},
                    shell_integration={"mark_sources": "none"})
    ready(app)
    send(app, "clear; seq 1 6; printf 'KEEP_ME %s\\n' \"$(printf 'A%.0s' $(seq 70))\"")
    wait_text(app, "KEEP_ME")
    wait_prompt(app)
    app.resize(450, 600)
    wait_narrowed(app)
    x0, y0, x1, y1 = app.cell_rect(0, kept_row(app), top=app.top())
    app.click((x0 + x1) // 2, (y0 + y1) // 2, repeat=3)
    app.snap("the held frame let go of a triple clicked kept line")
    set_clipboard("nothing copied yet")

    def copied_line() -> str | None:
        copied = get_clipboard()
        return copied if copied != "nothing copied yet" and copied.strip() else None

    app.key("ctrl+shift+c")
    copied = app.wait(copied_line, timeout=5, msg="the copied kept line")
    line = copied.strip()
    assert line.startswith("KEEP_ME ") and "\n" not in line, repr(copied[:120])
    assert line.count("A") == 70, repr(copied[:120])


# ---- a held frame with a program printing lines -----------------------------


@pytest.mark.parametrize("hold", [True, False], ids=["held", "live"])
def test_a_drag_holds_the_frame_while_lines_keep_coming(app_factory, hold):
    """selection.hold_frame with a program printing lines and keep_block_width on: with it on the
    frame under the drag stands still and catches up on release, with it off the new lines move
    it while the button is down"""
    app = combo_app(app_factory, selection={"hold_frame": hold},
                    terminal={"keep_block_width": True}, shell_integration={"mark_sources": "none"})
    ready(app)
    send(app, LOOP)
    wait_text(app, "tick_20", timeout=25)
    rows = app.layout_pty()[0]
    x0, y0 = app.cell_rect(1, 1, top=app.top())[:2]
    x1, y1 = app.cell_rect(30, rows - 3, top=app.top())[:2]
    app.mouse(x0, y0)
    app.button(1, True)
    app.mouse(x1, y1)
    time.sleep(0.4)
    first = grid_above_cursor(app)
    time.sleep(1.5)
    second = grid_above_cursor(app)
    if hold:
        assert np.array_equal(first, second), "the frame moved under the drag"
        app.snap("held: the ticks printed since the drag started are not shown yet")
        app.button(1, False)
        app.wait(lambda: not np.array_equal(grid_above_cursor(app), second),
                 msg="the frame caught up on release")
    else:
        assert not np.array_equal(first, second), "the output did not move the frame"
        app.snap("not held: the lines moved the frame under the drag")
        app.button(1, False)


def test_the_copied_selection_is_the_frame_that_was_shown(app_factory):
    """hold_frame with keep_block_width and a right click for the copy: the text that lands in
    the clipboard is the frame the drag started on, not the lines that kept arriving under it"""
    app = combo_app(app_factory, selection={"hold_frame": True},
                    terminal={"keep_block_width": True}, shell_integration={"mark_sources": "none"})
    ready(app)
    send(app, "i=0; while :; do i=$((i+1)); echo tick_$i; sleep 0.2; done")
    wait_text(app, "tick_15", timeout=25)
    rows = app.layout_pty()[0]
    x0, y0 = app.cell_rect(1, 1, top=app.top())[:2]
    x1, y1 = app.cell_rect(30, rows - 3, top=app.top())[:2]
    app.mouse(x0, y0)
    app.button(1, True)
    app.mouse(x1, y1)
    time.sleep(2.5)  # a dozen lines arrive while the frame is held
    app.button(3, True)
    app.button(3, False)
    copied = clipboard_wait(app, "tick_", "the clipboard")
    app.snap("the held frame copied by a right click, the drag still on")
    live = ticks_in(text(app))
    app.button(1, False)
    ticks = ticks_in(copied)
    assert len(ticks) >= 5, repr(copied)
    assert ticks == list(range(ticks[0], ticks[0] + len(ticks))), ticks
    assert live and max(live) >= max(ticks) + 5, (max(live), max(ticks))


# ---- a link on a kept line that scrolled away -------------------------------


def link_opener(tmp_path: Path) -> tuple[Path, Path]:
    """a stand in for links.open_with that writes what it was given into a file"""
    script = tmp_path / "open.sh"
    out = tmp_path / "opened"
    script.write_text(f'#!/bin/sh\nprintf "%s" "$1" > "{out}.tmp"; mv "{out}.tmp" "{out}"\n')
    script.chmod(0o755)
    return script, out


def peek_pixels(app: App) -> int:
    """pixels of a peek box, in the tab bar color below the tab bar"""
    return int(near(app.shot()[app.top() + 4 :], C["tab_bar_background"]).sum())


def editor_line(app: App) -> str:
    """the line the editor's cursor is on, written out by the editor itself"""
    out = app.file("cur")
    out.unlink(missing_ok=True)
    app.key("Escape")
    app.type(f":.w! {out}")
    app.key("Return")
    return app.wait_file("cur").strip()


def test_a_link_on_a_kept_line_that_scrolled_away_peeks_and_opens(app_factory, tmp_path):
    """links.open_in_running_editor and links.peek with keep_block_width: a file:line on a wide
    line that scrolled away before the window narrowed is still drawn on one row, cut at the
    pane edge, and both the peek and the ctrl click find it there and open the editor at line 3"""
    script, opened = link_opener(tmp_path)
    app = combo_app(app_factory, terminal={"keep_block_width": True},
                    shell_integration={"mark_sources": "none"},
                    links={"patterns": [{"regex": r"[\w./-]+\.txt:\d+(:\d+)?", "action": "open"}],
                           "open_with": str(script), "open_in_running_editor": True, "peek": True})
    ready(app)
    (app.home / "f.txt").write_text("".join(f"line {n}\n" for n in range(1, 21)))
    os.makedirs(app.file("bin"), exist_ok=True)
    # vim.tiny under the name vim, the name kuterm knows it by
    os.symlink("/usr/bin/vim.tiny", app.file("bin/vim"))
    # the editor in tab 1, where the link is not; vim brings its own tab icon
    send(app, "cd ~ && $T2_TMP/bin/vim -u NONE -N")
    app.wait(lambda: app.title() != "1 D", msg="vim running in tab 1")
    app.key("ctrl+shift+t")
    app.wait_title("2 D")
    # the link printed at 900px, then pushed off the screen before the window narrows
    send(app, "clear; printf 'f.txt:3 %s\\n' \"$(printf 'W%.0s' $(seq 70))\"; seq 1 40; echo done_40")
    wait_text(app, "done_40")
    # the block has to end before the resize, else there is nothing to copy aside
    wait_prompt(app)
    app.wait_still()
    app.resize(450, 600)
    app.wait_still()
    # scrolled back: the kept line is back on the screen, one drawn row, the rows its reflow
    # would have taken left empty
    app.wheel(225, 300, up=True, clicks=30)
    app.wait(lambda: any(run >= 6 * app.cell_width() for run in row_runs(app)),
             msg="the kept line scrolled back into view")
    app.wait_still()
    row = kept_row(app)
    runs = row_runs(app)
    assert row + 1 >= len(runs) or runs[row + 1] < 6 * app.cell_width(), \
        "the line reflowed under the kept row"
    app.snap("the wide line with f.txt:3 scrolled back, kept on one row after the window narrowed")
    # resting on the link shows the file around line 3
    x0, y0, x1, y1 = app.cell_rect(1, row, top=app.top())
    app.mouse((x0 + x1) // 2, (y0 + y1) // 2)
    app.wait(lambda: peek_pixels(app) > 1000, msg="the peek of the kept link")
    app.snap("the peek of f.txt at line 3 from the kept line")
    # the ctrl click opens it in the vim running in the other tab, at that line
    with app.hold("ctrl"):
        app.mouse((x0 + x1) // 2, (y0 + y1) // 2)
        time.sleep(0.2)
        app.button(1, True)
        app.button(1, False)
    # vim brings its own tab icon, so only the tab number says where the focus went
    app.wait_title(lambda title: title.startswith("1 "))
    app.wait(lambda: "line 3" in text(app), msg="the file opened in the editor at line 3")
    assert editor_line(app) == "line 3"
    assert not opened.exists(), "the link fell back to links.open_with"
    app.snap("the kept line's link opened f.txt at line 3 in the editor of the other tab")


# ---- every notification source at once --------------------------------------


def notification_bands(app: App, img=None) -> list:
    """bboxes of the notices in the bottom right corner, oldest on top"""
    img = app.shot() if img is None else img
    h, w = img.shape[:2]
    mask = near(img[h // 2 :, w // 2 :], C["tab_bar_background"])
    bands = []
    for y in np.flatnonzero(mask.any(axis=1)):
        if bands and y - bands[-1][1] <= 2:
            bands[-1][1] = int(y)
        else:
            bands.append([int(y), int(y)])
    boxes = []
    for y0, y1 in bands:
        columns = np.flatnonzero(mask[y0 : y1 + 1].any(axis=0))
        if len(columns):
            boxes.append((int(columns[0]) + w // 2, y0 + h // 2,
                          int(columns[-1]) + w // 2, y1 + h // 2))
    return boxes


def test_a_question_and_an_agent_notify_once_each(app_factory):
    """notifications.program_waiting, question_patterns of its own, agent_waiting and
    from_programs always in one app: a question asked in the tab in front is told once it is
    left, an agent going idle in the other tab is marked, each wait is told once, nothing is
    told twice, and the notice of a wait takes its tab"""
    folder = agent_path()
    app = combo_app(app_factory, env={"PATH": f"{folder}:{os.environ['PATH']}"},
                    tab_icons={"interpreters": ["python3"],
                               "groups": [{"icon": "A", "agents": True, "commands": ["claude"]}]},
                    window_title=["number", {"text": " "}, "icon"],
                    hide_bar_for_one_tab=False,
                    shell_integration={"notify_after": 0},
                    notifications={"timeout": 0, "from_programs": "always",
                                   "agent_waiting": True, "program_waiting": True,
                                   "actions": True, "question_patterns": ["continue\\?"]})
    ready(app)
    send(app, "claude")
    # the title shows the agent program, so the app saw it running before it stops being busy
    app.wait_title("1 A", timeout=15)
    app.key("ctrl+shift+t")
    app.wait_title("2 D")
    second = active_pane(app)
    ready(app, second)
    send(app, QUESTION, pane=second)
    # the question is asked in the tab in front, where it is seen and not told; only the agent's
    # own osc 9 notice is in the corner so far
    nothing_for(2, lambda: len(notification_bands(app)) != 1)
    app.key("ctrl+Tab")
    app.wait_title("1 A")
    # leaving the question behind tells it once
    app.wait(lambda: len(notification_bands(app)) == 2, msg="the question told once left behind")
    before = marked(app)
    app.file("agent_go").touch()
    # the agent goes idle: its tab joins the marked one, no notice is added for it
    app.wait(lambda: marked(app) > before, msg="the tab of the agent marked when it stopped",
             timeout=25)
    assert len(notification_bands(app)) == 2, "the agent wait was told a second time"
    nothing_for(3, lambda: len(notification_bands(app)) != 2)  # nothing is told twice
    assert marked(app) > 4, "the waiting tabs are marked"
    app.snap("the question told once it was left, the agent waiting in the marked tab beside it")
    # the newest notice is the lowest one, clicking it goes to the tab it is about
    bands = notification_bands(app)
    app.click((bands[-1][0] + bands[-1][2]) // 2, (bands[-1][1] + bands[-1][3]) // 2)
    app.wait_title("2 D", timeout=10)
    app.snap("the notice of the question clicked, its tab in front")


def test_a_waiting_agent_is_told_on_the_desktop(app_factory):
    """a watcher on the session bus sees the desktop notification of an agent that stopped being
    busy in a window that is not focused"""
    server = Notifications()
    folder = agent_path()
    try:
        app = combo_app(app_factory, env={"PATH": f"{folder}:{os.environ['PATH']}"},
                        tab_icons={"interpreters": ["python3"],
                                   "groups": [{"icon": "A", "agents": True, "commands": ["claude"]}]},
                        window_title=["number", {"text": " "}, "icon"],
                        hide_bar_for_one_tab=False,
                        notifications={"timeout": 0, "agent_waiting": True})
        ready(app)
        send(app, "claude")
        app.wait_title("1 A", timeout=15)
        app_factory({"theme": {"mode": "dark"}})  # another window takes the focus
        app.file("agent_go").touch()
        app.wait_file("agent_idle", timeout=15)
        shown = server.wait(lambda n: "the agent in tab 1 waits for you" in n["body"],
                            msg="the desktop notification of the agent", timeout=25)
        assert shown["app"] == "kuterm", shown
    finally:
        server.stop()


# ---- the password manager next to the queue and incognito tabs --------------


@pytest.mark.parametrize("enter", [True, False], ids=["enter", "no-enter"])
def test_fill_password_with_a_custom_get_and_a_queued_command(app_factory, enter):
    """password_manager with a list and a get command of its own, enter on or off, while a
    command waits in the queue: the queue only runs once the secret is in and the prompt is
    back, and with enter off that takes the return key"""
    app = combo_app(app_factory,
                    password_manager={"list": ["printf", "one\ntwo\n"],
                                      "get": ["printf", "s3cret!x\nuser: me\n"], "enter": enter},
                    terminal={"queue_stop_on_failure": True})
    ready(app)
    send(app, "printf 'READY\\n'; read -s x; echo \"len ${#x}\" > $T2_TMP/len")
    app.wait(lambda: "READY" in text(app), msg="the program asking for a secret")
    queue(app, "touch $T2_TMP/queued")
    app.palette("fill password")
    app.wait(lambda: picker(app), msg="the entries listed")
    app.type("one")
    app.key("Return")
    if not enter:
        nothing_for(3, app.file("len").exists)
        app.snap("the secret typed without enter, the program still waits and the queue with it")
        app.key("Return")
    app.wait_file("len", "len 8", timeout=15)
    app.wait(app.file("queued").exists, msg="the queue ran once the prompt was back", timeout=20)
    assert "user: me" not in text(app, scrollback=True), "only the first line of get is typed"


def test_fill_password_in_an_incognito_tab(app_factory):
    """a password prompt in an incognito tab: the lock the prompt puts on the tab shows the app
    saw it, fill_password still answers it, and the tab reads nowhere over remote control"""
    app = combo_app(app_factory, password_manager={"list": ["printf", "secret\n"],
                                                   "get": ["printf", "hunter2\n"], "enter": True})
    ready(app)
    app.palette("new incognito tab")
    app.wait_title(lambda title: title.startswith("2 "))
    pane = active_pane(app)
    # remote control cannot type into an incognito tab, the keyboard can
    app.type(SECRET_PROMPT)
    app.key("Return")
    app.wait(lambda: "\uf023" in app.title(), msg="the lock of the secret prompt on the tab")
    app.snap("the incognito tab asking for a secret, the lock on its tab")
    app.palette("fill password")
    app.wait(lambda: picker(app), msg="the entries listed")
    app.type("secret")
    app.key("Return")
    app.wait_file("locked", "len 7")
    done = msg(app, "get-text", "--pane", pane)
    assert done.returncode != 0 and "incognito" in done.stderr + done.stdout, done
    assert "hunter2" not in text(app, first_tab(app)), "the secret stayed in its own tab"
    app.snap("the secret reached the incognito tab, which still reads nowhere")


# ---- project groups with sleeping tabs and the session ----------------------


def test_project_groups_sleep_and_come_back_with_the_session(app_factory):
    """tab_groups.by_project with sleep_tabs_after_minutes and restore_session: a project tab
    falls asleep hidden with its chip in place, and after a restart its shell is back in the
    project folder under the same group color"""
    app = combo_app(app_factory, tab_groups={"by_project": True}, sleep_tabs_after_minutes=0.02,
                    restore_session="always", confirm_quit="never",
                    window_title=["number", {"text": " "}, "folder"],
                    terminal={"max_history_length": 0})
    listening(app)
    project(app, "proj-one")
    app.wait_title("1 ~")
    send(app, "cd $T2_TMP/proj-one")
    app.wait_title("1 proj-one")
    box = app.wait(lambda: chips(app) and chips(app)[0], msg="the chip of the project")
    colour = chip_colour(app, box)
    send(app, "seq 1 1200; echo slept_project")
    pane = first_tab(app)
    app.key("ctrl+shift+t")
    app.wait_title("2 proj-one")
    wait_asleep(app, pane)
    assert len(chips(app)) == 1 and chip_colour(app, chips(app)[0]) == colour
    app.snap("the project tab asleep behind the front one, its chip still in the bar")
    app.palette("quit")
    app.wait(lambda: not app.alive(), msg="the app quit")
    app.start()
    listening(app)
    # the restored session comes back with the tab it left in front, its folder and its group
    app.wait_title(lambda title: title.endswith("proj-one"))
    back = app.wait(lambda: chips(app) and chips(app)[0], msg="the project group made again")
    assert chip_colour(app, back) == colour
    send(app, "pwd > $T2_TMP/where")
    assert app.wait_file("where").strip().endswith("proj-one")
    app.snap("restored in its project folder, the chip back")


# ---- the status bar blocks next to the titles -------------------------------


def test_status_bar_units_clock_and_titles_together(app_factory):
    """status_bar with memory_unit and clock_format of its own and the new blocks in
    status_bar.left and right, window_title and tab_title at once, with the tab bar hidden and
    shown, and a reload switching the memory unit and the clock"""
    repo = Path(tempfile.mkdtemp(prefix="e2e-repo-"))
    subprocess.run(["git", "init", "-q", "-b", "main", str(repo)], check=True)
    app = combo_app(app_factory,
                    status_bar={"enable": True, "memory_unit": "kb", "clock_format": "%Y",
                                "left": ["git", "cpu_graph", "cpu", "memory", "network", "ports",
                                         "xoff", "stopped_jobs"],
                                "right": [{"text": "all tabs"}, "total_cpu", "total_memory",
                                          "total_network", "all_ports", "size", "clock"]},
                    window_title=["number", {"text": " "}, "memory", {"text": " "}, "size",
                                  {"text": " "}, "clock"],
                    tab_title=["number", {"text": " "}, "git", {"text": " "}, "venv"],
                    hide_bar_for_one_tab=True,
                    shell_integration={"mark_sources": "none"},
                    env={"VIRTUAL_ENV": "/tmp/fake-venv"})
    listening(app)
    app.mouse(450, 300)
    app.wait(lambda: "KB" in app.title(), msg=f"the memory in kb: {app.title()!r}")
    send(app, f"cd {repo} && sleep 8")
    app.wait(lambda: "main" in tab_title(app, 1) and "fake-venv" in tab_title(app, 1),
             msg=f"branch and venv in the tab title: {tab_title(app, 1)!r}", timeout=15)
    assert "2026" in app.title(), app.title()
    assert status_bar_shown(app) and status_text(app), "the status bar is drawn with its blocks"
    assert app.pty_size() == app.layout_pty(900, 600), "one tab, no tab bar, the status bar"
    app.snap("the status bar with the new blocks, year and units in the title, no tab bar")
    app.key("ctrl+shift+t")
    app.wait(lambda: app.pty_size() == app.layout_pty(900, 600, tabs=2), msg="the tab bar came back")
    app.snap("a second tab brings the tab bar back above the status bar")
    app.write("settings.jsonc", {**app.settings,
                                 "status_bar": {**app.settings["status_bar"],
                                                "memory_unit": "gb", "clock_format": "%Y-%m-%d"}})
    app.wait(lambda: "GB" in app.title() and "-" in app.title() and "KB" not in app.title(),
             msg=f"the reloaded units: {app.title()!r}")
    app.snap("after the reload the memory is in gb and the clock shows the date")


# ---- old settings next to the new ones --------------------------------------


def test_clipboard_ask_with_paste_check_invisible(app_factory):
    """terminal.clipboard.write ask next to paste_check_invisible in one app: the paste asking
    about a hidden character reaches the shell whole once answered, then a program asking to copy
    is answered on its own"""
    app = combo_app(app_factory, terminal={"clipboard": {"write": "ask"}, "paste_protection": False,
                                           "paste_check_invisible": True})
    ready(app)
    send(app, "IFS= read -r l; printf '%s' \"$l\" > $T2_TMP/got")
    set_clipboard("ls ‮txt.exe")
    app.wait(lambda: get_clipboard() == "ls ‮txt.exe", msg="clipboard owner")
    app.key("ctrl+shift+v")
    app.wait(lambda: near(app.shot()[60:300, 250:650], C["tab_bar_background"]).sum() > 5000,
             msg="the paste dialog")
    app.wait(lambda: near(app.shot()[60:300, 150:450], C["danger_button"]).sum() > 200,
             msg="the hidden character pointed out")
    app.snap("the paste asks because of the hidden character")
    app.key("y")
    app.key("Return")
    assert "‮" in app.wait_file("got"), repr(app.file("got").read_bytes())
    # the program asks to copy on its own: denied it copies nothing, asked again it may
    import base64
    data = base64.b64encode(b"from the program").decode()
    send(app, f"printf '\\033]52;c;{data}\\007'")
    buttons = wait_buttons(app, C, 2)
    app.snap("the program asks to copy while the paste is done")
    app.click(*buttons[1])
    nothing_for(2, lambda: get_clipboard() == "from the program")
    send(app, f"printf '\\033]52;c;{data}\\007'")
    wait_buttons(app, C, 2)
    app.click(*notification_buttons(app, C)[0])
    app.wait(lambda: get_clipboard() == "from the program", msg="allowed copies")


def test_visual_bell_beside_an_attention_mark(app_factory):
    """bell.visual with program_waiting: a bell ringing in the front tab flashes the terminal
    while the tab behind stays marked, and the flash leaving does not take the mark with it"""
    app = combo_app(app_factory,
                    bell={"visual": {"enable": True, "color": "#ff00ff", "duration": 1500,
                                     "target": "terminal"}},
                    notifications={"timeout": 0, "question_patterns": ["continue\\?"]})
    ready(app)
    send(app, QUESTION)
    time.sleep(0.5)
    app.key("ctrl+shift+t")
    app.wait_title("2 D")
    app.wait(lambda: marked(app) > 4, msg="the mark of the question behind", timeout=20)

    def flashing() -> bool:
        """the middle of the terminal tinted by the bell"""
        return near(app.shot()[250:350, 400:500], (255, 0, 255), 60).mean() > 0.5

    send(app, "printf '\\a'")
    app.wait(flashing, msg="the bell flash")
    assert marked(app) > 4, "the flash took the attention mark with it"
    app.snap("the bell ringing over a window whose first tab waits for an answer")
    app.wait(lambda: not flashing(), msg="the flash faded", timeout=6)
    assert marked(app) > 4, "the mark went away with the flash"


def test_decorations_none_with_click_moves_cursor_and_the_overview(app_factory):
    """window.decorations none with shell_integration.click_moves_cursor and the tab overview in
    one app: the grid sits under the bar the frame-less window always shows, a click in the
    command line moves the shell's cursor there and the overview opens over two tabs"""
    app = combo_app(app_factory, window={"decorations": "none"}, hide_bar_for_one_tab=True,
                    shell_integration={"click_moves_cursor": True, "mark_sources": "none"},
                    keybindings=KEYS)
    ready(app)
    app.mouse(450, 300)
    assert app.pty_size() == app.layout_pty(900, 600), "the tab bar is the only bar above the grid"
    send(app, "PS1='$ '; clear")

    def ink_end(row: int) -> int:
        x0, y0, _, y1 = app.cell_rect(0, row, top=app.top())
        band = app.shot()[y0:y1, x0:]
        box = ink_bbox(band, dominant(band))
        return 0 if box is None else x0 + box[2]

    app.wait(lambda: 0 < ink_end(0) < app.cell_rect(4, 0)[0], msg="the short prompt alone on row 0")
    app.type("echo hello world>$T2_TMP/out")
    app.wait(lambda: ink_end(0) > app.cell_rect(30, 0)[0], msg="the command line typed")
    x0, y0, x1, y1 = app.cell_rect(7, 0, top=app.top())
    app.click((x0 + x1) // 2, (y0 + y1) // 2)
    app.type("X")
    app.snap("X typed where the command line was clicked")
    app.key("Return")
    assert app.wait_file("out").strip() == "Xhello world"
    app.key("ctrl+shift+t")
    app.wait_title("2 D")
    app.key("ctrl+shift+o")
    app.wait(lambda: near(app.shot()[6:46, 300:840], C["tab_active_background"], 3).mean() > 0.5,
             msg="the overview over both tabs")
    app.snap("the overview of a frame-less window, the tab bar still above the grid")
    app.key("Escape")
    app.wait(lambda: near(app.shot()[6:46, 300:840], C["tab_active_background"], 3).mean() <= 0.5,
             msg="the overview closed")
    assert app.pty_size() == app.layout_pty(900, 600, tabs=2)


# ---- the completion popup next to the gutter, xoff and the keymap -----------


def test_the_popup_rows_its_trigger_and_the_marked_shell(app_factory, tmp_path):
    """autocomplete with max_items 3 and trigger key next to mark_sources all, with ctrl+s
    pausing the output under the open popup: the popup waits for the complete key, shows fewer
    rows than the default eight, survives the pause, and typed commands still get their marks"""
    tall = zsh_app(app_factory, tmp_path / "tall", shell_integration={"mark_sources": "none"})
    # the line is still free here: later it holds "ls " and a sent command would run as `ls ...`
    plain_cols = columns_seen(tall, "plain")
    tall.type("ls ")
    big = tall.wait(lambda: popup(tall), msg="the default popup")
    tall.snap("the default eight rows of the same ten files")
    # one window at a time: on wayland another floating window of the test sits on the same
    # rect, and its open popup would be the pixels app.shot() reads
    tall.close()
    app = zsh_app(app_factory, tmp_path / "small", autocomplete={"trigger": "key", "max_items": 3},
                  window_title=["number", {"text": " "}, "xoff"],
                  shell_integration={"mark_sources": "all"}, terminal={"send_xoff": True})
    app.type("ls ")
    nothing_for(1, lambda: popup(app))
    app.key("ctrl+space")
    small = app.wait(lambda: popup(app), msg="the popup on the complete key")
    assert 1 < popup_rows(app) <= 4, popup_rows(app)
    assert small[3] - small[1] < big[3] - big[1], (small, big)
    app.snap("three rows at most, opened by the complete key only")
    app.key("ctrl+s")
    app.wait(lambda: "paused (ctrl+q)" in app.title(), msg=f"the pause in the title: {app.title()!r}")
    assert popup(app), "the popup went away with the pause"
    app.snap("output paused under the open popup")
    app.key("ctrl+q")
    app.wait(lambda: "paused" not in app.title(), msg=f"the pause over: {app.title()!r}")
    assert popup(app), "the popup went away with the resume"
    app.key("Escape")
    app.wait(lambda: popup(app) is None, msg="the popup dismissed")
    app.type("file_0")
    app.key("ctrl+space")
    app.wait(lambda: popup(app), msg="the popup for the file")
    app.key("Tab")
    app.wait(lambda: line_cells(app) >= len("$ ls file_0.txt"), msg="the completion in the line")
    app.wait(lambda: popup(app) is None, msg="the popup closed on the accepted candidate")
    # the marked shell shifts the grid two cells right, so the printed file is checked by what
    # the shell wrote, not by which cells the text lands on
    app.type(" > $T2_TMP/listed")
    app.key("Return")
    assert app.wait_file("listed", timeout=15).strip() == "file_0.txt"
    assert columns_seen(app, "marked") == plain_cols - 2, "one marked shell, one not"
    app.snap("completed with tab, run, and two cells taken by the source marks")


def test_show_descriptions_and_a_reload_of_the_popup(app_factory, tmp_path):
    """autocomplete.show_descriptions on and off across a config reload: the list of a command
    with a spec is narrower without the descriptions, and enable false leaves a new shell of the
    same profile without a popup"""
    app = zsh_app(app_factory, tmp_path, keybindings=KEYS,
                  autocomplete={"show_descriptions": True, "trigger": "key"})
    app.type("git ")
    app.key("ctrl+space")
    wide = app.wait(lambda: popup(app), msg="the popup with descriptions")
    app.snap("git subcommands with what each does")
    app.key("Escape")
    app.write("settings.jsonc", {**app.settings,
                                 "autocomplete": {**app.settings["autocomplete"],
                                                  "show_descriptions": False}})
    app.key("ctrl+space")
    narrow = app.wait(lambda: (box := popup(app)) and box != wide and box,
                      msg="the popup after the reload")
    assert narrow[2] - narrow[0] < wide[2] - wide[0], (narrow, wide)
    app.snap("the same subcommands without their descriptions")
    app.key("Escape")
    app.write("settings.jsonc", {**app.settings,
                                 "autocomplete": {**app.settings["autocomplete"], "enable": False}})
    app.key("ctrl+shift+r")  # a new shell of the same profile: a change is read by new ones
    app.wait(lambda: len(pane_ids(app)) == 2, msg="the split")
    time.sleep(1)
    app.type("ls ")
    app.key("ctrl+space")
    nothing_for(1.5, lambda: popup(app))
    app.snap("a shell started after enable was turned off shows no popup at all")


def test_a_completion_key_is_held_by_the_popup(app_factory, tmp_path):
    """completion_dismiss on ctrl-shift-u: while the popup shows, the key only hides the popup
    and the shell's line stays exactly as it was"""
    app = zsh_app(app_factory, tmp_path, keybindings={"completion_dismiss": "ctrl-shift-u"})
    app.type("ls file")
    app.wait(lambda: popup(app), msg="the popup")
    typed = line_cells(app)
    app.key("ctrl+shift+u")
    app.wait(lambda: popup(app) is None, msg="the key dismissed the popup")
    time.sleep(0.5)
    assert line_cells(app) == typed, "the dismiss key also reached the shell"
    app.snap("the popup dismissed by ctrl-shift-u, the shell's line untouched")


def test_the_dismiss_key_is_the_shells_own_without_the_popup(app_factory, tmp_path):
    """with the popup closed the key is not the dismiss key anymore but zsh's own: ctrl-shift-u
    is its kill-line, so the line typed before it is gone. the popup only opens on its key, so
    typing the line does not show it"""
    app = zsh_app(app_factory, tmp_path, autocomplete={"trigger": "key"},
                  keybindings={"completion_dismiss": "ctrl-shift-u"})
    app.type("ls file")
    app.key("ctrl+shift+u")
    # a marker typed right after the key tells the two cases apart: if zsh's kill-line ran, the
    # marker is the whole command line, otherwise it lands next to the text that stayed
    app.type("echo reached > $T2_TMP/killed")
    app.key("Return")
    assert app.wait_file("killed", timeout=15).strip() == "reached", "the key did not empty the line"
    app.snap("with no popup the same key cleared the shell's line")


def test_a_completion_keymap_entry_holds_the_key_while_the_popup_shows(app_factory, tmp_path):
    """a keymap entry with when "completion" runs a command while the popup shows, so the key
    never reaches zsh there: the typed line runs and the popup goes with it"""
    app = zsh_app(app_factory, tmp_path, autocomplete={"trigger": "key"},
                  keymap=[{"keys": "ctrl-shift-j", "when": "completion",
                           "action": {"run": "macro it"}}],
                  commands=[{"name": "macro it",
                             "actions": [{"type": "; touch $T2_TMP/macro_hit ; echo\n"}]}])
    app.type("ls file")
    app.key("ctrl+space")
    app.wait(lambda: popup(app), msg="the popup")
    app.key("ctrl+shift+j")
    app.wait_file("macro_hit", timeout=15)
    app.wait(lambda: popup(app) is None, msg="the typed line ran, so the popup is gone")
    app.snap("the completion keymap entry ran the command instead of the shell")


def test_the_keymap_key_is_the_shells_own_without_the_popup(app_factory, tmp_path):
    """the keymap entry is only for the popup: with none showing, the same key is zsh's own line
    feed and runs the line it was pressed on"""
    app = zsh_app(app_factory, tmp_path, autocomplete={"trigger": "key"},
                  keymap=[{"keys": "ctrl-shift-j", "when": "completion",
                           "action": {"run": "macro it"}}],
                  commands=[{"name": "macro it",
                             "actions": [{"type": "; touch $T2_TMP/macro_hit ; echo\n"}]}])
    app.type("echo still_here > $T2_TMP/lf_ran")
    app.key("ctrl+shift+j")
    assert app.wait_file("lf_ran", timeout=15).strip() == "still_here"
    app.snap("with no popup the same key reached zsh as a line feed")


# ---- every new setting at once ----------------------------------------------


def test_one_app_with_every_new_setting_away_from_its_default(app_factory):
    """every new setting at a non default value in one app: it starts, the grid is where the bars
    leave it, a typed command runs and is marked, a queued one waits in the tab title, the
    overview opens, and a reload that takes both bars away grows the grid without breaking the
    typing"""
    app = combo_app(app_factory,
                    sleep_tabs_after_minutes=1,
                    status_bar={"enable": True, "memory_unit": "gb", "clock_format": "%Y",
                                "left": ["git", "cpu_graph", "cpu", "memory", "network", "ports",
                                         "xoff", "stopped_jobs"],
                                "right": [{"text": "all tabs"}, "total_cpu", "total_memory",
                                          "total_network", "all_ports", "size", "clock"]},
                    window_title=["number", {"text": " "}, "clock", {"text": " "}, "size",
                                  {"text": " "}, "memory"],
                    tab_title=["number", {"text": " "}, "queue", {"text": " "}, "stopped_jobs"],
                    notifications={"timeout": 0, "from_programs": "always", "agent_waiting": True,
                                   "program_waiting": True, "actions": True,
                                   "question_patterns": ["continue\\?"]},
                    autocomplete={"enable": False, "trigger": "key", "max_items": 3,
                                  "show_descriptions": False},
                    tab_groups={"by_project": True},
                    password_manager={"list": ["printf", "one\n"], "get": ["printf", "s3cret\n"],
                                      "enter": True},
                    selection={"hold_frame": False},
                    links={"open_in_running_editor": False, "peek": False},
                    shell_integration={"inject": True, "click_moves_cursor": False,
                                       "mark_sources": "all", "notify_after": 0},
                    terminal={"paste_protection": False, "paste_check_invisible": False,
                              "queue_stop_on_failure": False, "send_xoff": False,
                              "keep_block_width": True, "max_history_length": 500},
                    bell={"visual": {"enable": True, "color": "#ff00ff", "duration": 300,
                                     "target": "tab"}},
                    keymap=[{"keys": "ctrl-shift-j", "when": "completion",
                             "action": {"run": "macro it"}}],
                    commands=[{"name": "macro it", "actions": [{"type": "echo macro\n"}]}],
                    keybindings=dict(KEYS), hide_bar_for_one_tab=False)
    listening(app)
    app.mouse(450, 300)
    assert app.pty_size() == app.layout_pty(900, 600), "tab bar and status bar over the grid"
    assert app.title().startswith("1 ") and "GB" in app.title(), app.title()
    assert status_bar_shown(app) and status_text(app), "the status bar is drawn"
    app.snap("every new setting at once: titles and the status bar with its blocks")
    send(app, "echo typed_ok > $T2_TMP/typed")
    assert app.wait_file("typed").strip() == "typed_ok"
    assert app.pty_size()[1] == app.layout_pty(900, 600)[1] - 2, "the mark gutter took two cells"
    send(app, "sleep 8")
    queue(app, "touch $T2_TMP/queued")
    app.wait(lambda: "⧗" in tab_title(app, 1), msg="the queue block in the tab title", timeout=10)
    app.snap("a queued command in the tab title of the all-settings app")
    app.wait(app.file("queued").exists, msg="the queue ran", timeout=25)
    app.key("ctrl+shift+o")
    app.wait(lambda: near(app.shot()[6:46, 300:840], C["tab_active_background"], 3).mean() > 0.5,
             msg="the overview")
    app.key("Escape")
    # the reload takes both bars away: the grid grows by the bar heights, the marks still take
    # their two cells, and typing and the titles still work
    app.write("settings.jsonc", {**app.settings,
                                 "status_bar": {**app.settings["status_bar"], "enable": False},
                                 "hide_bar_for_one_tab": True})
    rows, cols = app.expected_pty(900, 600)
    app.wait(lambda: app.pty_size() == (rows, cols - 2), msg="the grid without either bar")
    assert not status_bar_shown(app), "the status bar stayed after the reload"
    send(app, "echo again > $T2_TMP/typed2")
    assert app.wait_file("typed2").strip() == "again"
    assert app.title().startswith("1 "), app.title()
    app.snap("after the reload: both bars gone, the grid grown, typing and titles fine")



