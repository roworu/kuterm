"""command queue: commands queued while something runs are typed one by one when the prompt
comes back, from the palette, kuterm msg queue or at a time of day. a failure pauses the rest
when "terminal.queue_stop_on_failure" says so, and the palette edits, reorders, drops, resumes
and clears entries"""

import subprocess
import time

import pytest

from harness import App, as_rgb, mask_bbox, near, unique_theme
from kit import wait_block, integrated, msg, nothing_for, notification_box, ready, send, text

FEATURE = "command queue"

THEME = unique_theme(0x91)
C = as_rgb(THEME)


def queue_app(app_factory, stop_on_failure: bool = True, **extra) -> App:
    settings = integrated(terminal={"queue_stop_on_failure": stop_on_failure}, window_title=["queue"], **extra)
    app = app_factory(settings, themes={"dark": THEME})
    ready(app)
    return app


def order(app: App) -> list:
    path = app.file("order")
    return path.read_text().split() if path.exists() else []


def start(app: App, line: str, pane: str | None = None) -> None:
    """run a command and wait until it runs, so entries queued next wait for it"""
    send(app, line, pane)
    wait_block(app, line.split()[0], pane, finished=False)


def queue(app: App, command: str, *args: str) -> None:
    done = msg(app, "queue", *args, command)
    assert done.returncode == 0, done.stderr


def test_queued_commands_run_in_order_after_the_running_one(app_factory):
    app = queue_app(app_factory)
    start(app, "sleep 3; echo first >> $T2_TMP/order")
    for word in ("second", "third"):
        queue(app, f"echo {word} >> $T2_TMP/order")
    app.wait_title("⧗ 2")
    app.snap("two commands wait in the queue while sleep runs")
    app.wait(lambda: order(app) == ["first", "second", "third"], msg="all three in order", timeout=20)
    # the title block is empty once the queue is
    app.wait_title("kuterm")


def test_queue_at_an_idle_prompt_runs_right_away(app_factory):
    app = queue_app(app_factory)
    queue(app, "touch $T2_TMP/now")
    app.wait(app.file("now").exists, msg="ran at once")


def test_a_failure_pauses_the_queue_until_resumed(app_factory):
    app = queue_app(app_factory)
    start(app, "sleep 2; false")
    queue(app, "touch $T2_TMP/after")
    app.wait_title("⧗ 1")
    # the failure pauses it, the entry stays and the hint under the prompt says so
    nothing_for(5, app.file("after").exists)
    assert app.title() == "⧗ 1"
    app.snap("paused after a failure, the next entry shown under the prompt")
    app.palette("resume queue")
    app.wait(app.file("after").exists, msg="resumed entry ran")


def test_a_failing_queued_command_pauses_the_ones_after_it(app_factory):
    app = queue_app(app_factory)
    start(app, "sleep 2")
    queue(app, "false")
    queue(app, "touch $T2_TMP/later")
    app.wait_title("⧗ 2")
    app.wait_title("⧗ 1", timeout=15)
    nothing_for(3, app.file("later").exists)


def test_without_stop_on_failure_the_queue_goes_on(app_factory):
    app = queue_app(app_factory, stop_on_failure=False)
    start(app, "sleep 2; false")
    queue(app, "false")
    queue(app, "touch $T2_TMP/went_on")
    app.wait(app.file("went_on").exists, msg="queue went on after failures", timeout=20)


def test_queue_command_from_the_palette(app_factory):
    app = queue_app(app_factory)
    start(app, "sleep 3")
    app.palette("queue a command")
    time.sleep(0.5)
    app.type("touch $T2_TMP/from_palette")
    app.key("Return")
    app.wait_title("⧗ 1")
    app.wait(app.file("from_palette").exists, msg="palette entry ran", timeout=20)


def test_edit_an_entry_from_the_palette(app_factory):
    app = queue_app(app_factory)
    start(app, "sleep 6")
    queue(app, "touch $T2_TMP/wrong")
    app.wait_title("⧗ 1")
    app.palette("edit queued commands")
    time.sleep(0.5)
    # the first row edits the first entry, its text is in the field
    app.key("Return")
    time.sleep(0.5)
    app.key("ctrl+a")
    app.type("touch $T2_TMP/right")
    app.key("Return")
    app.wait(app.file("right").exists, msg="edited entry ran", timeout=20)
    assert not app.file("wrong").exists()


