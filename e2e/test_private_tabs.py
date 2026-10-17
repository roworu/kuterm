"""incognito and sandboxed profiles: incognito tabs keep no shell history, never come back with
the session, can't be read over remote control and get a frame; sandboxed profiles start their
shell under bubblewrap with the profile's rules, and refuse to open without it"""

import subprocess
import tempfile
import time
from pathlib import Path

import pytest

from harness import App, as_rgb, near, unique_theme
from kit import executable, integrated, listening, msg, ready, send, tabs

FEATURE = "private tabs"

THEME = unique_theme(0x9f)
C = as_rgb(THEME)


def private_app(app_factory, profiles: list | None = None, **extra) -> App:
    settings = integrated(window_title=["number", {"text": " "}, "icon"], icons={"default": "D"}, **extra)
    if profiles:
        settings["profiles"] = settings["profiles"] + profiles
    app = app_factory(settings, themes={"dark": THEME})
    ready(app)
    app.mouse(450, 300)
    return app


def framed(app: App, color: tuple) -> int:
    img = app.shot()
    # the frame runs along the left edge of the pane
    return int(near(img[app.bar_height() + 20 : -20, :4], color, 3).sum())


def ready_quiet(app: App, pane: str, marker: str) -> None:
    """like kit.ready, for a pane whose blocks can't be read back: the shell is up when the
    command it was sent writes its marker file"""
    listening(app)
    send(app, f"touch $T2_TMP/{marker}", pane)
    app.wait_file(marker)


def new_incognito(app: App) -> str:
    app.palette("new incognito tab")
    app.wait_title(lambda title: title.startswith("2 "))
    pane = str(next(t["id"] for t in tabs(app) if t["active"]))
    ready_quiet(app, pane, "incognito_ready")
    return pane


def test_incognito_tab_has_a_frame_and_icon(app_factory):
    app = private_app(app_factory)
    assert framed(app, C["ansi"][5]) == 0
    new_incognito(app)
    app.wait_title("2 ")
    app.wait(lambda: framed(app, C["ansi"][5]) > 50, msg="magenta frame")
    app.snap("an incognito tab: purple frame, label and icon")
    app.key("ctrl+Tab")
    app.wait_title("1 D")
    assert framed(app, C["ansi"][5]) == 0


def test_incognito_bash_keeps_no_history_file(app_factory):
    app = private_app(app_factory)
    send(app, "echo \"[$HISTFILE]\" > $T2_TMP/normal")
    pane = new_incognito(app)
    send(app, "echo \"[$HISTFILE]\" > $T2_TMP/private", pane)
    assert app.wait_file("private").strip() == "[]"
    assert app.wait_file("normal").strip() not in ("[]", "")


def test_incognito_zsh_adds_nothing_to_history(app_factory):
    zsh = {"name": "zsh", "command": {"program": "/bin/zsh"}, "env": {"PS1": "$ "}}
    app = private_app(app_factory, profiles=[{**zsh, "name": "zsh-private", "incognito": True}, zsh])
    for profile, name in (("zsh-private", "private"), ("zsh", "normal")):
        assert msg(app, "new-tab", "--profile", profile, "--name", name).returncode == 0
        ready_quiet(app, name, f"up_{name}")
        send(app, f"echo one_marker; touch $T2_TMP/ran_{name}", name)
        app.wait_file(f"ran_{name}")
        send(app, f"fc -ln -1 > $T2_TMP/{name} 2>&1", name)
    assert "one_marker" in app.wait_file("normal")
    assert "one_marker" not in app.wait_file("private")


def test_incognito_tabs_cannot_be_read_remotely(app_factory):
    app = private_app(app_factory)
    pane = new_incognito(app)
    send(app, "echo secret_output", pane)
    time.sleep(1)
    for verb in (["get-text"], ["get-text", "--scrollback"], ["query"], ["query", "--all"]):
        done = msg(app, *verb, "--pane", pane)
        assert done.returncode != 0 and "secret_output" not in done.stdout, (verb, done)
        assert "incognito" in done.stderr + done.stdout
    # the other tab still reads fine
    assert msg(app, "get-text", "--pane", str(next(t["id"] for t in tabs(app) if t["number"] == 1))).returncode == 0
    # typing into it remotely still works, only reading is refused
    send(app, "touch $T2_TMP/typed", pane)
    app.wait(app.file("typed").exists, msg="remote typing into the incognito tab")


def test_incognito_profile_from_settings(app_factory):
    app = private_app(app_factory, profiles=[{"name": "secret", "command": {"program": "/bin/bash"},
                                              "env": {"PS1": "$ "}, "incognito": True}])
    assert msg(app, "new-tab", "--profile", "secret").returncode == 0
    app.wait_title("2 ")


def test_incognito_tabs_never_come_back_with_the_session(app_factory):
    app = private_app(app_factory, restore_session="always", confirm_quit="never")
    new_incognito(app)
    app.palette("quit")
    app.wait(lambda: not app.alive(), msg="quit")
    app.start()
    listening(app)
    app.wait(lambda: len(tabs(app)) == 1, msg="only the normal tab came back")
    time.sleep(1)
    assert len(tabs(app)) == 1 and app.title() == "1 D"


