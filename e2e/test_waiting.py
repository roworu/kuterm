"""programs waiting for the user: questions and password prompts mark their tab and notify once
with answer buttons, password prompts lock out automation and show a lock, ctrl+s pausing output
and ctrl+z stopping programs show at the bottom of the pane"""

import time

import pytest

from harness import App, as_rgb, count, mask_bbox, near, unique_theme
from kit import (integrated, msg, nothing_for, notification_box, ready, send, text, wait_buttons,
                 wait_text)

FEATURE = "programs waiting"

THEME = unique_theme(0x93)
C = as_rgb(THEME)

QUESTION = "python3 -c \"x = input('continue? [y/N] '); open('$T2_TMP/answer', 'w').write(x)\""
SECRET = ("python3 -c \"import getpass; p = getpass.getpass('Password: '); "
          "open('$T2_TMP/secret', 'w').write(p)\"")


def waiting_app(app_factory, **extra) -> App:
    app = app_factory(integrated(window_title=["number", {"text": " "}, "icon"], icons={"default": "D"},
                                 **extra),
                      themes={"dark": THEME})
    ready(app)
    app.mouse(450, 300)
    return app


def dialog(app: App) -> tuple | None:
    """bbox of the "close" button of the close dialog, in the danger color"""
    mask = near(app.shot(), C["danger_button"])
    return mask_bbox(mask) if mask.sum() > 200 else None


def marked(app: App) -> int:
    """attention colored pixels in the tab bar"""
    return count(app.shot()[: app.bar_height()], C["attention"], 3)


def in_background(app: App, command: str) -> None:
    """run command in tab 1 and switch to a new tab 2"""
    send(app, command)
    time.sleep(0.5)
    app.key("ctrl+shift+t")
    app.wait_title(lambda title: title.startswith("2 "))


def test_a_quiet_question_in_a_background_tab_notifies_and_y_answers(app_factory):
    app = waiting_app(app_factory)
    in_background(app, QUESTION)
    buttons = wait_buttons(app, C, 2, timeout=20)
    assert marked(app) > 4
    app.snap("the question of tab 1 with y and n buttons, its tab marked")
    app.click(*buttons[0])
    app.wait(lambda: app.file("answer").exists() and app.file("answer").read_text() == "y",
             msg="y typed into the waiting program")
    app.wait(lambda: marked(app) == 0, msg="mark gone once answered")


def test_n_button_answers_no(app_factory):
    app = waiting_app(app_factory)
    in_background(app, QUESTION)
    buttons = wait_buttons(app, C, 2, timeout=20)
    app.click(*buttons[1])
    app.wait(lambda: app.file("answer").exists() and app.file("answer").read_text() == "n", msg="n typed")


def test_a_question_in_the_seen_tab_marks_without_notifying(app_factory):
    app = waiting_app(app_factory)
    send(app, QUESTION)
    app.wait(lambda: marked(app) > 4, msg="tab marked", timeout=20)
    assert notification_box(app, C) is None
    app.type("typed")
    app.key("Return")
    app.wait(lambda: app.file("answer").exists(), msg="answer typed by hand")


def test_a_question_that_is_no_question_is_left_alone(app_factory):
    """a quiet program whose last line asks nothing is just busy"""
    app = waiting_app(app_factory)
    in_background(app, "echo compiling; sleep 30")
    nothing_for(5, lambda: notification_box(app, C) or marked(app) > 4)


def test_program_waiting_off_never_notifies(app_factory):
    app = waiting_app(app_factory, notifications={"program_waiting": False})
    in_background(app, QUESTION)
    nothing_for(5, lambda: notification_box(app, C) or marked(app) > 4)


def test_own_question_patterns_replace_the_bundled_ones(app_factory):
    app = waiting_app(app_factory, notifications={"question_patterns": ["READY>$"]})
    in_background(app, "printf 'READY>'; read x; echo $x > $T2_TMP/answer")
    app.wait(lambda: notification_box(app, C), msg="notified", timeout=20)


def test_own_question_patterns_drop_the_bundled_ones(app_factory):
    app = waiting_app(app_factory, notifications={"question_patterns": ["READY>$"]})
    in_background(app, QUESTION)
    nothing_for(5, lambda: notification_box(app, C))


def test_bundled_patterns_miss_an_own_prompt(app_factory):
    app = waiting_app(app_factory)
    in_background(app, "printf 'READY>'; read x")
    nothing_for(5, lambda: notification_box(app, C))


def test_password_prompt_shows_a_lock_and_refuses_automation(app_factory):
    app = waiting_app(app_factory, commands=[{"name": "type it", "actions": [{"type": "macro\n"}]}])
    send(app, SECRET)
    app.wait_title("1 ", timeout=15)
    app.snap("a lock while the program reads a password")
    # remote and palette text never reach it, both say why
    refused = msg(app, "send-text", "remote\n")
    assert refused.returncode != 0 and "asks for a password" in refused.stderr, refused
    time.sleep(1)
    assert not app.file("secret").exists(), app.file("secret").read_text()
    app.palette("type it")
    app.wait(lambda: notification_box(app, C), msg="refused notice")
    time.sleep(1)
    assert not app.file("secret").exists(), app.file("secret").read_text()
    # the keyboard still can
    app.type("kb")
    app.key("Return")
    app.wait(lambda: app.file("secret").exists() and app.file("secret").read_text() == "kb",
             msg="typed password went in")
    app.wait_title("1 D", timeout=15)


