"""e2e session: xvfb display, openbox for focus and size hints, a private session bus, the report"""

import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

import numpy as np
import pytest
from PIL import Image

sys.path.insert(0, str(Path(__file__).parent))

from harness import (ARTIFACTS, BACKEND, BIN, WORKER, WORKER_NUMBER, App, Input, Session, session_env, sh,
                     swaymsg, wait_until)
from report import Report

DISPLAY = os.environ.get("E2E_DISPLAY", f":{99 + WORKER_NUMBER}")
# workers of a parallel run keep their screenshots apart, the controller merges the reports
SHOTS = ARTIFACTS / f"shots-{BACKEND}" / WORKER

REPORT = Report()


def pytest_addoption(parser):
    # read by harness.py at import, so run.sh passes it on as E2E_BACKEND too
    parser.addoption("--backend", choices=["x11", "wayland"], default=None,
                     help="display server to run the apps on, x11 by default or $E2E_BACKEND")


def pytest_configure(config):
    config.addinivalue_line("markers", "x11_only: needs x11 tools like xprop or xkb layouts")
    config.addinivalue_line("markers", "wayland_only: tests something only wayland has")
    backend = config.getoption("--backend")
    if backend is not None and backend != BACKEND:
        raise pytest.UsageError(f"--backend {backend} needs E2E_BACKEND={backend} set before pytest starts")


def pytest_sessionstart(session):
    # each backend keeps its own report, so a run on one leaves the other's. runs on the
    # controller before any worker starts
    if not WORKER:
        shutil.rmtree(ARTIFACTS / f"shots-{BACKEND}", ignore_errors=True)


def pytest_collection_modifyitems(config, items):
    REPORT.order = {item.nodeid: i for i, item in enumerate(items)}
    other = "wayland_only" if BACKEND == "x11" else "x11_only"
    for item in items:
        if other in item.keywords:
            item.add_marker(pytest.mark.skip(reason=f"{other.replace('_', ' ')}, running on {BACKEND}"))


