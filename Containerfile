FROM docker.io/library/rust:1.98

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        clang cmake pkg-config \
        libfontconfig-dev libfreetype-dev libxkbcommon-dev libxkbcommon-x11-dev \
        libwayland-dev libx11-xcb-dev libxcb1-dev libvulkan-dev \
        libasound2-dev libssl-dev libzstd-dev \
        zsh \
    && rm -rf /var/lib/apt/lists/*

RUN rustup component add clippy

ENV SHELL=/bin/bash
ENV CARGO_TARGET_DIR=/src/target/podman
WORKDIR /src
