"""kuterm msg as a client of the socket: what it prints for each kind of reply,
how it fails without a kuterm, and the startup paths that only show in the real binary"""

import json
import socket
import subprocess
import tempfile
import threading
from pathlib import Path

from harness import BIN, Session
from kit import listening, socket_of

FEATURE = "remote control clients"


class FakeKuterm:
    """a unix socket answering like a running kuterm: one json reply per connection, then the
    lines of `stream` for a subscription"""

    def __init__(self, reply: dict, stream: list[dict] | None = None):
        self.dir = tempfile.mkdtemp(prefix="e2e-fake-kuterm-")
        self.path = str(Path(self.dir) / "kuterm-fake.sock")
        self.reply, self.stream = reply, stream or []
        self.requests: list[dict] = []
        self.server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.server.bind(self.path)
        self.server.listen()
        self.closed = threading.Event()
        threading.Thread(target=self._serve, daemon=True).start()

    def _serve(self) -> None:
        while not self.closed.is_set():
            try:
                conn, _ = self.server.accept()
            except OSError:
                return
            with conn:
                line = conn.makefile().readline()
                self.requests.append(json.loads(line))
                conn.sendall((json.dumps(self.reply) + "\n").encode())
                for event in self.stream:
                    conn.sendall((json.dumps(event) + "\n").encode())

    def close(self) -> None:
        self.closed.set()
        self.server.close()


def client(*args: str, sock: str, stdin: str | None = None, timeout: float = 20) -> subprocess.CompletedProcess:
    env = {"PATH": "/usr/bin:/bin", "KUTERM_SOCKET": sock, "XDG_RUNTIME_DIR": Session.runtime_dir}
    return subprocess.run([str(BIN), *args], env=env, input=stdin, capture_output=True, text=True,
                          timeout=timeout)


def test_msg_without_a_running_kuterm_fails_with_a_reason():
    missing = str(Path(tempfile.mkdtemp()) / "none.sock")
    for args in (["msg", "list-tabs"], ["msg", "subscribe"]):
        done = client(*args, sock=missing)
        assert done.returncode == 1, (args, done)
        assert "no running kuterm" in done.stderr, (args, done.stderr)
    # bad arguments show the usage, --help shows it on stdout
    done = client("msg", "fly", sock=missing)
    assert done.returncode == 2 and "usage: kuterm msg" in done.stderr, done
    done = client("msg", "list-tabs", "--help", sock=missing)
    assert done.returncode == 0 and done.stdout.startswith("usage: kuterm msg"), done


def test_msg_prints_tabs_and_blocks_as_lines():
    tabs = [
        {"id": 3, "name": None, "window": 1, "pane": 0, "number": 1, "title": "build", "active": True,
         "cwd": "/tmp"},
        {"id": 4, "name": None, "window": 1, "pane": 0, "number": 2, "title": "logs", "active": False,
         "cwd": None},
    ]
    fake = FakeKuterm({"ok": True, "tabs": tabs})
    try:
        done = client("msg", "list-tabs", sock=fake.path)
        assert done.returncode == 0, done
        assert done.stdout.splitlines() == ["* 3 1:0:1 build  /tmp", "  4 1:0:2 logs"]
    finally:
        fake.close()

    block = {"id": 9, "number": 1, "command": "make", "exit": 0, "cwd": None, "started": 1.0,
             "seconds": 2.0, "output": "built", "hidden": False, "link": "kuterm://block/9"}
    running = {**block, "id": 10, "number": 2, "command": "sleep 9", "exit": None, "output": None}
    fake = FakeKuterm({"ok": True, "blocks": [block, running]})
    try:
        done = client("msg", "query", "--all", sock=fake.path)
        assert done.returncode == 0, done
        # a block still running has no exit yet
        assert done.stdout.splitlines() == ["1 [0] make", "2 [-] sleep 9"]
        assert fake.requests[-1] == {"cmd": "query", "pane": None, "block": None, "all": True}
    finally:
        fake.close()


def test_msg_subscribe_prints_events_until_the_stream_ends():
    events = [{"event": "bell", "pane": 3}, {"event": "command_finished", "pane": 3, "exit": 1}]
    fake = FakeKuterm({"ok": True}, stream=events)
    try:
        done = client("msg", "subscribe", "bell", sock=fake.path)
        assert done.returncode == 0, done
        assert [json.loads(line) for line in done.stdout.splitlines()] == events
        assert fake.requests == [{"cmd": "subscribe", "events": ["bell"]}]
    finally:
        fake.close()
    # a refused subscription tells why
    fake = FakeKuterm({"ok": False, "error": "remote control is off"})
    try:
        done = client("msg", "subscribe", sock=fake.path)
        assert done.returncode == 1 and "remote control is off" in done.stderr, done
    finally:
        fake.close()


def test_missing_keybindings_file_is_a_warning(app_factory):
    """a --keybindings-file that is not there is skipped with a warning, kuterm still starts"""
    missing = Path(tempfile.mkdtemp()) / "keys.jsonc"
    app = app_factory({"theme": {"mode": "dark"}}, args=["--keybindings-file", str(missing)])
    app.wait(lambda: f"{missing} not found, skipped" in app.output(), msg="warning on stderr")


def test_single_instance_without_a_running_kuterm_starts_its_own(app_factory):
    """--single-instance with no kuterm to take over opens a window of its own"""
    app = app_factory({"theme": {"mode": "dark"}}, args=["--single-instance"],
                      env={"KUTERM_SOCKET": str(Path(tempfile.mkdtemp()) / "none.sock")})
    assert app.alive()


def test_socket_answers_a_bad_request_with_its_error(app_factory):
    app = app_factory({"theme": {"mode": "dark"}, "remote_control": "socket"})
    listening(app)
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as conn:
        conn.settimeout(10)
        conn.connect(str(socket_of(app)))
        conn.sendall(b'{"cmd": "fly"}\n')
        reply = json.loads(conn.makefile().readline())
    assert reply["ok"] is False and reply["error"].startswith("bad request"), reply
