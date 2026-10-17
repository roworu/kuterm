"""plugins: programs listed in settings that kuterm starts and talks to over stdin and stdout
with json-rpc, framed like lsp. a plugin adds palette commands, link patterns, tab title blocks
and panels, asks kuterm for things with the requests of `kuterm msg`, gets the events of the
bus while it may see panes, and is asked once before it first runs"""

import json
import re
import textwrap
from pathlib import Path

import numpy as np
import pytest

from harness import App, SRC, Notifications, as_rgb, ink, mask_bbox, median, near, unique_theme
from kit import (integrated, msg, nothing_for, notification_box, ready, send, tabs, wait_block,
                 wait_buttons, wait_text)

FEATURE = "plugins"

THEME = unique_theme(0x6d)
C = as_rgb(THEME)

# the parts every e2e plugin is built from: it logs what it gets and sends, so the tests read
# what a plugin saw out of a file under $T2_TMP instead of looking inside the app
PRELUDE = r'''"""e2e plugin: json-rpc over stdio, framed like lsp.

answers `initialize` with CAPABILITIES, logs every message it gets and sends as one json line
in $T2_TMP/<KUTERM_PLUGIN>.jsonl, then runs on_message for the rest.
"""

import json
import os
import sys

TMP = os.environ.get("T2_TMP", "/tmp")
NAME = os.environ.get("KUTERM_PLUGIN", "plugin")
CALLS = os.path.join(TMP, NAME + ".jsonl")


def log(entry):
    with open(CALLS, "a") as handle:
        handle.write(json.dumps(entry) + "\n")


def send(message):
    body = json.dumps(message).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()


def answer(message, result):
    log({"sent": {"id": message["id"], "result": result}})
    send({"jsonrpc": "2.0", "id": message["id"], "result": result})


def notify(method, params):
    log({"sent": {"method": method, "params": params}})
    send({"jsonrpc": "2.0", "method": method, "params": params})


next_id = 100


def request(method, params):
    """ask kuterm something, the answer comes back as a message with this id"""
    global next_id
    next_id += 1
    log({"sent": {"id": next_id, "method": method, "params": params}})
    send({"jsonrpc": "2.0", "id": next_id, "method": method, "params": params})
    return next_id


def ask(cmd, **params):
    """one request of the remote control socket, answered for the plugin"""
    return request("kuterm/request", {"cmd": cmd, **params})


def read():
    """one message from kuterm, none once it closed the pipe"""
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        name, _, value = line.decode().partition(":")
        if name.lower() == "content-length":
            length = int(value)
    return json.loads(sys.stdin.buffer.read(length))
'''

MAIN = r'''
while True:
    message = read()
    if message is None:
        break
    log({"got": message})
    if message.get("method") == "initialize":
        answer(message, CAPABILITIES)
        on_initialize()
        continue
    on_message(message)
'''

SAYS_HELLO = {"commands": [{"name": "plugin says hello", "category": "e2e"}]}

BODY_HELLO = '''
if message.get("method") == "command":
    notify("notify", {"text": "hello from the plugin, pane " + str(message["params"]["pane"])})
'''

BODY_TYPE = '''
if message.get("method") == "command":
    ask("send_text", pane=str(message["params"]["pane"]), text="echo ran >> $T2_TMP/typed_by_plugin\\n")
'''

BODY_READ = '''
if message.get("method") == "command":
    pane = str(message["params"]["pane"])
    ask("get_text", pane=pane)
    ask("query", pane=pane, all=True)
    ask("send_text", pane=pane, text="touch $T2_TMP/typed_by_plugin\\n")
    ask("new_tab")
'''

BODY_READ_PANE = '''
if message.get("method") == "command":
    ask("get_text", pane=str(message["params"]["pane"]))
'''


def indent(code: str) -> str:
    """the body of one of the plugin's functions, indented to fit it"""
    return textwrap.indent(textwrap.dedent(code).strip() or "pass", "    ") + "\n"


def plugin_source(caps: dict | None = None, body: str = "", after: str = "",
                  setup: str = "") -> str:
    """python source of an e2e plugin: `setup` runs at the top, `caps` is what the plugin
    answers `initialize` with, `after` runs right after that, `body` gets every other message"""
    return (PRELUDE + "\nCAPABILITIES = " + json.dumps(caps or {}) + "\n\n"
            + textwrap.dedent(setup).lstrip("\n") + "\n"
            + "\ndef on_initialize():\n" + indent(after)
            + "\ndef on_message(message):\n" + indent(body)
            + MAIN)


