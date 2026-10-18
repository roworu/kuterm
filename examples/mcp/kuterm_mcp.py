#!/usr/bin/env python3
"""an mcp server for a running kuterm, over its automation socket.

kuterm needs "remote_control": "socket" in settings.jsonc. this speaks json-rpc on stdin and
stdout, one message per line, and turns each tool call into one request on the unix socket.
$KUTERM_SOCKET picks the kuterm to talk to, otherwise the newest running one is used.
see docs/MCP.md for the socket and the protocol, and examples/mcp/README.md for setup.
"""
import glob
import json
import os
import socket
import sys
import tempfile

PROTOCOL_VERSION = "2024-11-05"
SOCKET_TIMEOUT = 5.0
PANE = {"type": "string", "description": "tab id, name or title, the active tab without one"}


class KutermError(Exception):
    """the request could not be sent or kuterm answered with an error"""


def runtime_dir() -> str:
    """folder sockets live in: $XDG_RUNTIME_DIR, the temp folder without it"""
    path = os.environ.get("XDG_RUNTIME_DIR")
    return path if path and os.path.isabs(path) else tempfile.gettempdir()


def sockets() -> list[str]:
    """sockets of running kuterms, newest first"""
    chosen = os.environ.get("KUTERM_SOCKET")
    if chosen:
        return [chosen]
    found = glob.glob(os.path.join(runtime_dir(), "kuterm-*.sock"))

    def modified(path: str) -> float:
        try:
            return os.path.getmtime(path)
        except OSError:
            return 0.0

    return sorted(found, key=modified, reverse=True)


def request(payload: dict, timeout: float | None = SOCKET_TIMEOUT) -> dict:
    """send one request to the newest kuterm that answers"""
    last_error: Exception | None = None
    for path in sockets():
        try:
            with socket.socket(socket.AF_UNIX) as conn:
                conn.settimeout(timeout)
                conn.connect(path)
                conn.sendall((json.dumps(payload) + "\n").encode())
                with conn.makefile("r", encoding="utf-8") as reader:
                    line = reader.readline()
            if not line:
                raise KutermError("kuterm closed the socket")
            return json.loads(line)
        except (OSError, ValueError) as error:
            last_error = error
    detail = f" ({last_error})" if last_error else ""
    raise KutermError('no running kuterm with "remote_control": "socket" found' + detail)


def wait_for_event(events: list[str], timeout: float) -> dict:
    """subscribe and return the first event, then close the connection"""
    payload = {"cmd": "subscribe"}
    if events:
        payload["events"] = events
    last_error: Exception | None = None
    for path in sockets():
        conn = socket.socket(socket.AF_UNIX)
        try:
            conn.settimeout(timeout)
            conn.connect(path)
            conn.sendall((json.dumps(payload) + "\n").encode())
            with conn.makefile("r", encoding="utf-8") as reader:
                reply = json.loads(reader.readline())
                if not reply.get("ok"):
                    raise KutermError(reply.get("error") or "subscribe failed")
                line = reader.readline()
            if not line:
                raise KutermError("kuterm closed the socket")
            return json.loads(line)
        except TimeoutError:
            raise KutermError(f"no event within {timeout:g}s") from None
        except (OSError, ValueError) as error:
            last_error = error
        finally:
            conn.close()
    detail = f" ({last_error})" if last_error else ""
    raise KutermError('no running kuterm with "remote_control": "socket" found' + detail)


def tool(name: str, description: str, properties: dict | None = None,
         required: list[str] | None = None) -> dict:
    schema: dict = {"type": "object", "properties": properties or {}}
    if required:
        schema["required"] = required
    return {"name": name, "description": description, "inputSchema": schema}


TOOLS = [
    tool("list_tabs", "open tabs of every window, with their id, title, folder and window"),
    tool("get_text", "what a pane shows now, or its whole scrollback",
         {"pane": PANE, "scrollback": {"type": "boolean",
                                       "description": "the whole history instead of the screen"}}),
    tool("query", "the commands a pane ran, with their output and exit codes",
         {"pane": PANE,
          "block": {"type": "integer",
                    "description": "1 based block number, negative counts back from the last "
                                   "finished one"},
          "all": {"type": "boolean", "description": "every block instead of one"}}),
    tool("run", "run a command in a pane; kuterm asks the user to allow it in the window",
         {"pane": PANE, "command": {"type": "string"}}, ["command"]),
    tool("queue", "run a command once the pane's prompt comes back, after the ones queued before",
         {"pane": PANE, "command": {"type": "string"}}, ["command"]),
    tool("new_tab", "open a new tab in the active window",
         {"cwd": {"type": "string"}, "profile": {"type": "string"},
          "name": {"type": "string", "description": "name later tool calls find the tab by"}}),
    tool("wait_for_event", "wait for the next event of a pane, like a finished command",
         {"events": {"type": "array", "items": {"type": "string"},
                     "description": 'event names like "command_finished", all without it'},
          "timeout": {"type": "number", "description": "seconds, 30 by default"}}),
]


