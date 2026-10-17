#!/bin/sh
podman build -q -t kuterm-dev - < Containerfile && \
podman run --rm -v "${1}":/src:Z \
  -v kuterm-cargo-registry:/usr/local/cargo/registry \
  -v kuterm-cargo-git:/usr/local/cargo/git \
  kuterm-dev sh -c 'cargo fmt --check && cargo build && cargo clippy --all-targets -- -D warnings && cargo test'
