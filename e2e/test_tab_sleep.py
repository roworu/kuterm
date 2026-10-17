"""tab sleeping: a tab hidden longer than "sleep_tabs_after_minutes" has its history packed away
and put back when shown, read or searched; its programs keep running and output while asleep is
kept below the history"""

import time

import pytest

from harness import App
from kit import SOCKET, listening, msg, tabs, text, wait_text

FEATURE = "tab sleeping"


def sleepy_app(app_factory, minutes: float) -> App:
    app = app_factory({**SOCKET, "hide_bar_for_one_tab": False, "window_title": ["number"],
                       "sleep_tabs_after_minutes": minutes, "terminal": {"max_history_length": 0}})
    listening(app)
    return app


def first_tab(app: App) -> str:
    return str(next(t["id"] for t in tabs(app) if t["number"] == 1))


def asleep(app: App, pane: str) -> bool:
    return next(t["asleep"] for t in tabs(app) if str(t["id"]) == pane)


def wait_asleep(app: App, pane: str) -> None:
    app.wait(lambda: asleep(app, pane), msg="the hidden tab asleep", timeout=15)


def fill_and_hide(app: App, lines: int = 3000) -> str:
    app.run(f"seq 1 {lines}; echo printed_all")
    pane = first_tab(app)
    wait_text(app, "printed_all", pane)
    app.key("ctrl+shift+t")
    app.wait_title("2")
    return pane


@pytest.mark.parametrize("minutes", [0.02, 0])
def test_history_survives_a_sleep_with_output_while_asleep(app_factory, minutes):
    """with sleeping on or off, the history and what came while hidden read back in order"""
    app = sleepy_app(app_factory, minutes)
    pane = fill_and_hide(app)
    if minutes:
        wait_asleep(app, pane)
    else:
        # well past where it would sleep
        time.sleep(4)
        assert not asleep(app, pane)
    assert msg(app, "send-text", "--pane", pane, "echo while_asleep\n").returncode == 0
    time.sleep(2)
    history = text(app, pane, scrollback=True).splitlines()
    # reading it woke it, it sleeps again later as it stays hidden
    numbers = [line for line in history if line.isdigit()]
    assert numbers[:3] == ["1", "2", "3"] and numbers[-1] == "3000", numbers[:3] + numbers[-3:]
    assert numbers == [str(n) for n in range(1, 3001)]
    at = {needle: next(i for i, line in enumerate(history) if needle in line)
          for needle in ("printed_all", "while_asleep")}
    assert at["printed_all"] < at["while_asleep"]


def test_showing_a_sleeping_tab_brings_the_top_of_its_history_back(app_factory):
    app = sleepy_app(app_factory, 0.02)
    pane = fill_and_hide(app)
    wait_asleep(app, pane)
    app.key("ctrl+Tab")
    app.wait_title("1")
    app.wait(lambda: not asleep(app, pane), msg="woken by showing it")
    app.palette("scroll to top")
    app.wait(lambda: text(app) != "", msg="shown")
    app.snap("the top of the woken history, 1 2 3 ...")


def test_find_in_all_tabs_reaches_a_sleeping_tab(app_factory):
    app = sleepy_app(app_factory, 0.02)
    app.run("seq 1 3000 | sed 's/^/needle_/'; echo printed_all")
    wait_text(app, "printed_all")
    app.key("ctrl+shift+t")
    app.wait_title("2")
    wait_asleep(app, first_tab(app))
    app.palette("find in all tabs")
    app.type("needle_2999")
    time.sleep(1)
    app.key("Return")
    app.wait_title("1")


def test_programs_keep_running_while_asleep(app_factory):
    app = sleepy_app(app_factory, 0.02)
    app.run("seq 1 2000; while :; do date +%s%N > $T2_TMP/tick; sleep 0.2; done")
    time.sleep(1)
    app.key("ctrl+shift+t")
    app.wait_title("2")
    wait_asleep(app, first_tab(app))
    first = app.file("tick").read_text()
    time.sleep(1)
    assert app.file("tick").read_text() != first


def test_a_sleeping_tab_resized_while_hidden_wakes_whole(app_factory):
    app = sleepy_app(app_factory, 0.02)
    pane = fill_and_hide(app)
    wait_asleep(app, pane)
    app.resize(600, 400)
    time.sleep(1)
    app.key("ctrl+Tab")
    app.wait_title("1")
    numbers = [line for line in text(app, pane, scrollback=True).splitlines() if line.isdigit()]
    assert numbers == [str(n) for n in range(1, 3001)]


def test_the_active_tab_never_sleeps(app_factory):
    app = sleepy_app(app_factory, 0.02)
    app.run("seq 1 500")
    time.sleep(4)
    assert not any(t["asleep"] for t in tabs(app))


def test_a_tab_without_history_stays_awake(app_factory):
    """nothing to pack away, so nothing changes"""
    app = sleepy_app(app_factory, 0.02)
    pane = first_tab(app)
    app.key("ctrl+shift+t")
    app.wait_title("2")
    time.sleep(4)
    assert not asleep(app, pane)


def test_closing_a_sleeping_tab(app_factory):
    app = sleepy_app(app_factory, 0.02)
    pane = fill_and_hide(app)
    wait_asleep(app, pane)
    app.key("ctrl+Tab")
    app.wait_title("1")
    app.key("ctrl+shift+w")
    app.wait_title("1")
    assert len(tabs(app)) == 1
