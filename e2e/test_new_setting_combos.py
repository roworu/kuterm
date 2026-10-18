"""the new settings used together, not one at a time: the command queue, a stopped job, a held
frame, the completion popup
with its keys, its keymap and the shell, a question and an agent notifying together, and one app
started with every new setting away from its default"""

import os
import subprocess
import tempfile
import time
from pathlib import Path

import numpy as np

from harness import (BIN, App, Notifications, as_rgb, close_to, count, dominant,
                     get_clipboard, ink, ink_bbox, mask_bbox, near, set_clipboard, unique_theme)
from kit import (blocks, executable, integrated, listening, msg, nothing_for, notification_buttons,
                 ready, send, socket_of, tabs, text, wait_buttons, wait_text)

FEATURE = "new settings together"

THEME = {**unique_theme(0xc1), "search_match": "#ffd400", "search_match_active": "#ff4000"}
C = as_rgb(THEME)

# keys for the features in this file, a test passes the ones it uses
KEYS = {"split_right": "ctrl-shift-r"}

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


def test_without_injection_the_queue_waits_until_a_reload(app_factory):
    """shell_integration.inject off: no blocks, and the queue cannot know the prompt came back
    so it waits; a new tab after a reload has injection and runs
    its own queue while the old shell keeps waiting"""
    app = combo_app(app_factory, shell_integration={"inject": False},
                    tab_title=["number", {"text": " "}, "queue"])
    listening(app)
    app.mouse(450, 300)
    assert app.pty_size() == app.layout_pty(900, 600), "the grid fills the window"
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
                                 "shell_integration": {"inject": True}})
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


def test_decorations_none_with_click_moves_cursor(app_factory):
    """window.decorations none with shell_integration.click_moves_cursor in one app: the grid
    sits under the bar the frame-less window always shows, a click in the command line moves
    the shell's cursor there, and a second tab keeps the grid under the bar"""
    app = combo_app(app_factory, window={"decorations": "none"}, hide_bar_for_one_tab=True,
                    shell_integration={"click_moves_cursor": True},
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
    assert app.pty_size() == app.layout_pty(900, 600, tabs=2)


# ---- the completion popup, xoff and the keymap ------------------------------


def test_the_popup_rows_and_its_trigger(app_factory, tmp_path):
    """autocomplete with max_items 3 and trigger key, with ctrl+s pausing the output under the
    open popup: the popup waits for the complete key, shows fewer rows than the default eight,
    survives the pause, and an accepted candidate runs"""
    tall = zsh_app(app_factory, tmp_path / "tall")
    tall.type("ls ")
    big = tall.wait(lambda: popup(tall), msg="the default popup")
    tall.snap("the default eight rows of the same ten files")
    # one window at a time: on wayland another floating window of the test sits on the same
    # rect, and its open popup would be the pixels app.shot() reads
    tall.close()
    app = zsh_app(app_factory, tmp_path / "small", autocomplete={"trigger": "key", "max_items": 3},
                  window_title=["number", {"text": " "}, "xoff"], terminal={"send_xoff": True})
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
    app.type(" > $T2_TMP/listed")
    app.key("Return")
    assert app.wait_file("listed", timeout=15).strip() == "file_0.txt"
    app.snap("completed with tab and run")


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
    leave it, a typed command runs, a queued one waits in the tab title, and a reload that takes both bars away grows the grid without breaking the
    typing"""
    app = combo_app(app_factory,
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
                    selection={"hold_frame": False},
                    links={"open_in_running_editor": False, "peek": False},
                    shell_integration={"inject": True, "click_moves_cursor": False,
                                       "notify_after": 0},
                    terminal={"paste_protection": False, "paste_check_invisible": False,
                              "queue_stop_on_failure": False, "send_xoff": False,
                              "max_history_length": 500},
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
    send(app, "sleep 8")
    queue(app, "touch $T2_TMP/queued")
    app.wait(lambda: "⧗" in tab_title(app, 1), msg="the queue block in the tab title", timeout=10)
    app.snap("a queued command in the tab title of the all-settings app")
    app.wait(app.file("queued").exists, msg="the queue ran", timeout=25)
    # the reload takes both bars away: the grid grows by the bar heights, and typing and the
    # titles still work
    app.write("settings.jsonc", {**app.settings,
                                 "status_bar": {**app.settings["status_bar"], "enable": False},
                                 "hide_bar_for_one_tab": True})
    rows, cols = app.expected_pty(900, 600)
    app.wait(lambda: app.pty_size() == (rows, cols), msg="the grid without either bar")
    assert not status_bar_shown(app), "the status bar stayed after the reload"
    send(app, "echo again > $T2_TMP/typed2")
    assert app.wait_file("typed2").strip() == "again"
    assert app.title().startswith("1 "), app.title()
    app.snap("after the reload: both bars gone, the grid grown, typing and titles fine")