def test_an_emptied_entry_is_dropped(app_factory):
    app = queue_app(app_factory)
    start(app, "sleep 5")
    queue(app, "touch $T2_TMP/dropped")
    queue(app, "touch $T2_TMP/kept")
    app.wait_title("⧗ 2")
    app.palette("edit queued commands")
    time.sleep(0.5)
    app.key("Return")
    time.sleep(0.5)
    app.key("ctrl+a", "BackSpace", "Return")
    app.wait_title("⧗ 1")
    app.wait(app.file("kept").exists, msg="the other entry ran", timeout=20)
    assert not app.file("dropped").exists()


def test_run_first_moves_an_entry_to_the_front(app_factory):
    app = queue_app(app_factory)
    start(app, "sleep 5")
    queue(app, "echo a >> $T2_TMP/order")
    queue(app, "echo b >> $T2_TMP/order")
    app.palette("edit queued commands")
    time.sleep(0.5)
    app.type("run first")
    app.key("Return")
    app.wait(lambda: order(app) == ["b", "a"], msg="b ran first", timeout=20)


def test_clear_the_queue(app_factory):
    app = queue_app(app_factory)
    start(app, "sleep 4")
    queue(app, "touch $T2_TMP/a")
    queue(app, "touch $T2_TMP/b")
    app.palette("edit queued commands")
    time.sleep(0.5)
    app.type("clear the queue")
    app.key("Return")
    app.wait_title("kuterm")
    nothing_for(6, lambda: app.file("a").exists() or app.file("b").exists())


def test_an_empty_queue_says_so(app_factory):
    """the editor of an empty queue opens nothing, a notification tells why"""
    app = queue_app(app_factory)
    app.mouse(100, 100)
    app.palette("edit queued commands")
    app.wait(lambda: notification_box(app, C), msg="the queue is empty notice")
    app.snap("the queue is empty")


def test_timed_entry_waits_for_its_minute(app_factory):
    """"@HH:MM cmd" waits for that time of day, then for the prompt"""
    app = queue_app(app_factory)
    now = subprocess.run(["date", "+%H:%M:%S"], capture_output=True, text=True).stdout.strip()
    hour, minute, second = map(int, now.split(":"))
    # a minute ahead, far enough that the test can see it wait
    if second > 50:
        time.sleep(61 - second)
        minute += 1
    minute += 1
    hour, minute = (hour + minute // 60) % 24, minute % 60
    queue(app, f"@{hour:02}:{minute:02} touch $T2_TMP/timed")
    app.wait_title("⧗ 1")
    nothing_for(3, app.file("timed").exists)
    app.wait(app.file("timed").exists, msg="timed entry ran", timeout=130)


@pytest.mark.parametrize("bad", ["@25:00 x", "@12:00", "   "])
def test_bad_entries_are_refused(app_factory, bad):
    app = queue_app(app_factory)
    done = msg(app, "queue", bad)
    assert done.returncode != 0


def test_queue_into_a_background_tab_by_name(app_factory):
    app = queue_app(app_factory)
    assert msg(app, "new-tab", "--name", "bg").returncode == 0
    ready(app, "bg")
    start(app, "sleep 3", pane="bg")
    queue(app, "touch $T2_TMP/bg_ran", "--pane", "bg")
    app.wait(app.file("bg_ran").exists, msg="background tab ran its queue", timeout=20)


def test_queue_list_shows_over_the_pane_while_a_command_runs(app_factory):
    app = queue_app(app_factory)
    start(app, "sleep 8")
    queue(app, "true one")
    queue(app, "true two")
    queue(app, "true three")
    queue(app, "true four")

    def listed():
        img = app.shot()
        h, w = img.shape[:2]
        # the list sits in the bottom left corner in the tab bar color with a border
        return mask_bbox(near(img[h // 2 :, : w // 2], C["tab_bar_background"]))

    box = app.wait(listed, msg="queue list in the bottom left corner")
    assert box[2] - box[0] > 40
    app.snap("the waiting commands over the pane, three and a count")


def test_typed_input_at_the_prompt_holds_the_queue(app_factory):
    """an entry is only typed into an empty command line, never into what the user typed"""
    app = queue_app(app_factory)
    app.type("echo typed")
    app.wait(lambda: "echo typed" in text(app), msg="typed text shown")
    queue(app, "touch $T2_TMP/held")
    nothing_for(3, app.file("held").exists)
    assert app.title() == "⧗ 1"
    app.key("ctrl+u")
    app.wait(app.file("held").exists, msg="held entry ran once the line was empty", timeout=20)
