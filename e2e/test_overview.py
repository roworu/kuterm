"""tab overview: a grid of previews of every tab of the window, typing filters them by title,
arrows or a click pick one, escape closes it. covers groups, pinned, sleeping, incognito,
sandboxed, alternate screen and split tabs, and what happens on a config reload"""

import tempfile
import time
from pathlib import Path

import numpy as np
import pytest

from harness import App, as_rgb, ink, mask_bbox, near, unique_theme
from kit import executable, integrated, msg, nothing_for, ready, send, tabs, text, wait_text

FEATURE = "tab overview"

THEME = unique_theme(0xd1)
C = as_rgb(THEME)
# a full line printed into a tab, wide enough to reach across a preview box
FULL = "M" * 60
# ink columns of a printed line of `FULL` in a preview, against ~60 for the prompt line
LINE_COLUMNS = 120


def overview_app(app_factory, **settings) -> App:
    """an app with the overview on ctrl+shift+o, remote control on and the first tab ready"""
    base = integrated(window_title=["number"], keybindings={"tab_overview": "ctrl-shift-o"},
                      # the source gutter shifts the grid, the pixel checks want the plain one
                      shell_integration={"mark_sources": "none"})
    base.update(settings)
    app = app_factory(base, themes={"dark": THEME})
    ready(app)
    # keep the pointer off the tab bar, hover recolors the tabs
    app.mouse(450, 300)
    return app


def new_tabs(app: App, count: int) -> None:
    """ctrl+shift+t until there are `count` tabs, the window title is the active tab number"""
    for number in range(2, count + 1):
        app.key("ctrl+shift+t")
        app.wait_title(str(number))


def tab_ids(app: App) -> list:
    return [str(tab["id"]) for tab in tabs(app)]


def active(app: App) -> str:
    return next(str(tab["id"]) for tab in tabs(app) if tab["active"])


def show(app: App, ix: int) -> None:
    """make the tab at `ix` the active one and wait for the window to say so"""
    assert msg(app, "focus-tab", "--pane", tab_ids(app)[ix]).returncode == 0
    app.wait_title(str(ix + 1))


def fill(app: App, lines: int, pane: str, drawn: bool = True) -> None:
    """clear a tab's screen and print `lines` full lines, its preview is then that many rows.

    the preview reads the snapshot a painted tab takes, so the call waits for this one to reach the
    screen; a hidden tab, or one under the overview, never paints there, its caller waits instead"""
    parts = "; ".join(f"printf '{FULL}\\n'" for _ in range(lines))
    send(app, f"clear; {parts}", pane)
    wait_text(app, FULL[:20], pane)
    if drawn:
        app.wait_line(lines - 1, len(FULL), top=app.top())


def spans(mask: np.ndarray) -> list:
    """(first, end) of every run of True in a one dimensional mask"""
    index = np.flatnonzero(mask)
    if len(index) == 0:
        return []
    return [(int(g[0]), int(g[-1]) + 1) for g in np.split(index, np.flatnonzero(np.diff(index) > 1) + 1)]


def previews(app: App, img=None) -> list:
    """(x0, y0, x1, y1) of every preview box on screen, in reading order.

    the boxes are the only thing painted in the terminal background while the overview is up"""
    img = app.shot() if img is None else img
    boxes = []
    for y0, y1 in spans(near(img, C["terminal_background"], 2).any(axis=1)):
        # a box is 9 rems tall, the band of the terminal behind the overlay is much taller
        if not 40 < y1 - y0 < 320:
            continue
        for x0, x1 in spans(near(img[y0:y1], C["terminal_background"], 2).any(axis=0)):
            if x1 - x0 > 60:
                boxes.append((x0, y0, x1, y1))
    return boxes


def box_ink(app: App, box, img=None) -> np.ndarray:
    """ink of the text drawn in a preview box, the rounded edges and padding left out"""
    img = app.shot() if img is None else img
    x0, y0, x1, y1 = box
    return ink(img[y0 + 3 : y1 - 3, x0 + 3 : x1 - 3], C["terminal_background"], 40)


def text_rows(app: App, box, img=None, columns: int = LINE_COLUMNS) -> int:
    """printed lines in a preview box: the shell's prompt line is far too narrow to count"""
    mask = box_ink(app, box, img)
    return sum(1 for start, end in spans(mask.any(axis=1)) if mask[start:end].any(axis=0).sum() > columns)


def rows(app: App, img=None) -> list:
    """what each preview holds, one printed line per row of text"""
    img = app.shot() if img is None else img
    return [text_rows(app, box, img) for box in previews(app, img)]


