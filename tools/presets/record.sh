#!/bin/sh
# record examples/presets/<preset>/demo.mp4 (60 fps, native resolution) for every preset and the
# readme tour assets/tour/tour.mp4 with its poster, or the ones named:
#   sh tools/presets/record.sh [--fps 60] [--crf 16] [--stills] [preset or tour ...]
# kuterm runs on a virtual x display with a window manager inside podman, driven by xdotool and
# filmed with ffmpeg, so nothing touches your desktop. KUTERM_BIN=path picks the binary, otherwise
# a release build is made first, a debug one draws too slowly for 60 fps. --stills also saves a
# png per step into target/podman/stills
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
set -e
podman build -q -t kuterm-e2e -f "$ROOT/e2e/Containerfile" "$ROOT/e2e" >/dev/null
podman build -q -t kuterm-record -f "$ROOT/tools/presets/Containerfile" "$ROOT/tools/presets" >/dev/null
if [ -z "$KUTERM_BIN" ]; then
	podman build -q -t kuterm-dev - <"$ROOT/Containerfile" >/dev/null
	podman run --rm -v "$ROOT":/src:Z \
		-v kuterm-cargo-registry:/usr/local/cargo/registry \
		-v kuterm-cargo-git:/usr/local/cargo/git \
		kuterm-dev cargo build --release --quiet
	KUTERM_BIN="$ROOT/target/podman/release/kuterm"
fi
mkdir -p "$ROOT/target/podman/stills"
podman run --rm --init --shm-size 1g -v "$ROOT":/src:Z -v "$(realpath "$KUTERM_BIN")":/kuterm:ro,Z \
	-v "$ROOT/target/podman/stills":/tmp/stills:Z -e KUTERM_BIN=/kuterm \
	kuterm-record python3 /src/tools/presets/record.py "$@"
