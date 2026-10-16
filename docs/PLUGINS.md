# plugins

a kuterm plugin is a program listed in `"plugins"` in settings. kuterm starts it, talks to it
over its stdin and stdout with json-rpc 2.0, and starts it again when it exits. messages are
framed like the language server protocol: a `Content-Length: <bytes>` header, an empty line
(`\r\n\r\n`), then the json. stderr goes where kuterm's goes, so print logs there.

see `examples/plugins/demo.py` for a complete one.

## settings

```jsonc
"plugins": [
  // a program and its arguments, with what it may do
  { "name": "demo", "command": ["python3", "~/plugins/demo.py"],
    "permissions": { "panes": true, "pty_write": false, "run": false, "clipboard": false } },
  // a git repository with a kuterm-plugin.json manifest, cloned into
  // $XDG_DATA_HOME/kuterm/plugins
  "github:user/repo",
  "github:user/repo@v1.2",
]
```

the manifest of a repository has the same keys as a local entry: `name`, `command` (run in
the repository folder) and `permissions`.

before a plugin first runs kuterm asks, showing its command and permissions. the answer is
kept with a hash of the command, the permissions and the files the command names, so a
changed script or wider permissions ask again. every plugin restarts when the config is
reloaded.

### permissions

all are off unless named:

- `panes`: get every event (see below), read pane text and commands (`get_text`, `query`,
  `list_tabs`), change tabs (`focus_tab`, `set_tab_title`, ...) and mark blocks (`decorate`)
- `pty_write`: type into panes (`send_text`, `run`, `queue`, `run_command`)
- `run`: start programs (`new_tab`, `new_window`, `open_url`, `open_panel`)
- `clipboard`: reserved, no request uses it yet

## kuterm to plugin

- request `initialize` (`id` 0) with `{ "kuterm": "<version>", "permissions": {...} }`. the
  result tells what the plugin adds, every key may be left out:

  ```json
  {
    "commands": [{ "name": "say hello", "category": "demo" }],
    "title_blocks": ["commands"],
    "patterns": [{ "name": "ticket", "regex": "TICKET-[0-9]+" }],
    "panels": [{ "name": "top", "command": ["top"] }]
  }
  ```

  commands show in the palette; panels show as "open <name>" and open a tab running the
  command (needs `run`). patterns are link patterns, a click with the link modifier tells
  the plugin.
- notification `command` `{ "name": "...", "pane": 3 }`: a palette command was picked
- notification `pattern` `{ "name": "ticket", "text": "TICKET-12", "pane": 3 }`
- notification `event` with an event of the bus, the same json `kuterm msg subscribe --json`
  prints, like `{ "event": "command_finished", "pane": 3, "block": 12, "exit": 0,
  "seconds": 1.5 }` (needs `panes`)
- responses to the plugin's requests

## plugin to kuterm

kuterm never waits for a plugin: requests are answered when their answer is ready, and a
plugin that stops reading only fills its own pipe.

- `kuterm/request`: the params are a request of the remote control socket, exactly as
  `kuterm msg` sends them, like `{ "cmd": "send_text", "text": "ls\r", "pane": "3" }` or
  `{ "cmd": "query", "all": true }`. the result is the reply json. `run` still asks the user.
- `set_title_block` `{ "name": "commands", "text": "3 cmds", "pane": 3 }`: text of a tab title
  block, written `{ "plugin": "demo/commands" }` in `tab_title`, `window_title` or the status
  bar. without `pane` it is the text for every pane
- `decorate` `{ "pane": 3, "block": 12, "icon": "✓", "tooltip": "exit 0" }`: a mark left of a
  block's command, the last block without `block` (needs `panes`)
- `open_panel` `{ "command": ["htop"], "name": "top" }`: a tab running the command (needs
  `run`)
- `notify` `{ "text": "..." }`: a notification in the window

## not yet

- automation conditions and actions, until kuterm has automations
- underlines and tooltips on cell ranges, `decorate` only puts marks next to blocks
- wasm plugins
