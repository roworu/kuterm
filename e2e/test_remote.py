"""remote control: kuterm msg drives the running app and --single-instance"""

import json
import subprocess
import time
from pathlib import Path

import pytest

from harness import (BIN, App, Session, as_rgb, dominant, ink_bbox, mask_bbox, near, unique_theme,
                     xdo)

FEATURE = "remote control"

SOCKET = {"theme": {"mode": "dark"}, "window_title": ["number"], "remote_control": "socket"}


def test_msg_new_tab_send_text_and_list_tabs(app_factory):
    """kuterm msg run in a tab reaches its own kuterm through $KUTERM_SOCKET"""
    app = app_factory(dict(SOCKET))
    listening(app)
    app.run(f"{BIN} msg new-tab --cwd /tmp")
    app.wait_title("2")
    app.run(f"{BIN} msg list-tabs --json > {app.file('tabs.json')}")
    tabs = json.loads(app.wait_file("tabs.json"))
    assert [tab["number"] for tab in tabs] == [1, 2]
    assert tabs[1]["active"] and tabs[1]["cwd"] == "/tmp"
    # the text is typed into the shell once this command is done, its newline runs it
    app.run(f"{BIN} msg send-text $'touch {app.file('typed')}\\n'")
    app.wait(app.file("typed").exists, msg="text sent by msg ran")


def listening(app: App) -> None:
    """wait for the app's remote control socket"""
    socket = Path(Session.runtime_dir) / f"kuterm-{app.proc.pid}.sock"
    app.wait(socket.exists, msg=f"socket {socket}")


def windows(app: App) -> list:
    return xdo("search", "--onlyvisible", "--pid", str(app.proc.pid), check=False).split()


def env_of(app: App) -> dict:
    env = app.app_env()
    env.pop("KUTERM_SOCKET", None)
    return env


@pytest.mark.x11_only
def test_single_instance_opens_a_window_in_the_running_app(app_factory):
    """kuterm --single-instance asks the running kuterm for a window and exits"""
    app = app_factory(dict(SOCKET))
    listening(app)
    done = subprocess.run([str(BIN), "--single-instance"], env=env_of(app), timeout=20,
                          capture_output=True)
    assert done.returncode == 0, done.stderr
    app.wait(lambda: len(windows(app)) == 2, msg="second window")


def command_line_end(app: App, row: int) -> int:
    """right edge of the ink on a row, 0 when it is blank"""
    x0, y0, _, y1 = app.cell_rect(0, row)
    band = app.shot()[y0:y1, x0:]
    box = ink_bbox(band, dominant(band))
    return 0 if box is None else x0 + box[2]


def test_run_link_types_its_command_without_running_it(app_factory):
    """kuterm started with a kuterm://run link opens with the command typed, nothing runs"""
    app = app_factory({"theme": {"mode": "dark"}}, args=["kuterm://run?cmd=echo%20hi%0Aecho%20gone"])
    # "$ echo hiecho gone" and the cursor after it, the newline was taken out
    app.wait(lambda: command_line_end(app, 0) > app.cell_rect(18, 0)[0], msg="command typed")
    app.snap("the command of the link waits on the command line")
    # nothing ran, so no output line under it
    time.sleep(1)
    assert command_line_end(app, 1) == 0


def test_run_link_opens_a_tab_in_the_running_kuterm(app_factory):
    """a run link opened while kuterm runs becomes a new tab of it"""
    app = app_factory(dict(SOCKET))
    listening(app)
    socket = Path(Session.runtime_dir) / f"kuterm-{app.proc.pid}.sock"
    subprocess.run([str(BIN), "kuterm://run?cwd=%2Ftmp&cmd=pwd"], env={**app.app_env(), "KUTERM_SOCKET": str(socket)},
                   timeout=10, check=True)
    app.wait_title("2")
    app.wait(lambda: command_line_end(app, 0) > app.cell_rect(4, 0)[0], msg="pwd typed")


# bash without arguments gets kuterm's integration, so its commands become blocks
INTEGRATED = {**SOCKET, "profiles": [{"name": "bash", "command": {"program": "/bin/bash"}, "env": {"PS1": "$ "}}],
              "hide_bar_for_one_tab": False}
ASK_THEME = unique_theme(0x7b)
ASK = as_rgb(ASK_THEME)


