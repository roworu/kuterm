# AGENTS.md

that file provides guidance to AI coding agents on how to work with code in this repo.

kuterm is gpu accelerated desktop terminal emulator written in rust: gpui for ui, alacritty for terminal emulation.

## Build and test

host usually lacks native deps (fontconfig, some wayland libs, xkbcommon... etc.), so build inside podman/docker using `Containerfile`:

```bash
# cargo build, clippy, test in container
sh .zed/podman-build-test.sh "$PWD"
# run container-built binary on host
./target/podman/debug/kuterm
```

to run single test:

```bash
podman run --rm -v "$PWD":/src:Z \
  -v kuterm-cargo-registry:/usr/local/cargo/registry \
  -v kuterm-cargo-git:/usr/local/cargo/git \
  kuterm-dev cargo test <test_name>
```

e2e tests drive the real binary and check pixels (`e2e/`, pytest), on xvfb with openbox (x11, default) or on headless sway (wayland). they run for both backends on pull requests in the test workflow and write `e2e/artifacts/report-<backend>.html` with screenshots of every tested feature:

```bash
# all of them, or pass pytest args like -k palette
sh e2e/run.sh
# the same tests on wayland, --backend goes first
sh e2e/run.sh --backend wayland -k palette
# both backends at once, lines prefixed with the backend, fails if either fails
sh e2e/run.sh --backend all
# 4 parallel sessions by default, E2E_WORKERS=0 runs one test at a time
E2E_WORKERS=0 sh e2e/run.sh -k palette
```

- tests run in parallel with pytest-xdist, every worker gets its own display, session bus and runtime dir, and the reports of all workers are merged into one. a test must not look at processes, files or windows outside its own app. tests that measure cpu or speed share one `xdist_group`, so they run one after another.
- wait for what shows on screen (`app.wait`, `wait_title`, `wait_line`, `wait_still`) instead of fixed sleeps; a sleep is only for checking that something does *not* happen, or for pacing input.
- tests talk to the display only through `App` in `e2e/harness.py` (keys, mouse, screenshots, titles, clipboard), which picks xdotool/xwininfo or swaymsg/grim/wl-clipboard by backend.
- wayland input comes from `e2e/fake_input.py`, one virtual keyboard and pointer for the whole session; headless sway has no devices of its own.
- `@pytest.mark.x11_only` / `@pytest.mark.wayland_only` skip tests on the other backend.
- the e2e container (`e2e/Containerfile`) also has cjk, emoji and ligature (fira code) fonts, xkb tools for layout tests, wmctrl, sway, grim, wl-clipboard and less.

CI also runs `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings`. container output goes to `target/podman/`. gpui and alacritty are pinned to same git revision in `Cargo.toml`; bump them together. gpui comes from a fork of zed with kuterm commits on top, bump it as in `docs/UPDATE_DEPS.md`.

## Architecture

- `main.rs` `init(cx)` holds all startup setup (fonts, settings, theme, keybindings); keep it separate from `main`.

- `src/terminal/`: terminal model, no ui.
  - `TerminalBuilder::new` spawns shell in a pty.
  - `subscribe` moves it into a gpui `Entity<Terminal>` and pumps alacritty events (`events.rs`).
  - `Terminal::sync` snapshots grid into `last_content` (`content.rs`), which is all renderer reads. it only copies when alacritty reported new output (a `Wakeup` sets a shared dirty flag) or a local change was queued, and reuses the cell buffer.
  - `selection.rs` maps mouse positions to grid points; the selection lives in alacritty's `Term`, so it follows scrollback.
  - `keys.rs` maps keystrokes to escape sequences.

- `src/settings/` and `src/theme.rs`: gpui globals read with `Settings::get(cx)` / `Theme::get(cx)`.
  - defaults live only in `assets/*.jsonc` (embedded as bin with `include_str!`); `Default` impls parse them, so NEVER hardcode defaults in rust as a code.
  - user files in `$XDG_CONFIG_HOME/kuterm/` are parsed and deep merged over the bundled json (`settings::merge`); enums and arrays are replaced whole. missing files are created from defaults, broken ones fall back to defaults with an error on stderr.
  - new setting: add the struct field, add it with a comment to `assets/default_settings.jsonc`. numeric limits are enforced in `Settings::parse`.
  - new action: add it to `binding()` in `keybindings.rs` and to `assets/default_keybindings.jsonc` and `assets/default_keybindings_macos.jsonc` with a comment.

- `src/completion/`: autocomplete engine, no ui. `complete` turns a command line and cursor into candidates from fig-style json specs (`assets/completions/`), PATH and the filesystem, it never runs anything. `Bridge` is the private unix socket of one tab that kuterm's zsh script (`assets/shell/zsh/.zshenv`) reports its line on and gets picked completions back from (`protocol.rs`). the popup lives in `src/ui/terminal_view/completions.rs`. bundled specs are generated from the real programs inside podman, never on the host: follow `docs/COMPLETIONS.md` to add or refresh one.

- `src/ui/`: `Workspace` (root view, tabs) -> `TerminalView` (focus, keys, scroll, paste) -> `TerminalElement`, a custom gpui `Element` that sizes terminal in `prepaint`, builds background rects and batched text runs (`grid.rs`), and paints cursor (`cursor.rs`).

## Conventions

comments explain `why`, not `what` they do. tests go in a `mod tests` at the bottom of the file they test; terminal tests spawn a real shell.

every change must pass the full test suite (`sh .zed/podman-build-test.sh "$PWD"`), and new functionality must come with new tests.

always ask the user to review changes before committing or opening a PR. when acting autonomously (no user in the loop), end the PR description with a line like `Changes prepared by <model>, running <harness>.`