def marked(app: App, img=None) -> list:
    """indices of the boxes drawn as picked, the picked one wears the cursor colored border"""
    img = app.shot() if img is None else img
    picked = []
    for ix, (x0, y0, x1, y1) in enumerate(previews(app, img)):
        ring = img[max(0, y0 - 6) : y1 + 6, max(0, x0 - 6) : x1 + 6]
        if near(ring, C["cursor"], 3).sum() > 200:
            picked.append(ix)
    return picked


def overview_open(app: App, img=None) -> bool:
    """true while the overview covers the window: its filter box paints the top of the window"""
    img = app.shot() if img is None else img
    return near(img[6:46, 300:840], C["tab_active_background"], 3).mean() > 0.5


def open_overview(app: App) -> None:
    app.key("ctrl+shift+o")
    app.wait(lambda: overview_open(app), msg="the overview open")


def set_titles(app: App) -> None:
    """give every tab a title of its own, so the filter has something to match"""
    for name, tab in zip(["alpha", "beta", "gamma"], tabs(app)):
        assert msg(app, "set-tab-title", "--pane", str(tab["id"]), name).returncode == 0


def asleep(app: App, pane: str) -> bool:
    return next(tab["asleep"] for tab in tabs(app) if str(tab["id"]) == pane)


def test_every_tab_gets_a_preview_of_its_own_screen(app_factory):
    """the grid holds one preview per tab of the window, each showing that tab's own screen"""
    app = overview_app(app_factory)
    # one, two and three printed lines, so each preview can be told apart from the others
    for lines in range(1, 4):
        if lines > 1:
            app.key("ctrl+shift+t")
            app.wait_title(str(lines))
        fill(app, lines, active(app))
    open_overview(app)
    app.wait(lambda: len(previews(app)) == 3, msg=f"three previews, got {len(previews(app))}")
    app.snap("three tabs, each preview showing the lines its own tab printed")
    assert rows(app) == [1, 2, 3]


def test_the_active_tab_is_marked_and_up_down_move_the_pick(app_factory):
    """the grid starts on the preview of the active tab, up and down move that mark"""
    app = overview_app(app_factory)
    new_tabs(app, 3)
    open_overview(app)
    app.wait(lambda: len(previews(app)) == 3, msg="three previews")
    # the third tab is the active one, so its preview is the picked one
    assert marked(app) == [2], marked(app)
    app.snap("three tabs, the active one's preview picked")
    # two columns, so from the second cell of the second row up is the first cell
    app.key("Up")
    app.wait(lambda: marked(app) == [0], msg=f"up picks the first preview, got {marked(app)}")
    app.key("Down")
    app.wait(lambda: marked(app) == [2], msg=f"down picks it back, got {marked(app)}")
    app.key("Down")
    app.wait(lambda: marked(app) == [2], msg=f"there is no row below, got {marked(app)}")
    app.snap("up and down moved the pick, the last row is a dead end")


def test_left_and_right_move_the_pick_between_the_cells_of_a_row(app_factory):
    """left and right move the pick one cell sideways, like up and down do vertically"""
    app = overview_app(app_factory)
    new_tabs(app, 3)
    open_overview(app)
    app.wait(lambda: len(previews(app)) == 3, msg="three previews")
    assert marked(app) == [2], marked(app)
    app.key("Left")
    app.wait(lambda: marked(app) == [1], msg=f"left picks the second preview, got {marked(app)}")
    app.key("Right")
    app.wait(lambda: marked(app) == [2], msg=f"right picks it back, got {marked(app)}")
    # from the first cell left stays there
    app.key("Up")
    app.wait(lambda: marked(app) == [0], msg="up picks the first")
    app.key("Left")
    app.wait(lambda: marked(app) == [0], msg=f"left has nowhere to go, got {marked(app)}")


def test_enter_switches_to_the_picked_tab(app_factory):
    """an arrow then enter switches to the picked tab, the overview closes with it"""
    app = overview_app(app_factory)
    new_tabs(app, 3)
    open_overview(app)
    app.wait(lambda: len(previews(app)) == 3, msg="three previews")
    app.key("Up")
    app.wait(lambda: marked(app) == [0], msg="the first preview picked")
    app.key("Return")
    app.wait_title("1")
    app.wait(lambda: not overview_open(app), msg="the overview closed")
    app.snap("enter switched to the first tab")