def write_plugin(folder: Path, name: str, caps: dict | None = None, body: str = "",
                 after: str = "", setup: str = "") -> Path:
    """write a plugin script into the test's own folder and return it"""
    path = folder / f"{name}.py"
    path.write_text(plugin_source(caps, body, after, setup))
    return path


def entry(name: str, script: Path, permissions: dict | None = None) -> dict:
    """one "plugins" settings entry running the script with python3"""
    return {"name": name, "command": ["python3", str(script)], "permissions": permissions or {}}


def kuterm_version() -> str:
    """the version kuterm reports in `initialize`, from its Cargo.toml"""
    return re.search(r'(?m)^version = "([^"]+)"', (SRC / "Cargo.toml").read_text()).group(1)


def start(app_factory, plugins: list, **settings) -> App:
    """an app with remote control, a bash profile and these plugin entries, nothing answered"""
    app = app_factory(integrated(plugins=plugins, **settings), themes={"dark": THEME})
    # the tab bar, to take the pointer off the corner the question shows in: a test answering
    # the question leaves the pointer there, and a click is what settles the hover of a window
    # that opens under a pointer already in place
    app.click(30, 18)
    app.mouse(450, 300)
    return app


def allow(app: App, timeout: float = 30) -> None:
    """answer the question kuterm asks before a plugin first runs: the left button allows it"""
    app.click(*wait_buttons(app, C, 2, timeout=timeout)[0])


def save(app: App, **changes) -> None:
    """write settings.jsonc again with a change, which reloads the config and its plugins"""
    app.write("settings.jsonc", {**app.settings, **changes})


def entries(app: App, name: str) -> list[dict]:
    """what the plugin logged, oldest first, one {"got": ...} or {"sent": ...} per message"""
    path = app.file(f"{name}.jsonl")
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


def seen(app: App, name: str, method: str, direction: str = "got") -> list[dict]:
    """messages of this method the plugin logged going that way"""
    return [entry[direction] for entry in entries(app, name)
            if direction in entry and entry[direction].get("method") == method]


def wait_seen(app: App, name: str, method: str, direction: str = "got", timeout: float = 20) -> dict:
    """the first message of this method the plugin logged, waiting until it comes"""
    return app.wait(lambda: (found := seen(app, name, method, direction)) and found[0],
                    msg=f"{direction} {method!r} in {name}.jsonl", timeout=timeout)


def wait_answer(app: App, name: str, check, msg: str, timeout: float = 20):
    """the result of the plugin's request whose answer `check` accepts"""
    def found():
        for message in (entry.get("got", {}) for entry in entries(app, name)):
            result = message.get("result")
            if result is not None and check(result):
                return result
        return None

    return app.wait(found, msg=msg, timeout=timeout)


def wait_error(app: App, name: str, text: str, timeout: float = 20) -> dict:
    """the json-rpc error kuterm answered the plugin's request with, naming what was refused"""
    def found():
        for message in (entry.get("got", {}) for entry in entries(app, name)):
            error = message.get("error")
            if error is not None and text in json.dumps(error):
                return error
        return None

    return app.wait(found, msg=f"an error naming {text!r}", timeout=timeout)


def wait_refusal(app: App, name: str, text: str, timeout: float = 20) -> str:
    """the failed reply of a request kuterm ran, naming why"""
    def found():
        for message in (entry.get("got", {}) for entry in entries(app, name)):
            result = message.get("result") or {}
            if result.get("ok") is False and text in (result.get("error") or ""):
                return result["error"]
        return None

    return app.wait(found, msg=f"a failed reply naming {text!r}", timeout=timeout)


