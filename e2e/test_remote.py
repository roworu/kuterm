"""remote control: kuterm msg drives the running app and --single-instance"""

import json
import subprocess
import time

import pytest

from harness import BIN, App, session_env, xdo

FEATURE = "remote control"

SOCKET = {"theme": {"mode": "dark"}, "window_title": ["number"], "remote_control": "socket"}


def test_msg_new_tab_send_text_and_list_tabs(app_factory):
    """kuterm msg run in a tab reaches its own kuterm through $KUTERM_SOCKET"""
    app = app_factory(dict(SOCKET))
    time.sleep(0.5)
    app.run(f"{BIN} msg new-tab --cwd /tmp")
    app.wait_title("2")
    app.run(f"{BIN} msg list-tabs --json > {app.file('tabs.json')}")
    tabs = json.loads(app.wait_file("tabs.json"))
    assert [tab["number"] for tab in tabs] == [1, 2]
    assert tabs[1]["active"] and tabs[1]["cwd"] == "/tmp"
    # the text is typed into the shell once this command is done, its newline runs it
    app.run(f"{BIN} msg send-text $'touch {app.file('typed')}\\n'")
    app.wait(app.file("typed").exists, msg="text sent by msg ran")


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
    time.sleep(0.5)
    done = subprocess.run([str(BIN), "--single-instance"], env=env_of(app), timeout=20,
                          capture_output=True)
    assert done.returncode == 0, done.stderr
    app.wait(lambda: len(windows(app)) == 2, msg="second window")