def start_x11() -> list:
    """xvfb and openbox"""
    # xvfbs of parallel workers race to create the socket folder, the loser listens only on
    # the abstract socket, which some clients never try
    Path("/tmp/.X11-unix").mkdir(mode=0o1777, exist_ok=True)
    procs = [subprocess.Popen(
        ["Xvfb", DISPLAY, "-screen", "0", "3840x2160x24", "-nolisten", "tcp", "-dpi", "96"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )]
    wait_until(lambda: subprocess.run(["xdpyinfo", "-display", DISPLAY], capture_output=True).returncode == 0,
               timeout=20, msg=f"xvfb on {DISPLAY}")
    log = Path(Session.runtime_dir) / "openbox.log"
    # sessions starting together on a busy machine can leave openbox without the wm selection,
    # then no window ever gets focus, so it gets another try
    for _ in range(3):
        wm = subprocess.Popen(["openbox"], env=session_env(), stdout=subprocess.DEVNULL,
                              stderr=open(log, "a"))
        # openbox is ready once it owns the wm selection
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline and wm.poll() is None:
            out = sh("xprop", "-root", "_NET_SUPPORTING_WM_CHECK", check=False, env=session_env())
            if "window id" in out:
                return procs + [wm]
            time.sleep(0.1)
        wm.kill()
        wm.wait()
    raise RuntimeError(f"openbox did not start on {DISPLAY}:\n{log.read_text()}")


def start_wayland() -> list:
    """headless sway with a virtual keyboard and pointer, it has no input devices of its own"""
    runtime = Path(Session.runtime_dir)
    env = {**session_env(), "WLR_BACKENDS": "headless", "WLR_RENDERER": "pixman", "WLR_LIBINPUT_NO_DEVICES": "1"}
    env.pop("WAYLAND_DISPLAY")
    env.pop("SWAYSOCK")
    log = open(runtime / "sway.log", "w")
    procs = [subprocess.Popen(["sway", "-c", str(Path(__file__).with_name("sway.conf"))], env=env,
                              stdout=log, stderr=subprocess.STDOUT)]
    wait_until(lambda: list(runtime.glob("sway-ipc.*.sock")) and (runtime / Session.wayland_display).exists(),
               timeout=20, msg="sway sockets")
    Session.sway_socket = str(next(runtime.glob("sway-ipc.*.sock")))
    wait_until(lambda: swaymsg("-t", "get_version", check=False), timeout=10, msg="sway ipc")
    Input.start()
    procs.append(Input.proc)
    wait_until(lambda: any(i["type"] == "keyboard" for i in swaymsg("-t", "get_inputs")), msg="virtual keyboard")
    return procs


@pytest.fixture(scope="session", autouse=True)
def display_session():
    assert BIN.exists(), f"binary {BIN} missing, run cargo build first"
    Session.display = DISPLAY
    runtime = Path(Session.runtime_dir)
    shutil.rmtree(runtime, ignore_errors=True)
    runtime.mkdir(mode=0o700)
    bus = subprocess.run(
        ["dbus-daemon", "--session", "--fork", "--print-address=1", "--print-pid=1"],
        capture_output=True, text=True, check=True,
    ).stdout.split()
    Session.bus_address, bus_pid = bus[0], int(bus[1])
    procs = start_wayland() if BACKEND == "wayland" else start_x11()
    yield
    for proc in procs:
        proc.terminate()
    for proc in procs:
        try:
            proc.wait(5)
        except subprocess.TimeoutExpired:
            proc.kill()
    subprocess.run(["kill", str(bus_pid)], capture_output=True)


@pytest.fixture
def app_factory(request):
    """start apps with App(...) arguments, all closed after the test, screenshots go to the report"""
    entry = REPORT.entry(request.node)
    apps = []
    # last screenshot per app, so an unchanged final state is not added twice
    last = {}

    def on_shot(app: App, caption: str) -> None:
        img = app.shot()
        if id(app) in last and np.array_equal(last[id(app)], img):
            return
        last[id(app)] = img
        # the window title is not part of the screenshot, titles are tested too
        caption = f"{caption} (window title: {app.title()!r})"
        path = SHOTS / f"{len(REPORT.entries):04d}-{len(entry.shots):02d}.png"
        path.parent.mkdir(parents=True, exist_ok=True)
        Image.fromarray(img).save(path, optimize=True)
        entry.shots.append((f"app {apps.index(app) + 1}: {caption}" if len(apps) > 1 else caption, path))

    def make(*args, start: bool = True, **kwargs) -> App:
        kwargs.setdefault("name", request.node.name[:40].replace("/", "_").replace("[", "_").replace("]", ""))
        app = App(*args, start=False, on_shot=on_shot, **kwargs)
        apps.append(app)
        entry.states.append(app)
        if start:
            app.start()
        return app

    yield make
    failed = getattr(request.node, "rep_call", None) is not None and request.node.rep_call.failed
    for app in apps:
        if app.alive():
            try:
                app.snap("at failure" if failed else "end of test")
            except Exception:
                pass
        if failed:
            entry.logs.append(app.output()[-3000:])
        app.close()
        entry.freeze(app)
        app.cleanup()


@pytest.fixture
def report_note(request):
    """add text to the report of this test, like output of a run without a window"""
    entry = REPORT.entry(request.node)
    return lambda title, text: entry.notes.append((title, text))


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_makereport(item, call):
    # lets fixtures know whether the test body failed, and the report what happened
    outcome = yield
    rep = outcome.get_result()
    setattr(item, "rep_" + rep.when, rep)
    REPORT.record(item, rep)


def pytest_sessionfinish(session, exitstatus):
    if WORKER:
        REPORT.dump(SHOTS / "entries.json")
        return
    for path in sorted((ARTIFACTS / f"shots-{BACKEND}").glob("gw*/entries.json")):
        REPORT.load(path)
    REPORT.write(ARTIFACTS / f"report-{BACKEND}.html")
