"""who typed this: every command knows what typed it (keyboard, paste, palette, kuterm msg, run
links and the queue) and query tells it"""

import json
import subprocess
import time

from harness import BIN, App, set_clipboard, unique_theme
from kit import integrated, listening, msg, send, socket_of, wait_block, wait_text

FEATURE = "who typed this"

THEME = unique_theme(0x95)


def source_app(app_factory, **extra) -> App:
    app = app_factory(integrated(**extra), themes={"dark": THEME},
                      commands={"commands": [{"name": "macro it", "actions": [{"type": "echo from_macro\n"}]}]})
    listening(app)
    app.run("true")
    wait_block(app, "true")
    app.mouse(450, 300)
    return app


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


def test_a_run_link_is_typed_by_kuterm(app_factory):
    app = source_app(app_factory)
    subprocess.run([str(BIN), "kuterm://run?cmd=echo%20from_link"],
                   env={**app.app_env(), "KUTERM_SOCKET": str(socket_of(app))}, timeout=10, check=True)
    app.wait(lambda: len(json.loads(msg(app, "list-tabs", "--json").stdout)) == 2, msg="link tab")
    # the text is typed once the shell of the new tab is up: its echo shows it arrived
    wait_text(app, "echo from_link")
    app.key("Return")
    assert wait_block(app, "echo from_link")["source"] == ["keyboard", "automation"]
