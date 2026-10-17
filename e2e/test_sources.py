"""who typed this: every command knows what typed it (keyboard, paste, palette, broadcast,
kuterm msg, run links and the queue, mcp agents), query tells it, and a mark in a gutter left of
the grid shows it as "shell_integration.mark_sources" says"""

import json
import subprocess
import time

import pytest

from harness import BIN, App, as_rgb, near, set_clipboard, unique_theme
from kit import integrated, listening, msg, send, socket_of, wait_block, wait_buttons, wait_text

FEATURE = "who typed this"

THEME = unique_theme(0x95)
C = as_rgb(THEME)


def source_app(app_factory, mode: str = "others", **extra) -> App:
    keys = {"split_right": "ctrl-shift-r", "toggle_broadcast_all": "ctrl-shift-b"}
    app = app_factory(integrated(shell_integration={"mark_sources": mode}, **extra), themes={"dark": THEME},
                      keybindings=keys,
                      commands={"commands": [{"name": "macro it", "actions": [{"type": "echo from_macro\n"}]}]})
    # typed, so "others" puts no mark on it
    listening(app)
    app.run("true")
    wait_block(app, "true")
    app.mouse(450, 300)
    return app


def columns_seen(app: App, name: str, remote: bool = False) -> int:
    """columns the shell sees, asked by the keyboard or over remote control"""
    line = f"stty size > $T2_TMP/{name}.part && mv $T2_TMP/{name}.part $T2_TMP/{name}"
    if remote:
        send(app, line)
    else:
        app.run(line)
    return int(app.wait_file(name).split()[1])


def test_query_tells_what_typed_each_command(app_factory):
    app = source_app(app_factory)
    app.run("echo by_keys")
    assert wait_block(app, "echo by_keys")["source"] == ["keyboard"]
    send(app, "echo by_remote")
    assert wait_block(app, "echo by_remote")["source"] == ["remote"]
    app.palette("macro it")
    assert wait_block(app, "echo from_macro")["source"] == ["macro"]
    set_clipboard("echo by_paste")
    app.key("ctrl+shift+v")
    app.key("Return")
    assert wait_block(app, "echo by_paste")["source"] == ["keyboard", "paste"]
    assert msg(app, "queue", "echo by_queue").returncode == 0
    assert wait_block(app, "echo by_queue")["source"] == ["automation"]


def test_a_mixed_line_names_every_source(app_factory):
    app = source_app(app_factory)
    # the remote half waits on the line, the keyboard ends it
    assert msg(app, "send-text", "echo mixed_").returncode == 0
    time.sleep(0.5)
    app.type("line")
    app.key("Return")
    assert wait_block(app, "echo mixed_line")["source"] == ["keyboard", "remote"]


def test_broadcast_input_is_tagged(app_factory):
    app = source_app(app_factory)
    app.key("ctrl+shift+t")
    app.wait(lambda: len(json.loads(msg(app, "list-tabs", "--json").stdout)) == 2, msg="second tab")
    time.sleep(1)
    app.key("ctrl+shift+r")
    time.sleep(1)
    app.key("ctrl+shift+b")
    time.sleep(0.5)
    app.run("echo everywhere")
    ids = [str(t["id"]) for t in json.loads(msg(app, "list-tabs", "--json").stdout)]
    sources = sorted(tuple(wait_block(app, "echo everywhere", pane)["source"]) for pane in ids)
    assert sources == [("broadcast",), ("keyboard",)], sources


def test_a_run_link_is_typed_by_kuterm(app_factory):
    app = source_app(app_factory)
    subprocess.run([str(BIN), "kuterm://run?cmd=echo%20from_link"],
                   env={**app.app_env(), "KUTERM_SOCKET": str(socket_of(app))}, timeout=10, check=True)
    app.wait(lambda: len(json.loads(msg(app, "list-tabs", "--json").stdout)) == 2, msg="link tab")
    # the text is typed once the shell of the new tab is up: its echo shows it arrived
    wait_text(app, "echo from_link")
    app.key("Return")
    assert wait_block(app, "echo from_link")["source"] == ["keyboard", "automation"]


def test_mcp_runs_are_tagged_mcp(app_factory):
    app = source_app(app_factory)
    mcp = subprocess.Popen([str(BIN), "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
                           env={**app.app_env(), "KUTERM_SOCKET": str(socket_of(app))})
    try:
        def call(message: dict) -> None:
            mcp.stdin.write(json.dumps(message) + "\n")
            mcp.stdin.flush()

        call({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}})
        mcp.stdout.readline()
        call({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
              "params": {"name": "run", "arguments": {"command": "echo by_agent"}}})
        buttons = wait_buttons(app, C, 3)
        app.click(*buttons[0])
        assert json.loads(mcp.stdout.readline())["id"] == 2
        assert wait_block(app, "echo by_agent")["source"] == ["mcp"]
        app.snap("the agent's command marked with a robot")
    finally:
        mcp.kill()


def test_others_mode_marks_only_what_the_keyboard_did_not_type(app_factory):
    app = source_app(app_factory, "others")
    plain = columns_seen(app, "typed")
    # nothing typed by someone else yet, so no gutter
    assert columns_seen(app, "typed_again") == plain
    columns_seen(app, "remote", remote=True)
    app.wait(lambda: columns_seen(app, f"after{time.monotonic_ns()}") == plain - 2, msg="gutter took two columns")
    app.snap("the command sent over remote control has a mark")


def test_all_mode_marks_typed_commands_too(app_factory):
    plain = columns_seen(source_app(app_factory, "none"), "plain")
    app = source_app(app_factory, "all")
    # the first typed command is marked, so the gutter is there for the next one
    assert columns_seen(app, "next") == plain - 2
    app.snap("every command has a mark, a person for typed ones")


def test_none_mode_never_marks(app_factory):
    app = source_app(app_factory, "none")
    plain = columns_seen(app, "plain")
    columns_seen(app, "remote", remote=True)
    app.palette("macro it")
    wait_block(app, "echo from_macro")
    assert columns_seen(app, "after") == plain


def test_hovering_a_mark_shows_its_tooltip(app_factory):
    app = source_app(app_factory, "others")
    send(app, "echo tooltip_me")
    wait_block(app, "echo tooltip_me")
    time.sleep(1.5)
    img = app.shot()
    # the mark is in the gutter, on the row of its prompt: find its ink left of the text
    x = round(app._padding()[0] * app.scale) + 2
    rows = (near(img[:, x : x + round(app.cell_width() * 2) - 4], C["terminal_background"], 10).all(axis=1) == 0)
    ys = rows.nonzero()[0]
    ys = ys[ys > app.bar_height()]
    assert len(ys), "no mark in the gutter"
    before = int(near(img, C["tab_bar_background"]).sum())
    app.mouse(x + 4, int(ys[0]) + 4)
    app.wait(lambda: near(app.shot(), C["tab_bar_background"]).sum() > before + 200, msg="tooltip box")
    app.snap("the tooltip of the mark says what typed the command")


@pytest.mark.parametrize("mode", ["all", "others", "none"])
def test_marks_follow_scrollback(app_factory, mode):
    """lots of output pushes marked commands into history, the app keeps working"""
    app = source_app(app_factory, mode)
    send(app, "seq 1 300")
    wait_block(app, "seq 1 300")
    app.wheel(450, 300, up=True, clicks=10)
    time.sleep(0.5)
    app.snap(f"scrolled back with mark_sources {mode}")
    send(app, "echo still_alive")
    wait_block(app, "echo still_alive")