def test_a_window_of_only_incognito_tabs_saves_no_session(app_factory):
    app = private_app(app_factory, restore_session="always", confirm_quit="never")
    new_incognito(app)
    app.key("ctrl+Tab")
    app.wait_title("1 D")
    app.key("ctrl+shift+w")
    app.wait_title("1 ")
    app.palette("quit")
    app.wait(lambda: not app.alive(), msg="quit")
    app.start()
    listening(app)
    # nothing saved, a fresh default tab
    app.wait_title("1 D")


def stub_bwrap() -> Path:
    """a bwrap that writes its arguments and runs what follows "--", as the real one would"""
    folder = Path(tempfile.mkdtemp(prefix="e2e-bwrap-"))
    executable(folder / "bwrap", '#!/bin/sh\nprintf "%s\\n" "$@" > "$T2_TMP/bwrap_args"\n'
               'while [ "$1" != "--" ]; do shift; done; shift; exec "$@"\n')
    return folder


def boxed(app_factory, sandbox: dict, path: str) -> App:
    profile = {"name": "box", "command": {"program": "/bin/bash"}, "env": {"PS1": "$ "},
               "working_directory": "/tmp", "sandbox": sandbox}
    settings = integrated(window_title=["number", {"text": " "}, "icon"], icons={"default": "D"})
    settings["profiles"] = settings["profiles"] + [profile]
    app = app_factory(settings, themes={"dark": THEME}, env={"PATH": path})
    listening(app)
    return app


STRICT = {"network": False, "write": ["cwd", "~/.cache"], "tmp": "private"}


def test_sandboxed_profile_runs_under_bwrap_with_its_rules(app_factory):
    stub = stub_bwrap()
    app = boxed(app_factory, STRICT, f"{stub}:/usr/bin:/bin")
    done = msg(app, "new-tab", "--profile", "box")
    assert done.returncode == 0, done.stderr
    app.wait_title("2 ")
    args = app.wait_file("bwrap_args").split("\n")
    line = " ".join(args)
    assert "--ro-bind / /" in line and "--unshare-net" in line and "--tmpfs /tmp" in line
    assert "--bind /tmp /tmp" in line and "--die-with-parent" in line
    assert f"--bind {app.home}/.cache {app.home}/.cache" in line
    assert "-- /bin/bash" in line
    app.wait(lambda: framed(app, C["ansi"][3]) > 50, msg="yellow frame")
    app.snap("a sandboxed tab: shield icon and frame")
    # the shell inside works and is in the profile's folder
    pane = str(next(t["id"] for t in tabs(app) if t["active"]))
    send(app, "pwd > $T2_TMP/where", pane)
    assert app.wait_file("where").strip() == "/tmp"


@pytest.mark.parametrize("sandbox,present,absent", [
    ({"network": True, "write": ["cwd"], "tmp": "private"}, ["--tmpfs"], ["--unshare-net"]),
    ({"network": False, "write": [], "tmp": "shared"}, ["--unshare-net"], ["--tmpfs", "--bind /tmp /tmp"]),
])
def test_sandbox_rules_follow_the_profile(app_factory, sandbox, present, absent):
    stub = stub_bwrap()
    app = boxed(app_factory, sandbox, f"{stub}:/usr/bin:/bin")
    assert msg(app, "new-tab", "--profile", "box").returncode == 0
    line = " ".join(app.wait_file("bwrap_args").split("\n"))
    for part in present:
        assert part in line, (part, line)
    for part in absent:
        assert part not in line, (part, line)


def test_sandbox_without_bubblewrap_refuses_to_open(app_factory):
    folder = Path(tempfile.mkdtemp(prefix="e2e-nobwrap-"))
    app = boxed(app_factory, STRICT, str(folder))
    done = msg(app, "new-tab", "--profile", "box")
    assert done.returncode != 0
    assert "bubblewrap" in done.stderr + done.stdout
    assert len(tabs(app)) == 1


def test_real_sandbox_blocks_writes_outside_and_the_network(app_factory):
    """only where bubblewrap can make namespaces, rootless containers often can't"""
    check = subprocess.run(["bwrap", "--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc", "--unshare-net",
                            "true"], capture_output=True)
    if check.returncode != 0:
        pytest.skip(f"bubblewrap can't run here: {check.stderr.decode().strip()}")
    work = Path(tempfile.mkdtemp(prefix="e2e-box-"))
    profile = {"name": "box", "command": {"program": "/bin/bash"}, "env": {"PS1": "$ "},
               "working_directory": str(work), "sandbox": {"network": False, "write": ["cwd"], "tmp": "private"}}
    settings = integrated()
    settings["profiles"] = settings["profiles"] + [profile]
    app = app_factory(settings)
    listening(app)
    assert msg(app, "new-tab", "--profile", "box", "--name", "box").returncode == 0
    ready(app, "box")
    send(app, f"touch {work}/in; touch {app.home}/out 2>/dev/null; "
              f"python3 -c 'import socket; socket.create_connection((\"1.1.1.1\", 53), 2)' 2>/dev/null; "
              f"echo net=$? > {work}/result", "box")
    app.wait(lambda: (work / "result").exists(), msg="sandbox result")
    assert (work / "in").exists() and not (app.home / "out").exists()
    assert (work / "result").read_text().strip() != "net=0"
