# kuterm mcp server

an example mcp server that drives a running kuterm through its automation socket. it speaks
json-rpc on stdin and stdout, one message per line, and turns each tool call into one request
on the unix socket. `docs/MCP.md` describes the socket and the mapping.

## setup

needs python 3.10 or newer, nothing else. kuterm must have remote control on, in
`~/.config/kuterm/settings.jsonc`:

```jsonc
"remote_control": "socket",
```

then run it with an mcp client. for claude code:

```bash
claude mcp add kuterm -- python3 /path/to/kuterm/examples/mcp/kuterm_mcp.py
```

or in a client that reads json, like claude desktop:

```json
{
  "mcpServers": {
    "kuterm": {
      "command": "python3",
      "args": ["/path/to/kuterm/examples/mcp/kuterm_mcp.py"]
    }
  }
}
```

`KUTERM_SOCKET` picks the kuterm to talk to, like in a shell started by one. without it the
newest running kuterm with the socket on is used.

## tools

- `list_tabs`: open tabs, their ids, titles, folders and windows
- `get_text`: what a pane shows, or its whole scrollback
- `query`: the commands a pane ran, with output and exit codes
- `run`: run a command; kuterm asks the user to allow it in the window
- `queue`: run a command once the prompt comes back
- `new_tab`: open a tab, optionally in a folder, with a profile or a name
- `wait_for_event`: wait for the next pane event, like `command_finished`

`run` and `queue` are the way to run commands: the user allows them first, and `query` shows
what they printed. typing text without asking (the socket's `send_text`) is left out on purpose.
