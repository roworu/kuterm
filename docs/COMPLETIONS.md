# completions

completions for the command line while typing, shown in a native gpui popup under the typed
word, like fig. the first part is for an agent (or a person) adding or refreshing the
completions kuterm brings for a command, the second explains how the popup and engine work.

## part 1: adding completion specs

### rules

- **never run the programs on the host.** the generator runs every program it describes, so it
  only ever runs inside a throwaway podman container: `tools/completions/generate.sh` starts
  one, installs the programs, writes the json and removes the container. the container only
  sees `tools/completions/` (read only) and `assets/completions/` (where specs are written).
- **specs are data, never code.** fig's `generators`, `script`, `custom` and function valued
  fields are ignored by kuterm, so don't add them; they would only bloat the file. anything
  that would need running a command to know (package names, git branches, docker containers)
  stays without suggestions.
- **build specs from the real program**, its `--help` (or `help <cmd>`) in the container, not
  from memory. write by hand only what the help can't give: argument kinds, value
  suggestions, priorities, and tools whose help isn't in a parseable shape.
- one file per command: `assets/completions/<command>.json`, `"name"` equal to the file name.

### the spec format

fig's spec format as json ([fig docs](https://fig.io/docs/reference/subcommand)), the subset
kuterm reads (`src/completion/spec.rs`):

```json
{
  "name": "tool",
  "description": "One line, capitalized, no final period",
  "subcommands": [
    { "name": ["remove", "rm"], "description": "Remove things", "priority": 76,
      "args": { "name": "file", "template": "filepaths", "isVariadic": true },
      "options": [ ... ], "subcommands": [ ... ] },
    { "name": "big", "loadSpec": "tool/big" }
  ],
  "options": [
    { "name": ["-v", "--verbose"], "description": "Say more", "isPersistent": true, "isRepeatable": true },
    { "name": "--color", "args": { "name": "when", "isOptional": true, "suggestions": ["always", "never"] } },
    { "name": "--out", "args": { "name": "dir", "template": "folders" } }
  ],
  "args": [{ "name": "first", "suggestions": [{ "name": "a", "description": "The a value" }] }, { "name": "rest", "isVariadic": true }]
}
```

- `priority` 0 to 100, 50 when left out: give the few subcommands people type most 75 or 76,
  they are listed first. the generator does it for the names in a recipe's `top`.
- an option whose argument isn't optional takes the next word, so set `args` only when it
  really takes one, and `"isOptional": true` for `--opt[=VALUE]` forms.
- `template`: `"filepaths"` (files and folders) or `"folders"`; args without one complete
  nothing, except their `suggestions`.
- `isPersistent` options are also taken after subcommands, use it for global options.
- `hidden` hides a name until it is typed whole.
- `loadSpec` points a subcommand at another spec, only used by bundles (below).

### generating specs

recipes live in `tools/completions/generate.py`, grouped by the image their programs come from
(`ubuntu`, `fedora`, `arch`, `opensuse`, `alpine`). each group has the image, an install
command and recipes by command name. a recipe calls `helpspec.spec(...)`, which runs the help
and parses it:

- option lines: `-a, --all  description`, `--file=FILE`, `-o <file>`, `--color[=WHEN]`,
  systemd's `-M --machine=NAME`, zypper's `--config, -c <FILE>`. arguments named like files or
  folders get a template.
- commands: `  name   description`, `  name ARGS...   description`, apt's
  `  name - description`, zypper's `  repos, lr   description` (aliases), under the headers
  `sections` (a regex) matches.
- `sub_help=("{}", "--help")` also parses each subcommand's own help for its options.
- `option_overrides` / `sub_overrides` fix or drop entries by name (`None` drops),
  `extra_options` / `extra_subcommands` add what the help doesn't list, `columns=True` handles
  help with two options per line (zip, unzip), `persistent=True` marks the top options global.

#### add a command

1. find the image that ships it. ubuntu is the default, use another distro's image only for
   that distro's own tools (dnf and rpm from fedora, pacman from arch, zypper from opensuse,
   apk from alpine).
2. add the package to that group's `install` line and a recipe to its `recipes`, for example

   ```python
   "jq": simple("jq", "Command-line JSON processor", args=[{"name": "filter"}, FILES]),
   "kubectl": lambda: spec("kubectl", "Control Kubernetes clusters", sections=r"commands",
                           sub_help=("{}", "--help"), top=("get", "apply", "describe", "logs")),
   ```

   look at the raw help first, inside the container:

   ```sh
   podman run --rm -it docker.io/library/ubuntu:26.04 sh -c 'apt-get update && apt-get install -y jq && jq --help'
   ```

3. generate only that command: `sh tools/completions/generate.sh ubuntu jq`
4. read the json it wrote. check that subcommands aren't missing (a wrong `sections` regex is
   the usual cause), descriptions aren't cut mid-word nonsense, options that take a value have
   `args`, and files or folders have a template. fix it in the recipe, not in the json, so the
   next regeneration keeps the fix.
