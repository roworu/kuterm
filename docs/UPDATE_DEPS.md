# Update dependencies

run cargo commands inside `kuterm-dev` container (see AGENTS.md).

## crates.io deps

- `cargo update`: bump within semver ranges (Cargo.lock only).
- `cargo upgrade --incompatible`: bump past ranges, edits Cargo.toml (needs `cargo-edit`).
- `cargo outdated`: list outdated deps (needs `cargo-outdated`, ignores git deps).

## git deps (gpui, gpui_platform, alacritty_terminal)

pinned by `rev`, so `cargo update` won't move them.

gpui comes from the fork https://github.com/roworu/zed, branch `kuterm-vulkan-first`: upstream zed
plus kuterm's own commits (vulkan without the gl probe) until they land upstream.

1. `git ls-remote https://github.com/zed-industries/zed HEAD` -> new zed rev.
2. rebase the fork branch on it and push. tag the commit too, so revs used by older kuterm
   releases stay fetchable after the next force push:
   ```bash
   git clone --filter=blob:none --branch kuterm-vulkan-first git@github.com:roworu/zed.git && cd zed
   git fetch https://github.com/zed-industries/zed <REV>
   git rebase FETCH_HEAD
   git tag "kuterm-$(git rev-parse --short FETCH_HEAD)"
   git push --force-with-lease origin kuterm-vulkan-first --tags
   ```
   once a commit is merged upstream it drops out in the rebase; with none left, point
   `Cargo.toml` back at `zed-industries/zed` and drop the fork from `allow-git` in `deny.toml`.
3. set the new fork commit (`git rev-parse HEAD`) on all 3 gpui entries (`gpui`,
   `gpui_platform`, dev-dep `gpui`).
4. `curl -s https://raw.githubusercontent.com/zed-industries/zed/<REV>/Cargo.toml | grep alacritty` -> set that rev on `alacritty_terminal` (must match, shared types).

## verify

```bash
sh .zed/podman-build-test.sh "$PWD"
cargo fmt --check && cargo clippy --all-targets -- -D warnings
```
