# MCP over the socket

kuterm has no mcp server of its own. an agent can still drive it: a small mcp server that talks to kuterm's unix socket, the same one `kuterm msg` uses, is all it takes. this page is the reference for writing one.

## the socket

- turn it on with `"remote_control": "socket"` in settings.jsonc.
- it is `$XDG_RUNTIME_DIR/kuterm-<pid>.sock` (the temp folder without `$XDG_RUNTIME_DIR`). shells started by kuterm get its path in `$KUTERM_SOCKET`, and `$KUTERM_PANE` is the id of their own tab.
- one connection per request: write one json object and a newline, read one json line back. a request that sends nothing for 5 seconds is dropped.
- `subscribe` keeps the connection: after `{"ok":true}` comes one json line per event, until the client closes it.

requests (`"cmd"` picks one, `pane` is a tab id, name or title, the active tab when left out):

| request | fields | answer |
| --- | --- | --- |
| `list_tabs` | | `tabs`: id, name, window, pane, number, title, active, cwd |
| `get_text` | `pane`, `scrollback` | `text`, `hidden` when concealed text was left out |
| `query` | `pane`, `block` (1 based, negative counts back), `all` | `blocks`: command, exit, cwd, started, seconds, output, source |
| `run` | `pane`, `command` | waits until the user allows or denies it in the window |
| `send_text` | `pane`, `text` | typed right away, refused while a program asks for a password |
| `queue` | `pane`, `command` | typed once the prompt comes back |
| `new_tab` | `cwd`, `profile`, `name` | |
| `subscribe` | `events`, like `["command_finished", "bell"]`, all without it | event lines |

every answer has `ok`, and `error` when it is false. `src/automation/protocol.rs` has every request and field, `src/automation/events.rs` every event.

## mapping it to mcp tools

an mcp server speaks json-rpc over stdin and stdout, one message per line. answer `initialize` with `{"capabilities": {"tools": {}}}`, `tools/list` with the tools, and turn each `tools/call` into one socket request:

- `query`, `get_text` and `list_tabs` give the agent what the panes ran and show.
- `run` lets it run commands. it is the one to offer for that: kuterm asks the user first, every time or once per tab. avoid `send_text` for agents, it types without asking.
- a `subscribe` tool waits for the next event with a timeout, closing the connection when it returns, so kuterm drops the subscription.

a minimal server in python:

```python
import json, os, socket, sys

def kuterm(request: dict) -> dict:
    with socket.socket(socket.AF_UNIX) as conn:
        conn.connect(os.environ["KUTERM_SOCKET"])
        conn.sendall((json.dumps(request) + "\n").encode())
        return json.loads(conn.makefile().readline())

TOOLS = [
    {"name": "query", "description": "commands a kuterm pane ran, with their output",
     "inputSchema": {"type": "object", "properties": {"pane": {"type": "string"}, "all": {"type": "boolean"}}}},
    {"name": "run", "description": "run a command in a kuterm pane once the user allows it",
     "inputSchema": {"type": "object", "properties": {"pane": {"type": "string"}, "command": {"type": "string"}},
                     "required": ["command"]}},
]

for line in sys.stdin:
    message = json.loads(line)
    if "id" not in message:
        continue
    method, params = message.get("method"), message.get("params", {})
    if method == "initialize":
        result = {"protocolVersion": params.get("protocolVersion"), "capabilities": {"tools": {}},
                  "serverInfo": {"name": "kuterm", "version": "1"}}
    elif method == "tools/list":
        result = {"tools": TOOLS}
    elif method == "tools/call":
        reply = kuterm({"cmd": params["name"], **params.get("arguments", {})})
        text = reply.get("error") or json.dumps(reply.get("blocks") or reply.get("text") or "done")
        result = {"content": [{"type": "text", "text": text}], "isError": not reply["ok"]}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": result}), flush=True)
```

commands run this way show `"remote"` as their source in `query`.
