#!/bin/sh
# e2e tests: check the real binary by pixels on xvfb or headless sway, all inside podman.
# usage: e2e/run.sh [--backend x11|wayland|all] [pytest args], e.g. -k cursor or -x
# KUTERM_BIN=path/to/kuterm tests that binary, otherwise a debug build is made first.
# E2E_WORKERS=n runs the tests in n parallel sessions (4 by default), 0 runs them one at a time.
# the report with screenshots of every tested feature is written to e2e/artifacts/report-<backend>.html.
# --backend all runs x11 and wayland at the same time, their lines prefixed with the backend.
# exits non-zero when any test fails
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
set -e
backend="${E2E_BACKEND:-x11}"
if [ "$1" = "--backend" ]; then
	backend="$2"
	shift 2
fi
case "$backend" in
x11 | wayland | all) ;;
*)
	echo "unknown backend $backend, use x11, wayland or all" >&2
	exit 2
	;;
esac
workers="${E2E_WORKERS:-4}"
podman build -q -t kuterm-e2e -f "$ROOT/e2e/Containerfile" "$ROOT/e2e" >/dev/null
if [ -n "$KUTERM_BIN" ]; then
	bin="$KUTERM_BIN"
else
	podman build -q -t kuterm-dev - <"$ROOT/Containerfile" >/dev/null
	podman run --rm -v "$ROOT":/src:Z \
		-v kuterm-cargo-registry:/usr/local/cargo/registry \
		-v kuterm-cargo-git:/usr/local/cargo/git \
		kuterm-dev cargo build --quiet
	bin="$ROOT/target/podman/debug/kuterm"
fi

# run_backend x11|wayland [pytest args]
# the container gets copies of the files it needs instead of the checkout mounted, so tests
# cannot change it, and only the report comes back out
run_backend() (
	b="$1"
	shift
	# the other backend's report stays
	rm -rf "$ROOT/e2e/artifacts/report-$b.html" "$ROOT/e2e/artifacts/shots-$b" "$ROOT/e2e/artifacts/presets-$b"
	if [ "$workers" -gt 0 ]; then
		# loadgroup keeps tests marked with the same xdist_group on one worker
		set -- -n "$workers" --dist loadgroup "$@"
	fi
	name="kuterm-e2e-$b-$$"
	trap 'podman rm -f "$name" >/dev/null 2>&1' EXIT
	trap 'exit 130' INT TERM
	# every session runs xvfb or sway and kuterm with 4k frame buffers shared through /dev/shm,
	# the 64 MB default is too small
	n=$((workers > 1 ? workers : 1))
	podman create --name "$name" --init --memory "$((2 + 2 * n))g" --shm-size "${n}g" \
		-e KUTERM_BIN=/src/kuterm -e E2E_BACKEND="$b" -e PYTHONDONTWRITEBYTECODE=1 \
		kuterm-e2e python3 -m pytest -p no:cacheprovider -v -rfEs --tb=short e2e "$@" >/dev/null || exit
	tar -C "$ROOT" --exclude=e2e/artifacts --exclude=__pycache__ -cf - e2e assets examples Cargo.toml |
		podman cp - "$name:/src" || exit
	podman cp "$bin" "$name:/src/kuterm" || exit
	podman start -a "$name"
	status=$?
	mkdir -p "$ROOT/e2e/artifacts"
	podman cp "$name:/src/e2e/artifacts/report-$b.html" "$ROOT/e2e/artifacts/" 2>/dev/null
	# screenshots of examples/presets, see e2e/test_presets.py
	podman cp "$name:/src/e2e/artifacts/presets-$b" "$ROOT/e2e/artifacts/" 2>/dev/null
	exit $status
)

set +e
if [ "$backend" = all ]; then
	logs="$(mktemp -d)"
	for b in x11 wayland; do
		{
			run_backend "$b" "$@" 2>&1
			echo $? >"$logs/$b.status"
		} | sed -u "s/^/[$b] /" &
	done
	wait
	status=0
	for b in x11 wayland; do
		s="$(cat "$logs/$b.status")"
		echo "[$b] report: e2e/artifacts/report-$b.html"
		if [ "$s" -ne 0 ]; then
			echo "=== E2E $b FAILED (exit $s) ==="
			status=1
		fi
	done
	rm -rf "$logs"
	exit $status
fi
run_backend "$backend" "$@"
status=$?
echo "report: e2e/artifacts/report-$backend.html"
if [ $status -ne 0 ]; then
	echo "=== E2E FAILED (exit $status) ==="
fi
exit $status