def test_a_click_switches_to_the_picked_tab(app_factory):
    """a click on a preview switches to that tab"""
    app = overview_app(app_factory)
    new_tabs(app, 3)
    open_overview(app)
    app.wait(lambda: len(previews(app)) == 3, msg="three previews")
    # three tabs make two columns, the third is the first cell of the second row
    boxes = previews(app)
    assert boxes[2][1] > boxes[0][1], boxes
    x0, y0, x1, y1 = boxes[2]
    app.click((x0 + x1) // 2, (y0 + y1) // 2)
    app.wait_title("3")
    app.wait(lambda: not overview_open(app), msg="the overview closed")
    app.snap("a click on the third preview switched to it")
    # the first preview switches back
    open_overview(app)
    app.wait(lambda: len(previews(app)) == 3, msg="three previews again")
    x0, y0, x1, y1 = previews(app)[0]
    app.click((x0 + x1) // 2, (y0 + y1) // 2)
    app.wait_title("1")


def test_a_click_switches_to_a_tab_in_another_pane(app_factory):
    """a preview of a tab in another pane switches to it and focuses that pane"""
    app = overview_app(app_factory)
    send(app, "export WHO=left")
    app.key("ctrl+shift+t")
    app.wait_title("2")
    send(app, "export WHO=right")
    # the split moves the active tab into a new pane on the right
    app.palette("split right")
    app.wait(lambda: len(tabs(app)) == 2 and len({tab["pane"] for tab in tabs(app)}) == 2,
             msg="two panes")
    app.mouse(450, 300)
    open_overview(app)
    app.wait(lambda: len(previews(app)) == 2, msg="a preview per tab of both panes")
    app.snap("two panes, both tabs previewed")
    # the left pane's tab holds the first preview, a click on it focuses the left pane
    x0, y0, x1, y1 = previews(app)[0]
    app.click((x0 + x1) // 2, (y0 + y1) // 2)
    app.wait(lambda: not overview_open(app), msg="the overview closed")
    send(app, 'echo "$WHO" > $T2_TMP/who')
    assert app.wait_file("who").strip() == "left"
    app.snap("a click on the left pane's preview typed into the left pane")


def test_escape_closes_without_switching(app_factory):
    """escape leaves the tab that was active before the overview alone"""
    app = overview_app(app_factory)
    new_tabs(app, 3)
    open_overview(app)
    app.wait(lambda: len(previews(app)) == 3, msg="three previews")
    app.key("Up")
    app.wait(lambda: marked(app) == [0], msg="the first preview picked")
    app.key("Escape")
    app.wait(lambda: not overview_open(app), msg="the overview closed")
    assert app.title() == "3", app.title()
    app.snap("escape closed the overview on the third tab")


def test_typing_filters_previews_by_title(app_factory):
    """typing part of a title leaves only the matching previews, backspace brings the others"""
    app = overview_app(app_factory)
    new_tabs(app, 3)
    for lines in range(1, 4):
        show(app, lines - 1)
        fill(app, lines, tab_ids(app)[lines - 1])
    set_titles(app)
    open_overview(app)
    app.wait(lambda: len(previews(app)) == 3, msg="three previews")
    app.type("et")
    app.wait(lambda: len(previews(app)) == 1, msg=f"only beta matches, got {len(previews(app))}")
    assert rows(app) == [2], f"the beta preview, got {rows(app)}"
    app.snap('typing "et" left only the beta preview')
    # backspace widens the filter again
    app.key("BackSpace", "BackSpace")
    app.wait(lambda: len(previews(app)) == 3, msg=f"all three back, got {len(previews(app))}")
    app.snap("backspace brought the other previews back")
    assert rows(app) == [1, 2, 3], rows(app)


def test_a_query_matching_nothing_keeps_the_overview_usable(app_factory):
    """a filter matching no tab shows an empty grid, escape still closes it"""
    app = overview_app(app_factory)
    new_tabs(app, 2)
    set_titles(app)
    open_overview(app)
    app.wait(lambda: len(previews(app)) == 2, msg="two previews")
    app.type("zzz")
    app.wait(lambda: len(previews(app)) == 0, msg=f"an empty grid, got {len(previews(app))}")
    assert overview_open(app), "the overview closed on its own"
    app.snap("a filter matching nothing: an empty grid")
    app.key("BackSpace", "BackSpace", "BackSpace")
    app.wait(lambda: len(previews(app)) == 2, msg="the previews came back")
    app.type("zzz")
    app.wait(lambda: len(previews(app)) == 0, msg="empty again")
    app.key("Escape")
    app.wait(lambda: not overview_open(app), msg="escape still closed it")
    assert app.title() == "2", app.title()


def test_the_preview_of_the_shown_tab_follows_new_output(app_factory):
    """output printed in the tab on screen while the overview is open shows up in its preview"""
    app = overview_app(app_factory)
    new_tabs(app, 2)
    open_overview(app)
    app.wait(lambda: len(previews(app)) == 2, msg="two previews")
    assert rows(app) == [0, 0], rows(app)
    # remote input reaches the second tab, the one on screen behind the overview
    fill(app, 4, tab_ids(app)[1], drawn=False)
    app.wait(lambda: rows(app)[1] == 4, msg=f"the second preview followed the output, got {rows(app)}")
    app.snap("output printed while the overview was open appeared in its preview")


def test_the_preview_of_a_hidden_tab_follows_its_output(app_factory):
    """output printed in a tab that is not on screen while the overview is open shows up too"""
    app = overview_app(app_factory)
    new_tabs(app, 2)
    open_overview(app)
    app.wait(lambda: len(previews(app)) == 2, msg="two previews")
    before = rows(app)
    fill(app, 5, tab_ids(app)[0], drawn=False)
    app.wait(lambda: rows(app)[0] == 5,
             msg=f"the hidden tab's preview stayed at {before}, now {rows(app)}")
    app.snap("output printed into a hidden tab appeared in its preview")


def test_a_collapsed_groups_tabs_are_still_previewed(app_factory):
    """a group's tabs hide behind its chip in the tab bar but keep their own previews"""
    app = overview_app(app_factory)
    new_tabs(app, 3)
    for lines in range(1, 4):
        show(app, lines - 1)
        fill(app, lines, tab_ids(app)[lines - 1])
    # the second tab makes a group of its own, the third joins it
    show(app, 1)
    app.palette("new group")
    show(app, 2)
    app.palette("add to group")

    def chip():
        # a group paints a 2px line in its color under its tabs too, the chip is the tall part
        bar = app.shot()[: app.bar_height() - 6]
        return mask_bbox(near(bar, C["ansi"][1], 3))

    app.wait(lambda: chip() is not None, msg="the group chip in the tab bar")
    before = chip()
    app.snap("two of the three tabs are in a group")
    # a click on the chip collapses the group, the label grows by the count
    x0, y0, x1, y1 = before
    app.click((x0 + x1) // 2, (y0 + y1) // 2)
    app.wait(lambda: chip() and chip()[2] - chip()[0] > (x1 - x0) + 10,
             msg=f"the chip of the collapsed group, was {before}, now {chip()}")
    open_overview(app)
    app.wait(lambda: len(previews(app)) == 3,
             msg=f"every tab of the group still previewed, got {len(previews(app))}")
    app.snap("three previews, two of their tabs collapsed under a chip")
    assert rows(app) == [1, 2, 3], rows(app)


def test_a_pinned_tab_is_previewed_where_it_moved(app_factory):
    """pinning a tab moves it to the front of the pane, its preview moves with it"""
    app = overview_app(app_factory)
    new_tabs(app, 3)
    for lines in range(1, 4):
        show(app, lines - 1)
        fill(app, lines, tab_ids(app)[lines - 1])
    set_titles(app)
    open_overview(app)
    app.wait(lambda: len(previews(app)) == 3, msg="three previews")
    assert rows(app) == [1, 2, 3], rows(app)
    app.key("Escape")
    app.wait(lambda: not overview_open(app), msg="the overview closed")
    app.palette("pin or unpin")
    app.wait(lambda: [tab["title"] for tab in tabs(app)] == ["gamma", "alpha", "beta"],
             msg=f"the pinned tab first, order {[t['title'] for t in tabs(app)]}")
    open_overview(app)
    app.wait(lambda: len(previews(app)) == 3, msg="three previews after pinning")
    app.snap("the pinned third tab previewed first")
    assert rows(app) == [3, 1, 2], rows(app)


def test_a_sleeping_tab_is_previewed_without_waking_it(app_factory):
    """a tab whose history is packed away still shows its screen, and stays asleep"""
    app = overview_app(app_factory, sleep_tabs_after_minutes=0.02, terminal={"max_history_length": 0})
    pane = tab_ids(app)[0]
    send(app, "clear; seq 1 60")
    wait_text(app, "60")
    app.key("ctrl+shift+t")
    app.wait_title("2")
    app.wait(lambda: asleep(app, pane), msg="the hidden tab asleep", timeout=15)
    open_overview(app)
    app.wait(lambda: len(previews(app)) == 2, msg="two previews")
    app.snap("a sleeping tab previewed with the screen it had")
    # its screen is the numbers `seq` printed, one per row, short lines
    assert text_rows(app, previews(app)[0], columns=1) > 20, "the sleeping tab's screen is in its preview"
    # looking at a preview is not showing, searching or reading that tab, so it stays asleep
    nothing_for(2, lambda: not asleep(app, pane))


def test_an_incognito_tab_is_previewed_but_cannot_be_read(app_factory):
    """an incognito tab has a preview like any other; its text still can't be read over the socket"""
    app = overview_app(app_factory)
    app.palette("new incognito tab")
    app.wait(lambda: len(tabs(app)) == 2, msg="an incognito tab")
    pane = next(str(tab["id"]) for tab in tabs(app) if tab["active"])
    # get-text refuses an incognito pane, so a file says the command ran
    send(app, f"clear; printf '{FULL}\\n'; touch $T2_TMP/private_done", pane)
    app.wait_file("private_done")
    assert "incognito" in msg(app, "get-text", "--pane", pane).stderr
    open_overview(app)
    app.wait(lambda: len(previews(app)) == 2, msg="two previews")
    app.snap("an incognito tab previewed beside an ordinary one")
    # the overview draws the pane as it is on screen, the socket still refuses to read it
    assert rows(app) == [0, 1], rows(app)


def test_a_sandboxed_tab_is_previewed(app_factory):
    """a sandboxed tab's screen is previewed like any other tab's"""
    app = sandbox_app(app_factory)
    assert msg(app, "new-tab", "--profile", "box").returncode == 0
    app.wait(lambda: len(tabs(app)) == 2, msg="the sandboxed tab")
    app.wait(lambda: app.file("bwrap_args").exists(), msg="the sandboxed shell started")
    # the ordinary tab first, it is on screen with clean pixels
    show(app, 0)
    fill(app, 1, tab_ids(app)[0])
    show(app, 1)
    open_overview(app)
    app.wait(lambda: len(previews(app)) == 2, msg="two previews")
    # the frame drawn around a sandboxed pane puts ink of its own in every row, so the preview
    # of the tab the fill below goes to is the only oracle left
    fill(app, 2, tab_ids(app)[1], drawn=False)
    app.wait(lambda: rows(app) == [1, 2], msg=f"both screens previewed, got {rows(app)}")
    app.snap("an ordinary tab and a sandboxed one, each previewed")


def test_more_tabs_than_fit_scroll(app_factory):
    """a grid taller than the window scrolls with the wheel, the last row comes into view whole"""
    app = overview_app(app_factory)
    new_tabs(app, 9)
    for lines in range(1, 10):
        show(app, lines - 1)
        fill(app, lines, tab_ids(app)[lines - 1])
    open_overview(app)
    app.wait(lambda: len(previews(app)) == 9 and rows(app) == list(range(1, 10)),
             msg=f"a preview per tab, got {rows(app)}")

    def heights() -> list:
        return [y1 - y0 for _, y0, _, y1 in previews(app)]

    def bottom() -> tuple:
        return max(previews(app), key=lambda box: box[1])

    # three columns by three rows: the last row of boxes is cut off by the bottom of the window
    app.wait(lambda: heights()[8] < heights()[0] - 40,
             msg=f"the last row is cut short, {heights()}")
    app.snap("nine tabs: two rows of previews and a third one cut off")
    assert max(heights()[:6]) - min(heights()[:6]) < 4, f"the first two rows are whole, {heights()}"
    x0, y0, x1, y1 = previews(app)[4]
    app.wheel((x0 + x1) // 2, (y0 + y1) // 2, up=False, clicks=10)
    app.wait(lambda: bottom()[3] - bottom()[1] > 150,
             msg=f"the last row scrolled whole, {heights()}")
    app.snap("scrolled to the last row of previews")


def test_a_single_tab_fills_the_grid(app_factory):
    """one tab makes a single wide preview of its whole screen"""
    app = overview_app(app_factory)
    fill(app, 4, active(app))
    open_overview(app)
    app.wait(lambda: len(previews(app)) == 1, msg="one preview")
    x0, y0, x1, y1 = previews(app)[0]
    assert x1 - x0 > 700, f"the only preview is wide, {x1 - x0}px"
    assert rows(app) == [4], rows(app)
    app.snap("one tab: a single preview across the window")


def test_an_alternate_screen_program_is_previewed(app_factory):
    """what a pager draws on the alternate screen is in the preview, and leaves it when it quits"""
    app = overview_app(app_factory)
    send(app, f"clear; printf '{FULL}\\n{FULL}\\n{FULL}\\n' > $T2_TMP/page; less $T2_TMP/page")
    wait_text(app, FULL[:20])
    open_overview(app)
    app.wait(lambda: len(previews(app)) == 1, msg="one preview")
    app.wait(lambda: text_rows(app, previews(app)[0]) >= 3,
             msg="the file on the alternate screen, rows "
                 f"{text_rows(app, previews(app)[0])}")
    app.snap("less on the alternate screen, its file visible in the preview")
    app.key("Escape")
    app.wait(lambda: not overview_open(app), msg="the overview closed")
    app.key("q")
    app.wait(lambda: FULL[:20] not in text(app), msg="less quit and the shell is back")
    open_overview(app)
    app.wait(lambda: len(previews(app)) == 1, msg="the preview again")
    app.wait(lambda: text_rows(app, previews(app)[0]) == 0,
             msg="the file is gone from the preview, rows "
                 f"{text_rows(app, previews(app)[0])}")
    app.snap("after quitting less the preview shows the shell again")


def test_a_config_reload_with_the_overview_open(app_factory):
    """saving settings.jsonc reloads the config while the overview is open"""
    app = overview_app(app_factory)
    new_tabs(app, 2)
    open_overview(app)
    app.wait(lambda: len(previews(app)) == 2, msg="two previews")
    app.write("settings.jsonc", {**app.settings, "window_title": [{"text": "reloaded"}]})
    app.wait_title("reloaded")
    time.sleep(1)
    assert overview_open(app), "the overview closed on its own when the config reloaded"
    app.snap("the overview still open after a config reload")
    app.key("Escape")
    app.wait(lambda: not overview_open(app), msg="escape closed the overview")


def test_closing_a_tab_from_under_the_overview(app_factory):
    """closing a tab with its key while the overview is up leaves the app alive, the grid keeps
    the previews it built until it is reopened"""
    app = overview_app(app_factory)
    new_tabs(app, 3)
    open_overview(app)
    app.wait(lambda: len(previews(app)) == 3, msg="three previews")
    app.key("ctrl+shift+w")
    app.wait(lambda: len(tabs(app)) == 2, msg="the active tab closed")
    # the grid was built when the overview opened, the closed tab's preview stays in it
    time.sleep(1.5)
    assert app.alive(), app.output()[-500:]
    app.snap("the active tab closed from under the overview")
    assert len(previews(app)) == 3, f"the grid was rebuilt, {len(previews(app))} previews"


def test_the_overview_still_takes_keys_after_a_tab_closed_under_it(app_factory):
    """the overview stays in charge when a tab closes under it: escape closes it, typing filters"""
    app = overview_app(app_factory)
    new_tabs(app, 3)
    open_overview(app)
    app.wait(lambda: len(previews(app)) == 3, msg="three previews")
    app.key("ctrl+shift+w")
    app.wait(lambda: len(tabs(app)) == 2, msg="the active tab closed")
    app.key("Escape")
    app.wait(lambda: not overview_open(app),
             msg=f"escape still closes it, {len(previews(app))} previews left")
    app.type("zz")
    app.wait(lambda: len(previews(app)) == 0, msg="typing still filters the grid")


def sandbox_app(app_factory) -> App:
    """an app with a sandboxed profile whose bwrap writes its arguments and runs the shell"""
    stub = Path(tempfile.mkdtemp(prefix="e2e-overview-bwrap-"))
    executable(stub / "bwrap", '#!/bin/sh\nprintf "%s\\n" "$@" > "$T2_TMP/bwrap_args"\n'
                                'while [ "$1" != "--" ]; do shift; done; shift; exec "$@"\n')
    profile = {"name": "box", "command": {"program": "/bin/bash"}, "env": {"PS1": "$ "},
               "working_directory": "/tmp", "sandbox": {"network": False, "write": ["cwd"], "tmp": "shared"}}
    base = integrated(window_title=["number"], keybindings={"tab_overview": "ctrl-shift-o"},
                      shell_integration={"mark_sources": "none"})
    base["profiles"] = base["profiles"] + [profile]
    app = app_factory(base, themes={"dark": THEME}, env={"PATH": f"{stub}:/usr/bin:/bin"})
    ready(app)
    app.mouse(450, 300)
    return app
