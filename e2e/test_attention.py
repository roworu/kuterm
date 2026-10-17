"""ai agents waiting for the user: every waiting agent marks its own tab, the next-attention
key takes the one waiting longest, the picker lists how long each waited, and the mark follows
the theme, other windows, tab groups, bells, dialogs and the palette"""

import os
import time

import numpy as np
import pytest

from harness import App, as_rgb, count, dominant, ink_bbox, mask_bbox, near, unique_theme
from kit import executable, listening, msg, notification_box, nothing_for, tabs

FEATURE = "agents waiting"

THEME = unique_theme(0x6a)
C = as_rgb(THEME)
THEME2 = unique_theme(0x6c)
C2 = as_rgb(THEME2)

# tabs are tab_width wide, so the dot of tab N sits in the columns [N*TAB_WIDTH, (N+1)*TAB_WIDTH)
TAB_WIDTH = 200

# a program named like an ai cli, in a group marked with "agents": true. its icon says the tab
# bar saw it running, the "work"/"idle"/"answered" files say where the program itself is
AGENT_GROUP = {"interpreters": ["python3"],
               "groups": [{"icon": "A", "agents": True, "commands": ["claude"]}]}
KEYS = {"next_attention_tab": "ctrl-shift-a", "pick_agent_tab": "ctrl-shift-g"}

# reports busy over osc 9;4 like an agent does, waits to be let go on, then stops being busy
# without finishing, the way an agent does when it asks to run a tool
AGENT = '''#!/usr/bin/python3
import os, sys, time
me = sys.argv[1] if len(sys.argv) > 1 else "1"
tmp = os.environ["T2_TMP"]
def wait(name):
    while not os.path.exists(os.path.join(tmp, name + me)):
        time.sleep(0.02)
def say(text):
    sys.stdout.write(text)
    sys.stdout.flush()
say("\\033]9;4;3\\007working %s\\n" % me)
open(os.path.join(tmp, "work" + me), "w").write("1")
wait("go")
say("\\033]9;4;0\\007allow? ")
open(os.path.join(tmp, "idle" + me), "w").write("1")
input()
open(os.path.join(tmp, "answered" + me), "w").write("1")
time.sleep(1000)
'''

# works, waits, works again over osc 9;4 and waits a second time
AGENT_AGAIN = '''#!/usr/bin/python3
import os, sys, time
tmp = os.environ["T2_TMP"]
def wait(name):
    while not os.path.exists(os.path.join(tmp, name)):
        time.sleep(0.02)
def say(text):
    sys.stdout.write(text)
    sys.stdout.flush()
say("\\033]9;4;3\\007working\\n")
open(os.path.join(tmp, "work1"), "w").write("1")
wait("go1")
say("\\033]9;4;0\\007allow? ")
open(os.path.join(tmp, "idle1"), "w").write("1")
wait("busy1")
say("\\033]9;4;3\\007thinking again\\n")
wait("go2")
say("\\033]9;4;0\\007allow again? ")
open(os.path.join(tmp, "idle2"), "w").write("1")
input()
'''

# rings the bell in the same breath as stopping to be busy
AGENT_BELL = AGENT.replace('say("\\033]9;4;0\\007allow? ")', 'say("\\033]9;4;0\\007\\aallow? ")')
assert AGENT_BELL != AGENT, "the bell script has to differ from the plain one"


def agent_app(app_factory, tab_icons: dict | None = None, commands: list | None = None, **extra) -> App:
    """an app with its own theme, a tab bar that is always shown and the attention keys"""
    settings = {"theme": {"mode": "dark"}, "window_title": ["number", {"text": " "}, "icon"],
                "tab_title": ["number"], "hide_bar_for_one_tab": False, "tab_width": TAB_WIDTH,
                "icons": {"default": "D"}, "notifications": {"timeout": 0}, "remote_control": "socket"}
    for key, value in extra.items():
        if isinstance(value, dict) and isinstance(settings.get(key), dict):
            settings[key] = {**settings[key], **value}
        else:
            settings[key] = value
    app = app_factory(settings, keybindings=KEYS, tab_icons=tab_icons or AGENT_GROUP,
                      commands={"commands": commands} if commands else None, themes={"dark": THEME})
    app.mouse(450, 300)
    return app


def install_agent(app: App, name: str = "claude", script: str = AGENT) -> None:
    """put the fake agent program and a link named like an ai cli into the case folder"""
    path = executable(app.file("agent.py"), script)
    os.makedirs(app.file("bin"), exist_ok=True)
    os.symlink(path, app.file(f"bin/{name}"))