def status_bar_shown(app: App) -> bool:
    """whether the band the status bar sits in is drawn in its color"""
    img = app.shot()
    row = img[img.shape[0] - 6, 4 : img.shape[1] // 2]
    return tuple(int(v) for v in median(row)) == tuple(C["tab_bar_background"])


def status_text(app: App, side: str) -> tuple | None:
    """bbox of the text on one side of the status bar, in window pixels. the bar's top border
    is a line over its whole width, so the pixels right under it are left out"""
    img = app.shot()
    h = round(round(1.5 * app.ui_font_size()) * app.scale)
    top, bottom = img.shape[0] - h + 6, img.shape[0] - 4
    half = img.shape[1] // 2
    left = (0 if side == "left" else half) + 4
    box = mask_bbox(ink(img[top:bottom, left : left + half - 8], C["tab_bar_background"]))
    return box and (box[0] + left, box[1] + top, box[2] + left, box[3] + top)


def tab_title_ink(app: App) -> int:
    """text pixels drawn over the background of the active tab, so a longer title has more"""
    img = app.shot()
    band = img[2 : app.bar_height() - 2]
    columns = np.flatnonzero(near(band, C["tab_active_background"]).any(axis=0))
    if not len(columns):
        return 0
    x0, x1 = int(columns[0]), int(columns[-1]) + 1
    return int(ink(band[:, x0:x1], C["tab_active_background"]).sum())


def underline_ink(app: App, row: int, col: int, cols: int) -> int:
    """text colored pixels in the bottom pixel rows of some cells, where the underline goes"""
    x0, _, x1, y1 = app.cell_rect(col, row, cols, top=app.top())
    return int(ink(app.shot()[y1 - 3 : y1, x0:x1], C["terminal_background"]).sum())


def columns_seen(app: App, name: str) -> int:
    """the columns the shell sees, asked by a command typing the size into a file"""
    send(app, f"stty size > $T2_TMP/{name}.part && mv $T2_TMP/{name}.part $T2_TMP/{name}")
    return int(app.wait_file(name).split()[1])


def ready_quiet(app: App, pane: str, marker: str) -> None:
    """like kit.ready, for a pane whose blocks can't be read back: the shell is up when the
    command it was sent wrote its marker file"""
    send(app, f"touch $T2_TMP/{marker}", pane)
    app.wait_file(marker)


def new_incognito(app: App) -> str:
    """open the incognito tab of the palette and return its pane id"""
    app.palette("new incognito tab")
    app.wait(lambda: len(tabs(app)) == 2, msg="the incognito tab")
    pane = str(next(tab["id"] for tab in tabs(app) if tab["active"]))
    ready_quiet(app, pane, "incognito_ready")
    return pane


def active_pane(app: App) -> int:
    """the id of the tab in front of the focused pane"""
    return next(tab["id"] for tab in tabs(app) if tab["active"])


def block_texts(app: App, name: str) -> list[str]:
    """the texts of the title block the plugin set, oldest first"""
    return [message["params"]["text"] for message in seen(app, name, "set_title_block", "sent")]


def test_a_plugin_is_started_and_answered_by_initialize(app_factory, tmp_path):
    """a local plugin in settings is started and asked with `initialize`, which tells it the
    kuterm version and the permissions of its settings entry"""
    script = write_plugin(tmp_path, "e2e", caps=SAYS_HELLO)
    app = start(app_factory, [entry("e2e", script, {"panes": True})])
    allow(app)
    told = wait_seen(app, "e2e", "initialize")["params"]
    assert told["kuterm"] == kuterm_version(), told
    assert told["permissions"] == {"panes": True, "pty_write": False, "run": False, "clipboard": False}
    # the plugin answered, which is what adds its capabilities
    app.wait(lambda: any(entry.get("sent", {}).get("result") == SAYS_HELLO
                         for entry in entries(app, "e2e")),
             msg="the answer to initialize")
    app.snap("the plugin answered initialize")


def test_a_palette_command_tells_the_plugin_and_it_notifies_back(app_factory, tmp_path):
    """a palette command of the plugin's answer runs, telling the plugin which pane picked it,
    and a notification the plugin sends back shows in the window"""
    script = write_plugin(tmp_path, "e2e", caps=SAYS_HELLO, body=BODY_HELLO)
    # a notification that stays until clicked is caught by a screenshot at any time
    app = start(app_factory, [entry("e2e", script)], notifications={"timeout": 0})
    allow(app)
    wait_seen(app, "e2e", "initialize")
    ready(app)
    pane = active_pane(app)
    app.palette("plugin says hello")
    told = wait_seen(app, "e2e", "command")["params"]
    assert told == {"name": "plugin says hello", "pane": pane}, told
    app.wait(lambda: notification_box(app, C), msg="the notification the plugin sent")
    app.snap("the plugin was told of the command and notified back")


def test_a_plugin_title_block_shows_in_titles_and_the_status_bar(app_factory, tmp_path):
    """{"plugin": "e2e/commands"} shows in tab_title, window_title and the status bar, and
    set_title_block updates all of them live"""
    caps = {"title_blocks": ["commands"],
            "commands": [{"name": "plugin sets a block", "category": "e2e"}]}
    setup = 'blocks = ["plugin block", "plugin block, updated and longer"]'
    body = '''
if message.get("method") == "command":
    notify("set_title_block", {"name": "commands", "text": blocks.pop(0)})
'''
    script = write_plugin(tmp_path, "e2e", caps=caps, body=body, setup=setup)
    block = {"plugin": "e2e/commands"}
    # the answer buttons of the question share their border color with the status bar, so the
    # bar comes on with a config reload after the plugin was allowed
    app = start(app_factory, [entry("e2e", script)],
                tab_title=["number", {"text": " "}, block],
                window_title=["number", {"text": " "}, block])
    allow(app)
    wait_seen(app, "e2e", "initialize")
    save(app, status_bar={"enable": True, "left": [block], "right": []})
    app.wait(lambda: status_bar_shown(app), msg="the status bar of the reloaded config")
    # the plugin set no text yet, so the block is empty in both titles and the bar
    app.wait_title("1 ")
    plain_title = tab_title_ink(app)
    assert status_text(app, "left") is None
    app.palette("plugin sets a block")
    app.wait_title("1 plugin block")
    short = app.wait(lambda: status_text(app, "left"), msg="the block in the status bar")
    assert tab_title_ink(app) > plain_title, "the tab title grew with the block"
    app.snap("the plugin's block in the tab title, the window title and the status bar")
    app.palette("plugin sets a block")
    app.wait_title("1 plugin block, updated and longer")

    def wider() -> tuple | None:
        box = status_text(app, "left")
        return box if box and box[2] - box[0] > short[2] - short[0] else None

    long = app.wait(wider, msg="the updated block in the status bar")
    # the status bar grew by the characters the new text has more
    assert long[2] - long[0] > short[2] - short[0] + 20, (short, long)
    app.snap("set_title_block updated the same block everywhere")


def test_a_click_on_a_pattern_tells_the_plugin(app_factory, tmp_path):
    """a link pattern the answer added underlines with the link modifier held, its click tells
    the plugin the text and the pane"""
    caps = {"patterns": [{"name": "ticket", "regex": "TICKET-[0-9]+"}]}
    script = write_plugin(tmp_path, "e2e", caps=caps)
    # no marks of the shell integration, so the grid never moves for a gutter
    app = start(app_factory, [entry("e2e", script)],
                shell_integration={"mark_sources": "none"})
    allow(app)
    wait_seen(app, "e2e", "initialize")
    ready(app)
    pane = active_pane(app)
    send(app, "clear; printf 'TICKET-42\\n'")
    app.wait_line(0, 9, top=app.top())
    x0, y0, x1, y1 = app.cell_rect(4, 0, top=app.top())
    app.mouse((x0 + x1) // 2, (y0 + y1) // 2)
    app.wait_still()
    plain = underline_ink(app, 0, 0, 9)
    with app.hold("ctrl"):
        app.mouse((x0 + x1) // 2 + 1, (y0 + y1) // 2)
        app.wait(lambda: underline_ink(app, 0, 0, 9) > plain + 20, msg="the pattern underlined")
        app.snap("holding the link modifier, the pattern is underlined")
        app.button(1, True)
        app.button(1, False)
    told = wait_seen(app, "e2e", "pattern")["params"]
    assert told == {"name": "ticket", "text": "TICKET-42", "pane": pane}, told


def test_a_plugin_reads_panes_and_the_rest_is_refused(app_factory, tmp_path):
    """with "panes" the plugin reads the text and the commands of a pane; its requests for
    typing and opening a tab come back refused, naming the permission it has not"""
    script = write_plugin(tmp_path, "e2e", caps=SAYS_HELLO, body=BODY_READ)
    app = start(app_factory, [entry("e2e", script, {"panes": True})])
    allow(app)
    wait_seen(app, "e2e", "initialize")
    ready(app)
    send(app, "echo read-me-marker")
    wait_block(app, "echo read-me-marker")
    app.palette("plugin says hello")
    text = wait_answer(app, "e2e", lambda result: "read-me-marker" in (result.get("text") or ""),
                       msg="the pane text the plugin asked for")
    assert "read-me-marker" in text["text"]
    asked = wait_answer(app, "e2e", lambda result: result.get("blocks"), msg="the pane's commands")
    assert asked["blocks"][-1]["command"] == "echo read-me-marker", asked["blocks"]
    assert "pty_write" in wait_error(app, "e2e", "pty_write")["message"]
    assert "run" in wait_error(app, "e2e", "run")["message"]
    # neither went through: the pane got nothing typed and no tab opened
    nothing_for(2, lambda: app.file("typed_by_plugin").exists() or len(tabs(app)) > 1)
    app.snap("the plugin read the pane, its other requests were refused")


def test_a_plugin_types_into_a_pane(app_factory, tmp_path):
    """with "pty_write" a request of the plugin types into the pane it was told about"""
    script = write_plugin(tmp_path, "e2e", caps=SAYS_HELLO, body=BODY_TYPE)
    app = start(app_factory, [entry("e2e", script, {"pty_write": True})])
    allow(app)
    wait_seen(app, "e2e", "initialize")
    ready(app)
    app.palette("plugin says hello")
    app.wait_file("typed_by_plugin", "ran")
    app.snap("a command the plugin typed ran in the pane")


def test_a_plugin_opens_a_tab(app_factory, tmp_path):
    """with "run" a request of the plugin opens a tab of its own in the window"""
    body = '''
if message.get("method") == "command":
    ask("new_tab", name="from the plugin")
'''
    script = write_plugin(tmp_path, "e2e", caps=SAYS_HELLO, body=body)
    app = start(app_factory, [entry("e2e", script, {"run": True})])
    allow(app)
    wait_seen(app, "e2e", "initialize")
    ready(app)
    app.palette("plugin says hello")
    app.wait(lambda: len(tabs(app)) == 2, msg="the tab the plugin opened")
    assert any(tab["name"] == "from the plugin" for tab in tabs(app)), tabs(app)
    app.snap("the tab the plugin opened")


def test_a_panel_opens_a_tab_running_its_command(app_factory, tmp_path):
    """a panel of the answer shows as "open <name>" in the palette, running its command in a
    tab of its own, which needs "run\""""
    caps = {"panels": [{"name": "e2e panel",
                        "command": ["sh", "-c", "printf 'PANEL-READY\\n'; sleep 100"]}]}
    script = write_plugin(tmp_path, "e2e", caps=caps)
    app = start(app_factory, [entry("e2e", script, {"run": True})])
    allow(app)
    wait_seen(app, "e2e", "initialize")
    ready(app)
    app.palette("open e2e panel")
    app.wait(lambda: len(tabs(app)) == 2, msg="the tab of the panel")
    wait_text(app, "PANEL-READY")
    assert next(tab for tab in tabs(app) if tab["active"])["title"] == "e2e panel", tabs(app)
    app.snap("the panel running in a tab of its own")


def test_a_panel_without_the_run_permission_opens_nothing(app_factory, tmp_path):
    """a panel to open needs "run": the palette command does nothing without it and the plugin's
    own open_panel request is answered with the missing permission"""
    caps = {"panels": [{"name": "e2e panel", "command": ["sh", "-c", "true"]}],
            "commands": [{"name": "plugin opens a panel", "category": "e2e"}]}
    body = '''
if message.get("method") == "command":
    request("open_panel", {"command": ["sh", "-c", "true"], "name": "e2e panel"})
'''
    script = write_plugin(tmp_path, "e2e", caps=caps, body=body)
    app = start(app_factory, [entry("e2e", script)])
    allow(app)
    wait_seen(app, "e2e", "initialize")
    ready(app)
    app.palette("open e2e panel")
    nothing_for(3, lambda: len(tabs(app)) > 1)
    app.palette("plugin opens a panel")
    assert "run" in wait_error(app, "e2e", "run")["message"]
    nothing_for(3, lambda: len(tabs(app)) > 1)
    app.snap("the panel stayed closed, its tab was never opened")


def test_the_question_comes_first_and_denying_keeps_the_plugin_off(app_factory, tmp_path):
    """kuterm asks before a plugin first runs, showing its command, and a plugin that is not
    allowed never starts"""
    script = write_plugin(tmp_path, "e2e", caps=SAYS_HELLO)
    app = start(app_factory, [entry("e2e", script, {"panes": True})])
    buttons = wait_buttons(app, C, 2, timeout=30)
    app.snap("the question kuterm asks before a plugin first runs")
    nothing_for(2, lambda: entries(app, "e2e"))
    app.click(*buttons[1])
    nothing_for(4, lambda: entries(app, "e2e"))
    # the app itself is untouched by the refusal
    ready(app)
    send(app, "echo no_plugin_here")
    wait_block(app, "echo no_plugin_here")
    app.snap("denied: the plugin never ran, the shell went on")


def test_an_allowed_plugin_is_remembered_and_a_changed_script_asks_again(app_factory, tmp_path):
    """the answer to the question is kept: a reload starts the plugin again without asking, but
    a script whose bytes changed asks again"""
    script = write_plugin(tmp_path, "e2e", caps=SAYS_HELLO)
    app = start(app_factory, [entry("e2e", script)])
    allow(app)
    app.wait(lambda: len(seen(app, "e2e", "initialize")) == 1, msg="the plugin started")
    save(app, sleep_tabs_after_minutes=31)
    app.wait(lambda: len(seen(app, "e2e", "initialize")) == 2,
             msg="the plugin started again after the reload, without a question")
    app.snap("the reload started the plugin again, it was not asked twice")
    script.write_text(script.read_text() + "\n# changed\n")
    save(app, sleep_tabs_after_minutes=32)
    allow(app)  # the changed script asks again
    app.wait(lambda: len(seen(app, "e2e", "initialize")) == 3, msg="the allowed plugin started")
    app.snap("the changed script was asked about again and then run")


def test_wider_permissions_ask_again(app_factory, tmp_path):
    """the hash covers the permissions too: a plugin that wants more than when it was allowed
    is asked about again"""
    script = write_plugin(tmp_path, "e2e", caps=SAYS_HELLO)
    app = start(app_factory, [entry("e2e", script)])
    allow(app)
    wait_seen(app, "e2e", "initialize")
    save(app, plugins=[entry("e2e", script, {"panes": True})])
    allow(app)
    told = app.wait(lambda: len(seen(app, "e2e", "initialize")) > 1
                    and seen(app, "e2e", "initialize")[-1]["params"],
                    msg="the plugin asked again and started with its new permissions")
    assert told["permissions"]["panes"] is True, told
    app.snap("the wider permissions were asked about again")


def test_a_plugin_that_exits_is_started_again(app_factory, tmp_path):
    """a plugin that exits is started again, and the app keeps running while it comes back"""
    script = write_plugin(tmp_path, "e2e", caps=SAYS_HELLO, after="sys.exit(0)")
    app = start(app_factory, [entry("e2e", script)])
    allow(app)
    app.wait(lambda: len(seen(app, "e2e", "initialize")) > 1, msg="the plugin started again")
    send(app, "echo still_alive")
    wait_block(app, "echo still_alive")
    app.snap("the plugin exited and was started again")


def test_reloading_the_config_restarts_plugins(app_factory, tmp_path):
    """every plugin restarts when the config is reloaded and works as before"""
    script = write_plugin(tmp_path, "e2e", caps=SAYS_HELLO, body=BODY_HELLO)
    app = start(app_factory, [entry("e2e", script)])
    allow(app)
    app.wait(lambda: len(seen(app, "e2e", "initialize")) == 1, msg="the plugin started")
    save(app, sleep_tabs_after_minutes=31)
    app.wait(lambda: len(seen(app, "e2e", "initialize")) == 2, msg="the plugin restarted")
    # the restarted plugin still answers its command
    app.palette("plugin says hello")
    app.wait(lambda: len(seen(app, "e2e", "command")) == 1, msg="the command reached it")
    app.snap("the plugin restarted with the config and still answers")


def test_a_plugin_marks_the_block_it_decorates(app_factory, tmp_path):
    """a `decorate` request puts a mark in the gutter next to the block's command, which takes
    two columns off the grid, its tooltip shows on hover, and the title block it updates with
    every command follows live"""
    caps = {"title_blocks": ["commands"]}
    setup = "marked = []"
    body = '''
if message.get("method") == "event" and message["params"].get("event") == "command_finished":
    params = message["params"]
    marked.append(params)
    notify("set_title_block", {"name": "commands", "text": "marked %d" % params["block"]})
    request("decorate", {"pane": params.get("pane"), "block": params.get("block"),
                         "icon": "\\u2713", "tooltip": "exit 0"})
'''
    script = write_plugin(tmp_path, "e2e", caps=caps, body=body, setup=setup)
    app = start(app_factory, [entry("e2e", script, {"panes": True})],
                shell_integration={"mark_sources": "none"},
                window_title=["number", {"text": " "}, {"plugin": "e2e/commands"}])
    # the plugin is not allowed yet, so no block and no gutter
    app.wait_title("1 ")
    plain = columns_seen(app, "plain")
    allow(app)
    ready(app)
    # the block of every command that ends carries the mark of its block, and the title says so
    app.wait(lambda: block_texts(app, "e2e")
             and any(app.title() == f"1 {text}" for text in block_texts(app, "e2e")),
             msg="the title holds the block the plugin set")
    # the mark needs the gutter, which takes two columns off the grid
    app.wait(lambda: columns_seen(app, f"after{len(entries(app, 'e2e'))}") == plain - 2,
             msg="the gutter took two columns")
    app.snap("a mark in the gutter next to the command, its block in the title")
    app.wait_still()
    img = app.shot()
    x = round(app._padding()[0] * app.scale) + 2
    columns = img[:, x : x + round(app.cell_width() * 2) - 4]
    rows = (near(columns, C["terminal_background"], 10).all(axis=1) == 0).nonzero()[0]
    rows = rows[rows > app.bar_height()]
    assert len(rows), "no mark drawn in the gutter"
    before = int(near(img, C["tab_bar_background"]).sum())
    app.mouse(x + 4, int(rows[0]) + 4)
    app.wait(lambda: near(app.shot(), C["tab_bar_background"]).sum() > before + 200,
             msg="the tooltip of the mark")
    app.snap("the tooltip the plugin put on its mark")


def broken_entry(folder: Path, kind: str) -> dict:
    """a settings entry for one kind of plugin that misbehaves"""
    if kind == "not executable":
        script = write_plugin(folder, "noexec", caps=SAYS_HELLO)
        script.chmod(0o644)
        return {"name": "e2e", "command": [str(script)], "permissions": {}}
    if kind == "exits at once":
        return {"name": "e2e", "command": ["sh", "-c", "exit 3"], "permissions": {}}
    if kind == "answers garbage":
        script = folder / "garbage.py"
        script.write_text("import sys, time\nsys.stdout.write('garbage\\r\\n\\r\\n' * 3)\n"
                          "sys.stdout.flush()\ntime.sleep(120)\n")
        return {"name": "e2e", "command": ["python3", str(script)], "permissions": {}}
    # never answers: it reads what kuterm sends and stays quiet
    script = folder / "silent.py"
    script.write_text("import time\nwhile True:\n    time.sleep(0.2)\n")
    return {"name": "e2e", "command": ["python3", str(script)], "permissions": {"panes": True}}


@pytest.mark.parametrize("kind", ["not executable", "exits at once", "answers garbage",
                                  "never answers"])
def test_a_broken_plugin_never_takes_the_app_down(app_factory, tmp_path, kind):
    """a plugin that can not be started, exits at once, answers garbage or never answers: the
    app stays alive, its shell still runs commands and the window still takes keys"""
    folder = tmp_path / kind.replace(" ", "_")
    folder.mkdir()
    app = start(app_factory, [broken_entry(folder, kind)])
    allow(app)
    ready(app)
    send(app, "echo still_working")
    wait_block(app, "echo still_working")
    app.palette("reload config", run=False)
    app.key("Escape")
    app.wait_still()
    app.snap(f"the app kept working with a plugin that {kind}")
    assert app.alive()


def test_a_bad_github_source_does_not_break_startup(app_factory, tmp_path):
    """a "github:" entry that is no source or can not be fetched fails on its own, the app
    starts and the plugins next to it still run"""
    script = write_plugin(tmp_path, "e2e", caps=SAYS_HELLO)
    app = start(app_factory, [entry("e2e", script), "github:not-a-repo",
                              "github:nobody/kuterm-no-such-plugin"])
    allow(app)
    wait_seen(app, "e2e", "initialize")
    app.wait(lambda: "not-a-repo" in app.output() and "kuterm-no-such-plugin" in app.output(),
             msg="the bad sources told on stderr", timeout=60)
    app.snap("the bad sources failed, the local plugin next to them runs")


def test_a_plugin_command_from_a_keymap_and_from_remote_control(app_factory, tmp_path):
    """a settings command running a plugin command works from its key and over `kuterm msg
    run-command`, telling the plugin both times"""
    script = write_plugin(tmp_path, "e2e", caps=SAYS_HELLO)
    app = start(app_factory, [entry("e2e", script)],
                commands=[{"name": "plugin via a command", "category": "e2e",
                           "actions": [{"plugin": {"plugin": "e2e",
                                                   "command": "plugin says hello"}}]}],
                keymap=[{"keys": "ctrl-alt-y", "action": {"run": "plugin via a command"}}])
    allow(app)
    wait_seen(app, "e2e", "initialize")
    ready(app)
    assert msg(app, "run-command", "plugin via a command").returncode == 0
    app.wait(lambda: len(seen(app, "e2e", "command")) == 1, msg="the command over remote control")
    app.key("ctrl+alt+y")
    app.wait(lambda: len(seen(app, "e2e", "command")) == 2, msg="the command from its key")
    app.snap("the plugin was told from the key and over remote control")


def test_a_plugin_cannot_read_an_incognito_pane(app_factory, tmp_path):
    """an incognito pane refuses a plugin's get_text like it refuses remote control, and the
    pane next to it reads fine"""
    script = write_plugin(tmp_path, "e2e", caps=SAYS_HELLO, body=BODY_READ_PANE)
    app = start(app_factory, [entry("e2e", script, {"panes": True})])
    allow(app)
    wait_seen(app, "e2e", "initialize")
    ready(app)
    first = str(active_pane(app))
    new_incognito(app)
    app.palette("plugin says hello")
    assert "incognito" in wait_refusal(app, "e2e", "incognito")
    app.snap("the plugin was refused the incognito pane")
    # the other tab is no incognito one, its text reads fine
    assert msg(app, "focus-tab", "--pane", first).returncode == 0
    app.wait(lambda: active_pane(app) == int(first), msg="the first tab in front")
    send(app, "echo not_incognito")
    wait_block(app, "echo not_incognito")
    app.palette("plugin says hello")
    text = wait_answer(app, "e2e", lambda result: "not_incognito" in (result.get("text") or ""),
                       msg="the text of the readable pane")
    assert "not_incognito" in text["text"]
    app.snap("the same plugin reads the tab that is no incognito one")


def test_a_plugin_typing_a_command_keeps_the_notification_actions(app_factory, tmp_path):
    """a command a plugin typed is a command like any other: with notifications.actions its
    desktop notification offers show, run again and open first error"""
    body = '''
if message.get("method") == "command":
    ask("send_text", pane=str(message["params"]["pane"]),
        text="echo ran >> $T2_TMP/typed_by_plugin; sleep 10; echo src/main.rs:12: oops; false\\n")
'''
    script = write_plugin(tmp_path, "e2e", caps=SAYS_HELLO, body=body)
    server = Notifications()
    try:
        app = start(app_factory, [entry("e2e", script, {"pty_write": True})],
                    shell_integration={"notify_after": 1},
                    hints={"patterns": [{"name": "rust", "regex": "[\\w/]+\\.rs", "action": "copy"}]},
                    notifications={"actions": True})
        allow(app)
        wait_seen(app, "e2e", "initialize")
        ready(app)
        app.palette("plugin says hello")
        app.wait_file("typed_by_plugin", "ran")
        # another window takes the focus, so the end of the command is told on the desktop
        app_factory({"theme": {"mode": "dark"}})
        shown = server.wait(lambda n: "failed" in n["body"], msg="the desktop notification",
                            timeout=30)
        assert shown["actions"][::2] == ["default", "run-again", "open-error"], shown
        app.snap("the command the plugin typed ended, its notification offers its buttons")
    finally:
        server.stop()