def call_tool(name: str, args: dict) -> tuple[dict, str]:
    """one tool, one socket request, and the text shown to the agent"""
    if name == "list_tabs":
        reply = request({"cmd": "list_tabs"})
        return reply, json.dumps(reply.get("tabs", []), indent=2)
    if name == "get_text":
        payload = {"cmd": "get_text", "scrollback": bool(args.get("scrollback"))}
        if args.get("pane"):
            payload["pane"] = args["pane"]
        reply = request(payload)
        text = reply.get("text") or ""
        if reply.get("hidden"):
            text += "\n\n[concealed text was left out]"
        return reply, text
    if name == "query":
        payload = {"cmd": "query", "all": bool(args.get("all"))}
        if args.get("pane"):
            payload["pane"] = args["pane"]
        if args.get("block") is not None:
            payload["block"] = int(args["block"])
        reply = request(payload)
        return reply, json.dumps(reply.get("blocks", []), indent=2)
    if name == "run":
        # no timeout: it waits for the user to allow or deny the command
        reply = request({"cmd": "run", "command": args["command"],
                         **({"pane": args["pane"]} if args.get("pane") else {})}, timeout=None)
        return reply, "sent to the pane" if reply.get("ok") else (reply.get("error") or "failed")
    if name == "queue":
        payload = {"cmd": "queue", "command": args["command"]}
        if args.get("pane"):
            payload["pane"] = args["pane"]
        reply = request(payload)
        return reply, "queued" if reply.get("ok") else (reply.get("error") or "failed")
    if name == "new_tab":
        payload = {"cmd": "new_tab"}
        for key in ("cwd", "profile", "name"):
            if args.get(key):
                payload[key] = args[key]
        reply = request(payload)
        return reply, "opened" if reply.get("ok") else (reply.get("error") or "failed")
    if name == "wait_for_event":
        events = [str(event) for event in args.get("events") or []]
        timeout = float(args.get("timeout", 30))
        event = wait_for_event(events, timeout)
        return {"ok": True}, json.dumps(event, indent=2)
    raise KutermError(f"unknown tool {name!r}")


def missing(tool: dict, args: dict) -> str | None:
    """name of the first required argument that is not there"""
    for name in tool["inputSchema"].get("required", []):
        if args.get(name) is None:
            return name
    return None


def rpc_error(identifier, code: int, message: str) -> dict:
    """a json-rpc error answer"""
    return {"jsonrpc": "2.0", "id": identifier, "error": {"code": code, "message": message}}


def answer(message: dict) -> dict | None:
    """one json-rpc message in, one out, none for notifications"""
    identifier = message.get("id")
    method = message.get("method")
    params = message.get("params") or {}

    if identifier is None:
        return None  # notifications like initialized never get an answer
    if method == "initialize":
        result = {
            "protocolVersion": params.get("protocolVersion") or PROTOCOL_VERSION,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "kuterm", "version": "1"},
        }
    elif method == "ping":
        result = {}
    elif method == "tools/list":
        result = {"tools": TOOLS}
    elif method == "tools/call":
        name = params.get("name")
        args = params.get("arguments") or {}
        found = next((tool for tool in TOOLS if tool["name"] == name), None)
        if found is None:
            return rpc_error(identifier, -32602, f"unknown tool {name!r}")
        if (required := missing(found, args)) is not None:
            return rpc_error(identifier, -32602, f"{name} needs {required!r}")
        try:
            reply, text = call_tool(name, args)
            result = {"content": [{"type": "text", "text": text}],
                      "isError": not reply.get("ok", False)}
        except KutermError as error:
            result = {"content": [{"type": "text", "text": str(error)}], "isError": True}
    else:
        return rpc_error(identifier, -32601, f"unknown method {method!r}")
    return {"jsonrpc": "2.0", "id": identifier, "result": result}


def main() -> None:
    """serve json-rpc on stdin and stdout until stdin closes"""
    sys.stdin.reconfigure(encoding="utf-8")
    sys.stdout.reconfigure(encoding="utf-8")
    for line in sys.stdin:
        if not line.strip():
            continue
        try:
            message = json.loads(line)
        except ValueError as error:
            print(json.dumps(rpc_error(None, -32700, f"invalid json: {error}")), flush=True)
            continue
        if not isinstance(message, dict):
            print(json.dumps(rpc_error(None, -32600, "invalid request")), flush=True)
            continue
        try:
            response = answer(message)
        except Exception as error:  # keep the stream alive, the agent sees the failure
            print(f"kuterm mcp: {error}", file=sys.stderr, flush=True)
            response = rpc_error(message.get("id"), -32603, f"internal error: {error}")
        if response is not None:
            print(json.dumps(response), flush=True)


if __name__ == "__main__":
    main()