def test_remote_input_works_again_after_the_password_prompt(app_factory):
    app = waiting_app(app_factory)
    send(app, SECRET)
    app.wait_title("1 ", timeout=15)
    app.type("x")
    app.key("Return")
    app.wait_title("1 D", timeout=15)
    send(app, "touch $T2_TMP/after")
    app.wait(app.file("after").exists, msg="remote input typed again")


def bottom_banner(app: App):
    """bbox of the attention colored banner in the bottom right corner"""
    img = app.shot()
    h, w = img.shape[:2]
    mask = near(img[h * 3 // 4 :, w // 2 :], C["attention"])
    return mask_bbox(mask) if mask.sum() > 100 else None


def screen_moves(app: App, seconds: float = 1.5) -> bool:
    before = text(app)
    time.sleep(seconds)
    return text(app) != before


LOOP = "stty ixon; i=0; while :; do i=$((i+1)); echo tick $i; sleep 0.1; done"


def test_ctrl_s_pauses_shows_a_banner_and_ctrl_q_resumes(app_factory):
    app = waiting_app(app_factory)
    send(app, LOOP)
    wait_text(app, "tick 5")
    app.key("ctrl+s")
    app.wait(lambda: bottom_banner(app), msg="paused banner")
    assert not screen_moves(app)
    app.snap("output paused by ctrl+s, the banner says how to go on")
    app.key("ctrl+q")
    app.wait(lambda: bottom_banner(app) is None, msg="banner gone")
    assert screen_moves(app)


def test_send_xoff_off_never_pauses(app_factory):
    app = waiting_app(app_factory, terminal={"send_xoff": False})
    send(app, LOOP)
    wait_text(app, "tick 5")
    app.key("ctrl+s")
    time.sleep(1)
    assert bottom_banner(app) is None
    assert screen_moves(app)


@pytest.mark.parametrize("send_xoff", [True, False])
def test_ctrl_s_reaches_programs_that_turn_flow_control_off(app_factory, send_xoff):
    """with ixon off, like in emacs or nano, ctrl+s is a key for the program either way"""
    app = waiting_app(app_factory, terminal={"send_xoff": send_xoff})
    send(app, "stty -ixon raw -echo; head -c1 | od -An -tx1 > $T2_TMP/key; stty sane")
    time.sleep(2)
    app.key("ctrl+s")
    app.wait(lambda: app.file("key").exists() and app.file("key").read_text().strip() == "13",
             msg="ctrl+s read by the program")
    assert bottom_banner(app) is None


def test_stopped_job_banner_click_brings_it_back(app_factory):
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["command"]}, themes={"dark": THEME})
    app.mouse(450, 300)
    app.run("sleep 100")
    app.wait_title("sleep")
    app.key("ctrl+z")
    app.wait_title("bash")
    box = app.wait(lambda: bottom_banner(app), msg="stopped job banner", timeout=15)
    app.snap("sleep stopped with ctrl+z, shown at the bottom")
    w, h = app.size()
    app.click(w // 2 + (box[0] + box[2]) // 2, h * 3 // 4 + (box[1] + box[3]) // 2)
    app.wait_title("sleep")
    app.wait(lambda: bottom_banner(app) is None, msg="banner gone once it runs again", timeout=15)


def test_two_stopped_jobs_each_come_back(app_factory):
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["command"]}, themes={"dark": THEME})
    app.run("sleep 101")
    app.wait_title("sleep")
    app.key("ctrl+z")
    app.wait_title("bash")
    app.run("less /etc/hostname")
    app.wait_title("less")
    app.key("ctrl+z")
    app.wait_title("bash")
    box = app.wait(lambda: (b := bottom_banner(app)) and b[3] - b[1] > 20 and b, msg="two jobs listed",
                   timeout=15)
    app.snap("two stopped jobs")
    w, h = app.size()
    # the lower entry is the second job, less
    app.click(w // 2 + (box[0] + box[2]) // 2, h * 3 // 4 + box[3] - 5)
    app.wait_title("less")


def test_closing_a_tab_with_a_stopped_job_asks(app_factory):
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["number", {"text": " "}, "command"],
                       "hide_bar_for_one_tab": False}, themes={"dark": THEME})
    app.mouse(450, 300)
    app.key("ctrl+shift+t")
    app.wait_title("2 bash")
    app.run("sleep 102")
    app.wait_title("2 sleep")
    app.key("ctrl+z")
    app.wait_title("2 bash")
    time.sleep(1.5)
    app.key("ctrl+shift+w")
    app.wait(lambda: dialog(app), msg="close dialog for the stopped job")
    app.snap("closing asks about the stopped sleep")
    app.key("Escape")
    app.wait(lambda: dialog(app) is None, msg="dialog closed")
    assert app.title() == "2 bash"


def test_closing_without_stopped_jobs_does_not_ask(app_factory):
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["number"], "hide_bar_for_one_tab": False},
                      themes={"dark": THEME})
    app.key("ctrl+shift+t")
    app.wait_title("2")
    app.key("ctrl+shift+w")
    app.wait_title("1")