def start_agent(app: App, me: str = "1", name: str = "claude") -> None:
    """run the agent in the active tab and wait until the tab bar saw it running"""
    app.run(f"$T2_TMP/bin/{name} {me}")
    app.wait_file(f"work{me}")


def go(app: App, me: str = "1") -> None:
    """let the agent of this number stop being busy"""
    app.file(f"go{me}").touch()


def marked(app: App) -> int:
    """attention colored pixels in the tab bar"""
    return count(app.shot()[: app.bar_height()], C["attention"], 3)


def marks(app: App) -> list:
    """(x0, x1) columns of every attention colored dot in the tab bar, left to right"""
    columns = near(app.shot()[: app.bar_height()], C["attention"], 3).any(axis=0).nonzero()[0]
    runs = []
    for x in columns:
        if runs and x - runs[-1][1] <= 4:
            runs[-1][1] = int(x)
        else:
            runs.append([int(x), int(x)])
    return [tuple(run) for run in runs]


def tab_pixels(app: App, ix: int, color: tuple, tol: int = 3) -> int:
    """pixels of `color` inside the rect of tab `ix`"""
    return count(app.shot()[: app.bar_height(), ix * TAB_WIDTH : (ix + 1) * TAB_WIDTH], color, tol)


def palette_box(app: App, img=None) -> tuple | None:
    """bbox of the palette panel, drawn in the tab bar color, away from the notifications"""
    img = app.shot() if img is None else img
    mask = near(img, C["tab_bar_background"], 2)
    mask[: app.bar_height()] = False
    # notifications in the bottom right corner use the same color
    mask[img.shape[0] // 2 :] = False
    return mask_bbox(mask) if mask.sum() > 5000 else None


def divider(img: np.ndarray, box: tuple) -> int:
    """y of the border colored line between the query and the list"""
    x0, y0, x1, y1 = box
    rows = np.flatnonzero(near(img[y0:y1, x0 + 5 : x1 - 5], C["border"]).mean(axis=1) > 0.9) + y0
    return int(rows[rows > y0 + 5].min())


def list_area(app: App) -> tuple:
    """(x0, y0, x1, y1) of the list of the open picker, below its query line"""
    img = app.shot()
    box = palette_box(app, img)
    assert box is not None, "the picker is not open"
    return box[0] + 3, divider(img, box) + 2, box[2] - 12, box[3] - 2


def picker_rows(app: App) -> list:
    """(x0, y0, x1, y1) of the text of every row of the open picker, top to bottom"""
    img = app.shot()
    box = palette_box(app, img)
    if box is None:
        return []
    x0, x1 = box[0] + 3, box[2] - 12
    top = divider(img, box) + 2
    area = img[top : box[3] - 2, x0:x1]
    bands, start = [], None
    lines = [int((np.abs(line.astype(int) - np.median(line, axis=0)).max(axis=1) > 40).sum()) > 1
             for line in area]
    for y, ink in enumerate(lines + [False]):
        if ink and start is None:
            start = y
        elif not ink and start is not None:
            # a gap of a few pixels is the padding inside one row
            if y - start > 4:
                bands.append((start, y))
            start = None
    rows = []
    for y0, y1 in bands:
        band = img[top + y0 : top + y1, x0:x1]
        text = ink_bbox(band, dominant(band), 40)
        if text is not None:
            rows.append((x0 + text[0], top + y0 + text[1], x0 + text[2], top + y0 + text[3]))
    return rows


def notification_bands(app: App) -> list:
    """(y0, y1) rows of every painted panel of the notification area, top to bottom"""
    img = app.shot()
    h, w = img.shape[:2]
    mask = near(img[h // 2 :, w // 2 :], C["tab_bar_background"])
    bands = []
    for y in mask.any(axis=1).nonzero()[0]:
        if bands and y - bands[-1][1] <= 2:
            bands[-1][1] = int(y)
        else:
            bands.append([int(y), int(y)])
    return [(a + h // 2, b + h // 2) for a, b in bands]


def settled_box(app: App, timeout: float = 10.0) -> tuple | None:
    """bbox of the notification panels, once two polls in a row show the same one"""
    seen = []

    def check():
        seen.append(notification_box(app, C))
        if len(seen) > 1 and seen[-1] is not None and seen[-1] == seen[-2]:
            return seen[-1]
        return None

    try:
        return app.wait(check, msg="the notifications in the corner to settle", timeout=timeout)
    except AssertionError:
        img = app.shot()
        h, w = img.shape[:2]
        corner = img[h // 2 :, w // 2 :]
        raise AssertionError(
            f"the boxes never settled: {seen[-10:]}, hover px "
            f"{int(near(corner, C['tab_active_background'], 2).sum())}, bar px "
            f"{int(near(corner, C['tab_bar_background'], 2).sum())}, title {app.title()!r}"
        ) from None


def dialog(app: App) -> tuple | None:
    """bbox of the danger colored button of the close dialog"""
    mask = near(app.shot(), C["danger_button"])
    return mask_bbox(mask) if mask.sum() > 200 else None


def test_two_agents_mark_their_own_tabs_and_the_key_takes_the_longest_wait(app_factory):
    """two waiting agents put a dot on each tab in the theme color, the next-attention key goes
    to the one that has waited longer, and typing an answer away from a tab only clears that
    tab's dot"""
    app = agent_app(app_factory)
    install_agent(app)
    start_agent(app, "1")
    app.wait_title("1 A")
    go(app, "1")
    app.wait_file("idle1")
    app.wait(lambda: marked(app) > 4, msg="tab 1 marked")
    app.key("ctrl+shift+t")
    app.wait_title("2 D")
    start_agent(app, "2")
    app.wait_title("2 A")
    go(app, "2")
    app.wait_file("idle2")
    app.wait(lambda: marked(app) > 4, msg="tab 2 marked")
    app.wait(lambda: len(marks(app)) == 2, msg="a dot on each tab")
    app.snap("two waiting agents, a dot on each of their tabs")
    for ix in (0, 1):
        assert tab_pixels(app, ix, C["attention"], 0) > 4, marks(app)
    # tab 1 stopped being busy first, so it has waited longer
    app.key("ctrl+shift+a")
    app.wait_title("1 A")
    app.key("ctrl+shift+a")
    app.wait_title("2 A")
    # an answer reaches only the tab it is typed into
    app.type("y")
    app.key("Return")
    app.wait_file("answered2")
    app.wait(lambda: tab_pixels(app, 1, C["attention"]) == 0, msg="tab 2 unmarked by the answer")
    assert tab_pixels(app, 0, C["attention"]) > 4
    app.key("ctrl+shift+a")
    app.wait_title("1 A")


def test_a_shown_tab_keeps_its_mark_until_an_answer_is_typed(app_factory):
    """looking at the waiting tab does not clear its mark, typing an answer there does"""
    app = agent_app(app_factory)
    install_agent(app)
    start_agent(app, "1")
    app.wait_title("1 A")
    go(app, "1")
    app.wait_file("idle1")
    app.wait(lambda: marked(app) > 4, msg="tab 1 marked")
    app.key("ctrl+shift+t")
    app.wait_title("2 D")
    app.key("ctrl+shift+a")
    app.wait_title("1 A")
    app.snap("the waiting tab shown and focused, its dot still there")
    nothing_for(3, lambda: marked(app) == 0)  # it was not answered by looking at it
    app.type("y")
    app.key("Return")
    app.wait_file("answered1")
    app.wait(lambda: marked(app) == 0, msg="the answer cleared the mark")


def test_the_picker_lists_the_waits_longest_first_and_picking_switches(app_factory):
    """pick_agent_tab lists one row per agent tab with how long it waited, longest on top: the
    older wait has a digit more, return picks it and the second row picks the other tab"""
    app = agent_app(app_factory)
    install_agent(app)
    start_agent(app, "1")
    app.wait_title("1 A")
    go(app, "1")
    app.wait(lambda: marked(app) > 4, msg="tab 1 marked")
    app.key("ctrl+shift+t")
    app.wait_title("2 D")
    start_agent(app, "2")
    app.wait_title("2 A")
    time.sleep(11)  # so the waits read as "1Xs" and "Xs" instead of the same number of digits
    go(app, "2")
    app.wait_file("idle2")
    app.wait(lambda: marked(app) > 4, msg="tab 2 marked")
    app.key("ctrl+shift+g")
    rows = app.wait(lambda: (found := picker_rows(app)) and len(found) == 2 and found,
                    msg="one row per waiting agent")
    app.snap("the picker saying how long each agent waited")
    # both rows list the same tab title, so the extra digit of "waiting 1Xs" is one glyph
    assert rows[0][2] - rows[0][0] >= rows[1][2] - rows[1][0] + 7, rows
    app.key("Return")  # the longest wait is listed first
    app.wait_title("1 A")
    app.key("ctrl+shift+g")
    app.wait(lambda: len(picker_rows(app)) == 2, msg="the picker again")
    app.key("Down", "Return")
    app.wait_title("2 A")


def test_escape_closes_the_picker_without_switching(app_factory):
    """escape leaves the picker and the active tab where it was"""
    app = agent_app(app_factory)
    install_agent(app)
    start_agent(app, "1")
    app.wait_title("1 A")
    go(app, "1")
    app.wait(lambda: marked(app) > 4, msg="tab 1 marked")
    app.key("ctrl+shift+t")
    app.wait_title("2 D")
    app.key("ctrl+shift+g")
    app.wait(lambda: picker_rows(app), msg="the picker open")
    app.key("Escape")
    app.wait(lambda: palette_box(app) is None, msg="the picker closed")
    assert app.title() == "2 D"
    assert marked(app) > 4


def test_the_picker_without_any_agent_says_so(app_factory):
    """with no agent at all the picker shows one muted line and enter picks nothing"""
    app = agent_app(app_factory)
    app.key("ctrl+shift+t")
    app.wait_title("2 D")
    app.key("ctrl+shift+g")
    rows = app.wait(lambda: picker_rows(app), msg="the picker with an empty list")
    assert len(rows) == 1, rows
    x0, y0, x1, y1 = list_area(app)
    area = app.shot()[y0:y1, x0:x1]
    # the muted "no matching commands" line, nothing listed in the text color
    assert count(area, C["text_muted"], 3) > 20
    assert count(area, C["text"], 3) == 0
    app.snap("the picker saying that no agent waits")
    app.key("Return")
    time.sleep(1)  # nothing was picked, it stays open
    assert palette_box(app) is not None
    assert app.title() == "2 D"
    app.key("Escape")
    app.wait(lambda: palette_box(app) is None, msg="closed")


def test_an_agent_that_keeps_working_is_listed_but_not_marked(app_factory):
    """an agent that never stopped being busy is listed by the picker, but its tab is not
    marked and nothing is notified"""
    app = agent_app(app_factory)
    install_agent(app)
    start_agent(app, "1")
    app.wait_title("1 A")
    assert marks(app) == []
    assert notification_box(app, C) is None
    app.key("ctrl+shift+g")
    assert len(app.wait(lambda: picker_rows(app), msg="the working agent listed")) == 1
    app.key("Escape")


def test_agent_waiting_off_never_marks_or_notifies(app_factory):
    """notifications.agent_waiting false: the agent stops being busy out of sight, its tab
    stays unmarked and nothing is shown, though the program still counts as an agent"""
    app = agent_app(app_factory, notifications={"agent_waiting": False})
    install_agent(app)
    start_agent(app, "1")
    app.wait_title("1 A")
    app.key("ctrl+shift+t")
    app.wait_title("2 D")
    go(app, "1")
    app.wait_file("idle1")
    nothing_for(4, lambda: marks(app) or notification_box(app, C))
    app.key("ctrl+shift+g")
    assert len(app.wait(lambda: picker_rows(app), msg="the agent tab listed")) == 1


@pytest.mark.parametrize("name, groups", [
    ("other", [{"icon": "A", "agents": True, "commands": ["claude"]}]),
    ("claude", [{"icon": "A", "agents": False, "commands": ["claude"]}]),
], ids=["no agent program", "agents false"])
def test_a_program_that_is_no_agent_never_waits(app_factory, name, groups):
    """only a program of a group with "agents": true waits: another program of it and a group
    with agents false stop being busy without marking anything"""
    app = agent_app(app_factory, tab_icons={"interpreters": ["python3"], "groups": groups})
    install_agent(app, name=name)
    start_agent(app, "1", name=name)
    go(app, "1")
    app.wait_file("idle1")
    nothing_for(4, lambda: marks(app) or notification_box(app, C))


def test_an_agent_marks_even_without_dynamic_icons(app_factory):
    """icons.dynamic false keeps the default icon on the tab, the program still counts as an
    agent and its tab is marked"""
    app = agent_app(app_factory, icons={"dynamic": False})
    install_agent(app)
    start_agent(app, "1")
    app.wait_title("1 D")
    # the picker lists the tab, so the app saw the program as an agent although the tab shows
    # the default icon of the setting instead of the one of the agent group
    app.key("ctrl+shift+g")
    app.wait(lambda: picker_rows(app), msg="the agent known to the picker")
    app.key("Escape")
    app.wait(lambda: palette_box(app) is None, msg="the picker closed")
    go(app, "1")
    app.wait_file("idle1")
    app.wait(lambda: marked(app) > 4, msg="marked under the default icon")
    assert "A" not in app.title()  # the icon stayed the default one


def test_an_agent_that_reports_busy_again_loses_its_mark(app_factory):
    """an agent reporting busy again over osc 9;4 ends its wait, and a new wait after that is
    marked again"""
    app = agent_app(app_factory)
    install_agent(app, script=AGENT_AGAIN)
    start_agent(app, "1")
    app.wait_title("1 A")
    go(app, "1")
    app.wait_file("idle1")
    app.wait(lambda: marked(app) > 4, msg="marked while it waits")
    app.file("busy1").touch()
    app.wait(lambda: marked(app) == 0, msg="unmarked while it works again")
    app.file("go2").touch()
    app.wait_file("idle2")
    app.wait(lambda: marked(app) > 4, msg="marked at the second wait")
    app.snap("the second wait of the same agent marked again")


def test_each_notification_switches_to_its_own_tab(app_factory):
    """two agents wait in tabs behind the shown one: their notifications stack, and clicking
    each switches to the tab it is about"""
    app = agent_app(app_factory)
    install_agent(app)
    start_agent(app, "1")
    app.wait_title("1 A")
    app.key("ctrl+shift+t")
    app.wait_title("2 D")
    start_agent(app, "2")
    app.wait_title("2 A")
    app.key("ctrl+shift+t")
    app.wait_title("3 D")
    go(app, "1")
    app.wait_file("idle1")
    go(app, "2")
    app.wait_file("idle2")
    box = settled_box(app)
    app.snap("the agents of the hidden tabs wait, notified in the corner")
    assert len(notification_bands(app)) == 2, "one notification per waiting agent"
    # the first agent to stop being busy is the top one, tab 1. its notification goes away
    # without moving the other one, which a click under the pointer would color as hovered
    app.click((box[0] + box[2]) // 2, box[1] + 8)
    app.wait_title("1 A")
    box = settled_box(app)
    app.click((box[0] + box[2]) // 2, (box[1] + box[3]) // 2)
    app.wait_title("2 A")
    app.wait(lambda: notification_box(app, C) is None, msg="both notifications clicked away")


def test_the_mark_uses_the_theme_color_and_follows_a_reload(app_factory):
    """the dot is exactly the attention color of the theme, and saving a theme with another
    one recolors it while the agent keeps waiting"""
    app = agent_app(app_factory)
    install_agent(app)
    start_agent(app, "1")
    app.wait_title("1 A")
    go(app, "1")
    app.wait(lambda: marked(app) > 4, msg="marked in the theme color")
    bar = app.shot()[: app.bar_height()]
    assert count(bar, C["attention"], 0) > 4  # the exact color, not a shade of it
    assert count(bar, C2["attention"], 0) == 0
    app.snap("the dot in the attention color of the dark theme")
    theme = {"dark": THEME2}
    app.write("settings.jsonc", {**app.settings, "theme": {**app.settings["theme"], "themes": theme}})
    app.wait(lambda: count(app.shot()[: app.bar_height()], C2["attention"], 0) > 4,
             msg="the dot in the attention color of the new theme")
    assert count(app.shot()[: app.bar_height()], C["attention"], 0) == 0
    assert marked(app) > 4  # it still waits
    app.snap("the same wait colored by the reloaded theme")


def test_a_waiting_agent_of_another_window_is_marked_and_notified(app_factory):
    """an agent of a tab in a window that is not focused still marks its tab and notifies,
    and the attention key of that window goes to it"""
    app = agent_app(app_factory)
    install_agent(app)
    listening(app)
    start_agent(app, "1")
    app.wait_title("1 A")
    app.key("ctrl+shift+t")
    app.wait_title("2 D")
    assert msg(app, "new-window").returncode == 0
    app.wait(lambda: len({tab["window"] for tab in tabs(app)}) == 2, msg="a second window")
    go(app, "1")
    app.wait_file("idle1")
    app.focus()  # back to the window whose tab holds the agent
    app.wait(lambda: marked(app) > 4, msg="the tab of the other window marked")
    app.wait(lambda: notification_box(app, C), msg="notified although the window was not focused")
    app.snap("the agent of the other window waits")
    app.key("ctrl+shift+a")
    app.wait_title("1 A")


def test_a_waiting_agent_in_a_grouped_tab_is_still_marked(app_factory):
    """the chip of a tab group shifts its tabs to the right, the dot follows its tab and the
    key still switches to it"""
    app = agent_app(app_factory, commands=[{"name": "group it", "category": "zz",
                                            "actions": ["new_group"]}])
    install_agent(app)
    start_agent(app, "1")
    app.wait_title("1 A")
    go(app, "1")
    app.wait(lambda: marks(app), msg="tab 1 marked")
    before = marks(app)[0][0]
    app.palette("zz: group it")  # the active tab goes into a group of its own
    app.wait(lambda: (found := marks(app)) and found[0][0] >= before + 10 and found,
             msg="the dot moved right with its tab")
    app.snap("the agent of a grouped tab waits, its dot behind the group chip")
    app.key("ctrl+shift+t")
    app.wait_title("2 D")
    app.key("ctrl+shift+a")
    app.wait_title("1 A")


def test_a_bell_and_a_wait_show_on_the_same_tab(app_factory):
    """a ring and a wait can land in the same hidden tab: showing the tab clears the bell,
    only an answer clears the dot"""
    app = agent_app(app_factory)
    install_agent(app, script=AGENT_BELL)
    start_agent(app, "1")
    app.wait_title("1 A")
    app.key("ctrl+shift+t")
    app.wait_title("2 D")
    # the bell icon is the only text colored thing on a tab that is not shown
    assert tab_pixels(app, 0, C["text"], 30) == 0
    go(app, "1")
    app.wait_file("idle1")
    app.wait(lambda: marked(app) > 4, msg="the waiting tab marked")
    app.wait(lambda: tab_pixels(app, 0, C["text"], 30) > 20, msg="the bell icon on the tab")
    app.snap("a bell and a wait on the same tab")
    app.key("ctrl+shift+a")
    app.wait_title("1 A")
    app.key("ctrl+Tab")
    app.wait_title("2 D")
    app.wait(lambda: tab_pixels(app, 0, C["text"], 30) == 0, msg="the bell gone once it was shown")
    assert marked(app) > 4  # showing it did not answer it
    app.key("ctrl+shift+a")
    app.wait_title("1 A")
    app.type("y")
    app.key("Return")
    app.wait_file("answered1")
    app.wait(lambda: marked(app) == 0, msg="the answer cleared the wait")


def test_the_mark_stays_with_the_close_dialog_open(app_factory):
    """asking to close a tab with a running program shows the dialog over the terminal, and
    the agent of the other tab stays marked behind it"""
    icons = {"interpreters": ["python3"],
             "groups": [{"icon": "A", "agents": True, "commands": ["claude"]},
                        {"icon": "S", "commands": ["sleep"]}]}
    app = agent_app(app_factory, tab_icons=icons)
    install_agent(app)
    start_agent(app, "1")
    app.wait_title("1 A")
    go(app, "1")
    app.wait(lambda: marked(app) > 4, msg="tab 1 marked")
    app.key("ctrl+shift+t")
    app.wait_title("2 D")
    app.run("sleep 300")
    app.wait_title("2 S")
    app.key("ctrl+shift+w")
    app.wait(lambda: dialog(app), msg="the close dialog")
    assert marked(app) > 4
    app.snap("the close dialog over the terminal, the waiting agent still marked")
    app.key("Escape")
    app.wait(lambda: dialog(app) is None, msg="the dialog closed")
    assert marked(app) > 4
    assert app.title() == "2 S"


def test_the_palette_goes_to_the_waiting_agent(app_factory):
    """the bundled "go to waiting agent" command switches to the marked tab while the palette
    is open over the terminal, and switching does not answer the agent"""
    app = agent_app(app_factory)
    install_agent(app)
    start_agent(app, "1")
    app.wait_title("1 A")
    go(app, "1")
    app.wait(lambda: marked(app) > 4, msg="tab 1 marked")
    app.key("ctrl+shift+t")
    app.wait_title("2 D")
    app.key("ctrl+shift+p")
    app.wait(lambda: palette_box(app), msg="the palette open")
    assert marked(app) > 4
    app.type("tabs: go to waiting agent")
    app.key("Return")
    app.wait_title("1 A")
    app.snap("switched to the waiting agent from the palette")
    assert marked(app) > 4