5. add the file to `BUNDLED` in `src/completion/spec.rs`, sorted. the test
   `every_bundled_spec_is_listed_and_parses` fails until every file is listed and parses.
6. run the tests (`cargo test completion` in the dev container, see AGENTS.md) and, for a
   visible check, `sh e2e/run.sh -k autocomplete`.

tools whose help is only a usage line or isn't in columns (find, ssh, scp, less, ip, man,
which, and the older git, cargo, docker, npm, cd) are written by hand: write the json directly,
from the program's man page or `--help` read in the container, and keep it in the same style.

#### refresh after an upgrade

`sh tools/completions/generate.sh <image>` regenerates a whole group. review `git diff
assets/completions`: removed options are fine when the program dropped them, a spec that
shrank a lot means the help format changed and the recipe needs a fix.

### big specs: bundles

a command with hundreds of subcommands would be megabytes of json, read on every completion.
those ship as a bundle instead: `assets/completions/<command>.jsonl.gz`, gzipped json lines.
the first line is the command's spec, its subcommands carry only `name`, `description` and
`"loadSpec": "<command>/<sub>"`; every following line is one such spec, starting with
`{"name":"<command>/<sub>",`. kuterm unpacks the bundle when the command is first completed and
parses a line only when its subcommand is typed. list it in `BUNDLES` in `spec.rs`.

aws is the one bundle now: `sh tools/completions/generate.sh aws` installs the aws cli in a
container and walks its own command tables (`tools/completions/aws.py`): every service, its
operations with their first help sentence, their options, and fixed values of up to 20 choices.
option descriptions are only kept for the global options, they would triple the size.

## part 2: how completions work

fig is the reference for the ux and the spec format, microsoft's inshellisense for the engine.

goals: suggest commands, paths, options, subcommands and arguments with descriptions, filter
them as the line changes, navigate, accept and dismiss them with keys, and never get in the
way: with completion off or the shell unsupported, the terminal works as before. the engine
knows nothing about the ui, and the shell stays the owner of its line: kuterm never types a
completion as keystrokes, the shell puts it in with its own line editor.

### status

| phase | state | notes |
| --- | --- | --- |
| 0. inspect kuterm and inshellisense | done | this note. inshellisense's engine design is ported to rust, its node code and pty wrapper are not used |
| 1. zsh prototype | done | `autocomplete.enable`, zle hooks, private socket per tab, popup, accept edits the live `BUFFER` |
| 2. editing and lifecycle | done | cursor mid line, quoting and spaces, stale and out of order answers dropped on both sides, nested shells, restarts, tab close; a closed socket can't kill zsh. multiline buffers complete the current line only |
| 3. engine and specs | done | fig json subset, lazy parsing, broken specs reported once, ~85 bundled specs generated from the real programs inside podman |
| 4. popup | done | anchored at the word, flips above, scrolling, descriptions, kinds in colors of the theme's palette, font zoom, mouse, configurable keys, `auto`/`key` trigger, empty state in key mode |
| 5. bash and fish | dropped | see below |
| 6. remote shells and containers | dropped | see below |
| 7. release | not started | on by default once it had real use, user docs, latency profiling |

bash, fish and remote shells were built once and taken out again. bash and fish can't open a
unix socket, and their line editors can't wake up when one has data, so they needed a second
channel through the terminal itself: messages in an osc, the line read off the screen after
the prompt mark, a key kuterm types to have the shell ask for the picked completion, and a
token to tell local shells from remote ones. remote shells only worked with kuterm's script
copied to the other side by hand. that was more code and more ways to go wrong than the
feature was worth; they keep working as plain terminals.

left:

- phase 7.
- dynamic suggestions (git branches, package names, containers, make targets): blocked on the
  sandboxed provider runner and trust model under "safety". no completion runs any command.
- more specs: kubectl, helm, terraform, gcloud, az, gh, jq and other developer tools,
  following part 1.
- specs are read once per kuterm process, editing one needs a restart.

### pieces

| piece | where |
| --- | --- |
| zsh adapter | `assets/shell/zsh/.zshenv`, inside kuterm's zsh injection (`ZDOTDIR`) |
| protocol | `src/completion/protocol.rs` |
| session bridge | `src/completion/bridge.rs`, same unix socket pattern as `src/automation/socket.rs` |
| engine | `src/completion/mod.rs`, `parse.rs`, `sources.rs`, modelled on inshellisense |
| specs | `src/completion/spec.rs`, `assets/completions/`, fig's format, a subset |
| spec generator | `tools/completions/`, runs programs' help inside podman |
| popup | `src/ui/terminal_view/completions.rs`, gpui `anchored` like the tab menus |
| settings, keys | `autocomplete` section, `complete` / `completion_*` actions |

the terminal model only carries the bridge (`Terminal::set_completion_bridge`,
`send_to_shell`, `Event::Shell`), so pty input and output are untouched.