def msg(app: App, *args: str, timeout: float = 10) -> subprocess.CompletedProcess:
    socket = Path(Session.runtime_dir) / f"kuterm-{app.proc.pid}.sock"
    return subprocess.run([str(BIN), "msg", *args], env={**app.app_env(), "KUTERM_SOCKET": str(socket)},
                          capture_output=True, text=True, timeout=timeout)


def query(app: App, pane: str, *args: str) -> dict | None:
    done = msg(app, "query", "--pane", pane, "--json", *args)
    return json.loads(done.stdout) if done.returncode == 0 and done.stdout.strip() else None


def worker(app_factory) -> App:
    """an integrated app with a second tab named worker, at its prompt"""
    app = app_factory(dict(INTEGRATED), themes={"dark": ASK_THEME})
    listening(app)
    assert msg(app, "new-tab", "--name", "worker").returncode == 0
    app.wait_title("2")
    # the prompt is drawn once the shell read its integration
    msg(app, "send-text", "--pane", "worker", "true\n")
    app.wait(lambda: query(app, "worker"), msg="first block of worker")
    return app


def test_query_names_and_hidden_text(app_factory):
    """a named pane runs a command sent to it, query returns its exit and output as people see
    it, get-text leaves concealed text out"""
    app = worker(app_factory)
    msg(app, "send-text", "--pane", "worker", "printf 'out1\\nout2 \\033[8msecret\\033[0m\\n'; false\n")
    block = app.wait(lambda: (b := query(app, "worker")) and b["command"].startswith("printf") and b,
                     msg="the printf block")
    assert block["exit"] == 1, block
    assert block["output"].split() == ["out1", "out2"], block
    assert block["hidden"] and block["link"].startswith("kuterm://block/")
    lines = msg(app, "get-text", "--pane", "worker").stdout.splitlines()
    # the output line, not the typed command line that spells the secret out
    assert "out2" in lines and not any(line.startswith("out2 ") and "secret" in line for line in lines), lines
    app.snap("the worker tab ran the command sent to it, the secret is concealed")


def test_subscribe_sees_command_finished(app_factory):
    app = worker(app_factory)
    socket = Path(Session.runtime_dir) / f"kuterm-{app.proc.pid}.sock"
    events = subprocess.Popen([str(BIN), "msg", "subscribe", "command_finished"], stdout=subprocess.PIPE, text=True,
                              env={**app.app_env(), "KUTERM_SOCKET": str(socket)})
    try:
        # the subscription is in place once the socket answered it
        time.sleep(0.5)
        msg(app, "send-text", "--pane", "worker", "(exit 7)\n")
        line = events.stdout.readline()
        event = json.loads(line)
        assert event["event"] == "command_finished" and event["exit"] == 7, event
    finally:
        events.kill()


def test_kout_prints_the_last_output(app_factory):
    app = worker(app_factory)
    msg(app, "send-text", "--pane", "worker", "printf 'a\\nb\\nc\\n'\n")
    app.wait(lambda: (b := query(app, "worker")) and b["command"].startswith("printf"), msg="printf block")
    msg(app, "send-text", "--pane", "worker", f"kout | wc -l > {app.file('kout')}\n")
    app.wait_file("kout", "3")


def notification_buttons(app: App) -> list:
    """centers of the answer buttons in the bottom right notification, left to right"""
    img = app.shot()
    box = mask_bbox(near(img[300:, 400:], ASK["tab_bar_background"]))
    if box is None:
        return []
    x0, y0, x1, y1 = box[0] + 400 + 3, box[1] + 300 + 3, box[2] + 400 - 3, box[3] + 300 - 3
    # the notification has its own border, inside it only the buttons are outlined
    mask = near(img[y0:y1, x0:x1], ASK["border"])
    columns = mask.any(axis=0).nonzero()[0]
    rows = mask.any(axis=1).nonzero()[0]
    if len(columns) == 0:
        return []
    spans, start = [], columns[0]
    for a, b in zip(columns, columns[1:]):
        if b - a > 3:
            spans.append((start, a))
            start = b
    spans.append((start, columns[-1]))
    y = y0 + (rows[0] + rows[-1]) // 2
    return [(x0 + (a + b) // 2, y) for a, b in spans if b - a > 10]
