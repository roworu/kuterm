"""performance and resources: startup time, idle cpu, output speed, memory, file descriptors, threads, children

limits are loose on purpose, runs use a software renderer and may use a debug build.
the measured numbers are written to the report.
"""

import time

import pytest

from harness import BACKEND, App, bundled_theme, close_to, dominant

FEATURE = "performance and resources"

DARK = bundled_theme(True)

# timed tests run on one worker of a parallel run, one after another
timed = pytest.mark.xdist_group("timed")


def plain(app_factory, **settings) -> App:
    # a blinking cursor repaints on purpose, the idle numbers are for a still one
    terminal = {"cursor_blink": "off", **settings.pop("terminal", {})}
    return app_factory({"theme": {"mode": "dark"}, "window_title": ["number"], "terminal": terminal, **settings})


def cycle_tabs(app: App, count: int) -> None:
    """open count tabs and close them again, back to one tab"""
    for n in range(2, count + 2):
        app.key("ctrl+shift+t")
        app.wait_title(str(n))
    for n in range(count, 0, -1):
        app.key("ctrl+shift+w")
        app.wait_title(str(n))
    # closed shells and pty threads are reaped in the background
    time.sleep(2)


@timed
def test_startup_time(app_factory, report_note):
    """the first frame is painted within a few seconds of launch"""
    app = app_factory({"theme": {"mode": "dark"}}, start=False)
    start = time.monotonic()
    app.start()
    took = time.monotonic() - start
    report_note("startup to first frame", f"{took:.2f} s")
    assert took < 15


@timed
def test_idle_cpu(app_factory, report_note):
    """an idle window with a few tabs uses almost no cpu, nothing repaints without a reason"""
    app = plain(app_factory)
    for n in (2, 3):
        app.key("ctrl+shift+t")
        app.wait_title(str(n))
    time.sleep(3)
    usage = app.cpu_usage(5)
    report_note("idle cpu, 3 tabs", f"{usage * 100:.1f} % of one core")
    assert usage < 0.05


@timed
def test_idle_cpu_after_cursor_stops_blinking(app_factory, report_note):
    """a blinking cursor stops after cursor_blink_timeout, then the window goes idle"""
    app = plain(app_factory, terminal={"cursor_blink": "on", "cursor_blink_interval": 500,
                                       "cursor_blink_timeout": 1})
    app.key("ctrl+shift+t")
    app.wait_title("2")
    time.sleep(3)
    usage = app.cpu_usage(5)
    report_note("idle cpu after the cursor stopped blinking", f"{usage * 100:.1f} % of one core")
    assert usage < 0.05


@timed
def test_idle_cpu_while_unfocused_with_notification(app_factory, report_note):
    """a notification and a hidden window do not keep the cpu busy"""
    app = app_factory({"theme": {"mode": "dark"}, "notifications": {"timeout": 0},
                       "terminal": {"cursor_blink": "off"}},
                      commands={"commands": [{"name": "note", "actions": [{"notify": "stays"}]}]})
    app.palette("note")
    time.sleep(3)
    usage = app.cpu_usage(5)
    report_note("idle cpu with a notification", f"{usage * 100:.1f} % of one core")
    assert usage < 0.05


@timed
def test_output_speed(app_factory, report_note):
    """a burst of output is drawn quickly and the cpu goes back to idle after it"""
    app = plain(app_factory)
    time.sleep(1)
    start = time.monotonic()
    app.run(f"seq 1 500000; touch {app.file('done')}")
    app.wait_file("done", timeout=120)
    took = time.monotonic() - start
    report_note("seq 1 500000 (3.4 MB)", f"{took:.2f} s, {3.4 / took:.1f} MB/s")
    assert took < 60
    time.sleep(2)
    usage = app.cpu_usage(4)
    report_note("cpu after the burst", f"{usage * 100:.1f} % of one core")
    assert usage < 0.05


def test_memory_does_not_grow_with_tabs(app_factory, report_note):
    """opening and closing tabs over and over does not leak memory"""
    app = plain(app_factory)
    cycle_tabs(app, 5)
    base = app.rss_mb()
    for _ in range(4):
        cycle_tabs(app, 5)
    after = app.rss_mb()
    report_note("memory, 4 cycles of 5 tabs", f"{base:.0f} MB -> {after:.0f} MB")
    assert after - base < 40


def test_no_fd_or_thread_leak(app_factory, report_note):
    """closed tabs give back their file descriptors and threads"""
    app = plain(app_factory)
    cycle_tabs(app, 3)
    fds, threads = app.fd_count(), app.thread_count()
    for _ in range(3):
        cycle_tabs(app, 3)
    report_note("fds and threads, 3 cycles of 3 tabs",
                f"fds {fds} -> {app.fd_count()}, threads {threads} -> {app.thread_count()}")
    assert app.fd_count() <= fds + 2
    assert app.thread_count() <= threads + 2


def test_closed_tabs_leave_no_zombies(app_factory, report_note):
    """shells of closed tabs are reaped, only the open tab's shell is a child"""
    app = plain(app_factory)
    cycle_tabs(app, 4)
    app.wait(lambda: len(app.children()) == 1, msg=f"one child, got {app.children()}")
    children = app.children()
    report_note("children after closing 4 tabs", repr(children))
    assert all(state != "Z" for _, state in children)


def test_scrollback_memory_is_bounded(app_factory, report_note):
    """max_history_length caps the memory long output can take"""
    app = plain(app_factory, terminal={"max_history_length": 1000})
    time.sleep(1)
    base = app.rss_mb()
    line = "x" * 200
    for n in range(3):
        app.run(f"for i in $(seq 1 20000); do echo {line}; done; touch {app.file(f'done{n}')}")
        app.wait_file(f"done{n}", timeout=180)
    after = app.rss_mb()
    report_note("memory after 60000 lines of 200 chars, history 1000", f"{base:.0f} MB -> {after:.0f} MB")
    assert after - base < 60


def test_resize_storm(app_factory, report_note):
    """many resizes in a row leave the app alive with the right grid"""
    app = plain(app_factory)
    start = time.monotonic()
    for i in range(40):
        app.resize(500 + (i * 37) % 800, 300 + (i * 53) % 500)
    app.resize(900, 600)
    if BACKEND == "wayland":
        # sway adopts a late commit for an older size as the floating window's own resize and
        # drops the queued ones, the app acks every configure right. ask for the size once more
        time.sleep(1)
        app.resize(900, 600)
    app.wait(lambda: app.size() == (900, 600), msg="final size")
    report_note("40 resizes", f"{time.monotonic() - start:.2f} s")
    app.wait(lambda: app.pty_size() == app.expected_pty(900, 600), msg="pty size after the storm")
    assert close_to(dominant(app.shot()), DARK["terminal_background"])


def test_rapid_tab_keys(app_factory):
    """pressing new tab and close tab as fast as possible does not crash"""
    app = plain(app_factory)
    for _ in range(3):
        app.key(*["ctrl+shift+t"] * 10)
        app.key(*["ctrl+shift+w"] * 10)
    app.wait_title("1", timeout=20)
    assert app.alive()