### protocol, version 1

shell to kuterm, one json object per line:

```
{"v":1,"type":"hello","shell":"zsh"}
{"v":1,"type":"env","path":"<PATH>"}                     when PATH changed
{"v":1,"type":"line","seq":7,"cursor":3,"buffer":"git","cwd":"/abs"}   every edit, cursor in chars
{"v":1,"type":"line",...,"history":true}                  the edit came from moving through history
{"v":1,"type":"end"}                                      the line ran or was dropped
```

the history flag keeps the auto popup out of the way while moving through history: the next
up or down reaches the shell again, and only typing or the complete key brings the popup back.

kuterm to shell, plain text so zsh needs no json parser:

```
1 accept <seq> <start> <end> <text>      replace chars start..end, text runs to the end of the line
```

### engine and specs

inshellisense (MIT, typescript on node) wraps the shell in its own pty and reads the command
line from the screen. kuterm already owns the pty, and zle gives the exact buffer and cursor,
so its shell layer is not used. its engine (split the line into words, walk the spec through
subcommands, options and their arguments, then filter by prefix) is ported to rust. bundling
node for it would add a runtime for little gain.

fig's specs (MIT) are typescript modules. kuterm reads their json form, the subset in part 1;
`generators`, `script` and other code are ignored.

about 85 commands: core tools (ls, cp, grep, find, tar, curl, ssh, rsync, ps, systemctl,
journalctl, ...), package managers (apt, apt-get, apt-cache, dpkg, dnf, yum, rpm, pacman,
zypper, apk, snap, flatpak, pip), git, cargo, docker, npm, and aws with all its services and
operations. each spec is embedded in the binary but only parsed when its command is
completed. aws (5.8 MB of json) ships as a 0.7 MB bundle, see part 1. a user's
`<command>.json` in `~/.config/kuterm/completions/` replaces the bundled one.

the popup colors each kind with the theme's terminal palette, like ls: subcommands magenta,
suggested values cyan, options yellow, programs green, folders blue, files in the text color.

### safety

nothing is run to find completions: commands come from listing PATH, files from listing
folders, the rest from declarative specs. dynamic generators are not supported at all, so no
sandbox or trust prompt is needed yet.

the socket of each tab sits in `$XDG_RUNTIME_DIR/kuterm-complete-<pid>/` (0700, socket 0600).
lines over 64 KiB, bad json, unknown versions, cursors outside the line and relative folders
are dropped without closing the connection. kuterm only sends `accept` lines without control
characters; zsh applies one only when its `seq` is the line's current one and the range fits
the buffer.

before any dynamic provider is added, it needs:

- no implicit execution: completing a command never runs it or its subcommands.
- a dedicated provider runner that executes in a sandbox: no host files beyond allowed paths,
  no network by default, restricted process creation, time, memory, process and output
  limits, cancellation and cleanup. containers alone don't count unless their filesystem,
  process, network, privilege and resource boundaries are set up for it.
- explicit trust: the user approves a provider, a spec referencing code is not enough.
- no side effects: providers return structured suggestions only, they can't touch the ui, the
  shell's line or the session; kuterm validates them and applies one only when accepted.
- fail closed: without isolation, static suggestions or none.
- nothing is evaluated in the user's interactive shell.

### decisions

1. reused from inshellisense: the engine design, not code. fig: the spec format.
2. both MIT; nothing is vendored. the only new dependency is flate2 for the aws bundle,
   already built into kuterm through gpui.
3. names, descriptions, subcommands, options with arguments, persistent and repeatable
   options, suggestions, file and folder templates, priority, hidden, `loadSpec`.
4. the cursor cell comes from `last_content.cursor`, cell size from `terminal_bounds`; the
   popup starts under the first char of the word being completed.
5. with `autocomplete.enable` and `shell_integration.inject` on, new zsh tabs get
   `KUTERM_COMPLETION_SOCKET` and kuterm's `.zshenv` connects. other shells get neither the
   socket nor the variable, so nothing there tries to complete.
6. `autocomplete.trigger`: `"auto"` (default) or `"key"`.
7. up/down move, tab accepts, escape hides until the next word, enter is left to the shell;
   all rebindable. they reach the popup while zsh edits its line and pass to the shell
   while it doesn't show; one typed while candidates are still being
   found, or right after an accept, waits for them (at most 400 ms) and goes to the shell
   when nothing shows. `"when": "completion"` is while the popup shows.
8. dynamic providers are not supported, see safety.
9. zsh 5.3 or newer (`add-zle-hook-widget`, `line-pre-redraw`) with `zsh/net/socket`.
10. zsh only, see status.

### known limits

- zsh only. other shells, and shells over ssh, keep working without completions.
- the socket fd is inherited by programs started from the shell; a background program that
  outlives the shell delays the "closed" event until it exits.
- the popup shows the candidates of the previous keystroke for the few milliseconds the engine
  takes; popup keys in that window wait for the fresh ones.
