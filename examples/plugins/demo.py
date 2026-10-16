#!/usr/bin/env python3
"""kuterm demo plugin: a palette command, a tab title block, a link pattern and a panel.

add it to settings.jsonc:

    "plugins": [
      { "name": "demo", "command": ["python3", "/path/to/demo.py"],
        "permissions": { "panes": true, "run": true } },
    ],
    "tab_title": ["icon", "folder", { "text": " " }, { "plugin": "demo/commands" }],
"""

import json
import sys

commands_seen = 0


def send(message):
    body = json.dumps(message).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()


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


def notify(method, params):
    send({"jsonrpc": "2.0", "method": method, "params": params})


next_id = 100


def request(method, params):
    global next_id
    next_id += 1
    send({"jsonrpc": "2.0", "id": next_id, "method": method, "params": params})


while True:
    message = read()
    if message is None:
        break
    method = message.get("method")
    params = message.get("params") or {}
    if method == "initialize":
        send({
            "jsonrpc": "2.0",
            "id": message["id"],
            "result": {
                "commands": [{"name": "say hello", "category": "demo"}],
                "title_blocks": ["commands"],
                "patterns": [{"name": "ticket", "regex": "TICKET-[0-9]+"}],
                "panels": [{"name": "top", "command": ["top"]}],
            },
        })
        notify("set_title_block", {"name": "commands", "text": "0 cmds"})
    elif method == "command":
        notify("notify", {"text": "hello from the demo plugin"})
    elif method == "pattern":
        notify("notify", {"text": "clicked " + params.get("text", "")})
    elif method == "event" and params.get("event") == "command_finished":
        commands_seen += 1
        notify("set_title_block", {"name": "commands", "text": f"{commands_seen} cmds"})
        # mark the block with its exit code, needs "panes"
        request("decorate", {
            "pane": params["pane"],
            "block": params.get("block"),
            "icon": "✓" if params.get("exit") == 0 else "✗",
            "tooltip": f"exit {params.get('exit')}",
        })
