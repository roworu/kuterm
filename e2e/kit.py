"""helpers shared by the tests of newer features: remote control as an oracle for what the app
holds, an integrated bash whose commands become blocks, and the answer buttons of in-window
notifications"""

import json
import os
import subprocess
import time
from pathlib import Path

import numpy as np

from harness import BIN, App, Session, mask_bbox, near

# remote control on, one tab per number in the window title unless a test sets its own
SOCKET = {"theme": {"mode": "dark"}, "remote_control": "socket"}


def integrated(**extra) -> dict:
    """settings for bash with kuterm's shell integration, so prompts and commands are marked"""
    settings = {**SOCKET, "hide_bar_for_one_tab": False,
                "profiles": [{"name": "bash", "command": {"program": "/bin/bash"}, "env": {"PS1": "$ "}}]}
    for key, value in extra.items():
        if isinstance(value, dict) and isinstance(settings.get(key), dict):
            settings[key] = {**settings[key], **value}
        else:
            settings[key] = value
    return settings


def socket_of(app: App) -> Path:
    return Path(Session.runtime_dir) / f"kuterm-{app.proc.pid}.sock"


def listening(app: App) -> None:
    """wait for the app's remote control socket"""
    app.wait(socket_of(app).exists, msg="remote control socket")


def msg(app: App, *args: str, timeout: float = 10) -> subprocess.CompletedProcess:
    """kuterm msg against this app"""
    return subprocess.run([str(BIN), "msg", *args], env={**app.app_env(), "KUTERM_SOCKET": str(socket_of(app))},
                          capture_output=True, text=True, timeout=timeout)


def tabs(app: App) -> list:
    done = msg(app, "list-tabs", "--json")
    return json.loads(done.stdout) if done.returncode == 0 and done.stdout.strip() else []


def active_id(app: App) -> str:
    return str(next(tab["id"] for tab in tabs(app) if tab["active"]))


def text(app: App, pane: str | None = None, scrollback: bool = False) -> str:
    """what a pane shows, the active tab without a pane"""
    args = ["get-text"] + (["--pane", pane] if pane else []) + (["--scrollback"] if scrollback else [])
    done = msg(app, *args)
    return done.stdout if done.returncode == 0 else ""


def wait_text(app: App, needle: str, pane: str | None = None, scrollback: bool = False,
              timeout: float = 15) -> str:
    return app.wait(lambda: (t := text(app, pane, scrollback)) and needle in t and t, timeout=timeout,
                    msg=f"{needle!r} in pane {pane or 'active'}")


def blocks(app: App, pane: str | None = None) -> list:
    args = ["query", "--json", "--all"] + (["--pane", pane] if pane else [])
    done = msg(app, *args)
    if done.returncode != 0 or not done.stdout.strip():
        return []
    data = json.loads(done.stdout)
    return data if isinstance(data, list) else [data]


def wait_block(app: App, starts: str, pane: str | None = None, finished: bool = True, timeout: float = 20) -> dict:
    """the newest block whose command starts with `starts`"""
    def find():
        for block in reversed(blocks(app, pane)):
            if block["command"].startswith(starts) and (not finished or block["seconds"] is not None):
                return block
        return None

    return app.wait(find, timeout=timeout, msg=f"block {starts!r}")


def ready(app: App, pane: str | None = None) -> None:
    """wait for the integration to mark a first command, so the shell reads its input"""
    listening(app)
    if pane:
        msg(app, "send-text", "--pane", pane, "true\n")
    else:
        msg(app, "send-text", "true\n")
    wait_block(app, "true", pane)


def send(app: App, line: str, pane: str | None = None) -> None:
    """type a command line over remote control and run it"""
    args = ["send-text"] + (["--pane", pane] if pane else []) + [line + "\n"]
    assert msg(app, *args).returncode == 0


def notification_box(app: App, colors: dict) -> tuple | None:
    """bbox of the notifications in the bottom right corner"""
    img = app.shot()
    h, w = img.shape[:2]
    mask = near(img[h // 2 :, w // 2 :], colors["tab_bar_background"])
    if mask.sum() < 300:
        return None
    box = mask_bbox(mask)
    return box[0] + w // 2, box[1] + h // 2, box[2] + w // 2, box[3] + h // 2


def notification_buttons(app: App, colors: dict) -> list:
    """centers of the answer buttons of the lowest notification, left to right"""
    img = app.shot()
    box = notification_box(app, colors)
    if box is None:
        return []
    x0, y0, x1, y1 = box[0] + 3, box[1] + 3, box[2] - 3, box[3] - 3
    # the notification has its own border, inside it only the buttons are outlined
    mask = near(img[y0:y1, x0:x1], colors["border"])
    rows = mask.any(axis=1).nonzero()[0]
    if len(rows) == 0:
        return []
    # the lowest row of buttons belongs to the newest notification
    last = rows[-1]
    band = rows[rows > last - 40]
    mask = mask[band[0] : last + 1]
    columns = mask.any(axis=0).nonzero()[0]
    if len(columns) == 0:
        return []
    spans, start = [], columns[0]
    for a, b in zip(columns, columns[1:]):
        if b - a > 3:
            spans.append((start, a))
            start = b
    spans.append((start, columns[-1]))
    y = y0 + (band[0] + last) // 2
    return [(x0 + (a + b) // 2, y) for a, b in spans if b - a > 10]


def wait_buttons(app: App, colors: dict, n: int, timeout: float = 15) -> list:
    return app.wait(lambda: len(b := notification_buttons(app, colors)) == n and b, timeout=timeout,
                    msg=f"{n} answer buttons")


def executable(path: Path, text: str) -> Path:
    path.write_text(text)
    path.chmod(0o755)
    return path


def nothing_for(seconds: float, check) -> None:
    """check stays false for a while, for things that must not happen"""
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        assert not check(), "it happened"
        time.sleep(0.1)


def ink_columns(img: np.ndarray, background: tuple) -> np.ndarray:
    """columns of img holding anything but the background"""
    return (np.abs(img.astype(int) - np.array(background)).sum(axis=2) > 60).any(axis=0)


def env_without_socket(app: App) -> dict:
    env = dict(app.app_env())
    env.pop("KUTERM_SOCKET", None)
    return env


def is_root() -> bool:
    return os.geteuid() == 0
