"""tab groups: the colored chip in the tab bar before the tabs of a group, the palette commands
that make, fill, empty and close one, project groups that follow the shell into git folders, and
a group through pinning, renaming, closing, moving between panes, sleeping and session restore"""

import json
from pathlib import Path

import numpy as np
import pytest

from harness import (App, as_rgb, bundled_theme, close_to, count, dominant, ink, mask_bbox, near,
                     unique_theme)
from kit import listening, nothing_for, tabs as tab_list, text as pane_text, wait_text

FEATURE = "tab groups"

THEME = unique_theme(0x71)
C = as_rgb(THEME)
BUNDLED = bundled_theme(True)

# one key per group command, so a test does not have to open the palette every time
KEYS = {"new_group": "ctrl-shift-g", "add_tab_to_group": "ctrl-shift-d",
        "remove_tab_from_group": "ctrl-shift-r", "close_group": "ctrl-shift-l",
        "tab_overview": "ctrl-shift-o"}


def group_app(app_factory, custom=THEME, **settings) -> App:
    """dark theme with colors of its own (the bundled dark one when `custom` is None), the tab
    bar always shown with 200px tabs, tab number, running program and terminal title in the
    window title, and the group commands on keys of their own"""
    base = {"theme": {"mode": "dark"},
            "window_title": ["number", {"text": " "}, "command", {"text": " "}, "title"],
            "tab_title": ["number", {"text": " "}, "title"],
            "hide_bar_for_one_tab": False, "tab_width": 200, "remote_control": "socket"}
    extra = {} if custom is None else {"themes": {"dark": custom}}
    app = app_factory({**base, **settings}, keybindings=KEYS, **extra)
    # keep the pointer away from the bar, hover changes its colors
    app.mouse(450, 580)
    return app


def title_tab(app: App, name: str) -> None:
    """give the active tab a terminal title of its own, readable in the window title"""
    app.run(f"printf '\\033]0;{name}\\007'")
    app.wait_title(lambda title: title.endswith(f" {name}"))


def name_tabs(app: App, names: list) -> None:
    """one tab per name, each with the name as its terminal title, the last one active"""
    for ix, name in enumerate(names):
        if ix:
            app.key("ctrl+shift+t")
        title_tab(app, name)


def repo(app: App, name: str) -> Path:
    """a folder that looks like a git repository to the project search"""
    root = app.file(name)
    (root / ".git").mkdir(parents=True, exist_ok=True)
    return root


def runs(cols) -> list:
    """(first, last) of each run of column numbers, gaps of up to 2 columns are bridged"""
    if not len(cols):
        return []
    return [(int(run[0]), int(run[-1])) for run in np.split(cols, np.flatnonzero(np.diff(cols) > 3) + 1)]


def group_masks(app: App, img=None, pal=C) -> dict:
    """per ansi color of the theme the pixels of the tab bar in it, chips and their lines share
    the color of their group, a chip is tall and the line under a tab only 2px"""
    img = app.shot() if img is None else img
    h = app.bar_height()
    return {ix: near(img[1:h], pal["ansi"][ix], 2) for ix in range(1, 7)}


def chips(app: App, img=None, pal=C) -> list:
    """bboxes of the group chips in the tab bar, left to right: blocks of columns tall as the
    bar in one of the group colors, the 2px line under a tab is not one"""
    found = []
    for mask in group_masks(app, img, pal).values():
        for x0, x1 in runs(np.flatnonzero(mask.sum(axis=0) >= 8)):
            rows = np.flatnonzero(mask[:, x0 : x1 + 1].mean(axis=1) > 0.5)
            found.append((x0, int(rows[0]) + 1, x1 + 1, int(rows[-1]) + 2))
    return sorted(found)


def chip_colour(app: App, box: tuple, img=None) -> int:
    """ansi index of the fill of the chip in `box`"""
    img = app.shot() if img is None else img
    x0, y0, x1, y1 = box
    fill = dominant(img[y0 + 2 : y1 - 2, x0 + 2 : x1 - 2])
    for ix in range(1, 7):
        if close_to(fill, C["ansi"][ix], 4):
            return ix
    raise AssertionError(f"the chip is not in a group color: {fill}")


def chip_label(app: App, box: tuple, img=None) -> int:
    """width of the name drawn inside the chip in `box`, in the terminal background color"""
    img = app.shot() if img is None else img
    x0, y0, x1, y1 = box
    mask = near(img[y0 + 1 : y1 - 1, x0 + 1 : x1 - 1], C["terminal_background"], 12)
    cols = np.flatnonzero(mask.any(axis=0))
    return 0 if not len(cols) else int(cols[-1] - cols[0]) + 1


def chip_width(box: tuple) -> int:
    return box[2] - box[0]


def underlines(app: App, img=None, pal=C) -> list:
    """(x0, x1, ansi index) of every group colored line under the tabs of a group, told apart
    from the chip of the same color by its height: a line is a wide strip of a few short columns,
    a chip column is tall and a column without the color at all is not part of a line"""
    found = []
    for ix, mask in group_masks(app, img, pal).items():
        heights = mask.sum(axis=0)
        for x0, x1 in runs(np.flatnonzero((heights > 0) & (heights <= 6))):
            if x1 - x0 >= 40:
                found.append((x0, x1 + 1, ix))
    return sorted(found)


def span(line: tuple) -> int:
    return line[1] - line[0]


def edges(app: App, img=None) -> list:
    """x of every tab border column in the middle row of the bar, the right edge of each tab"""
    img = app.shot() if img is None else img
    row = near(img[app.bar_height() // 2 : app.bar_height() // 2 + 1], C["border"])[0]
    return [int(x) for x in np.flatnonzero(row)]


def menu_rows(app: App) -> list:
    """centre of every row of the popup menu drawn below the tab bar, top to bottom"""
    h = app.bar_height()

    def find():
        mask = near(app.shot()[h:], C["tab_bar_background"], 2)
        return mask_bbox(mask) if mask.sum() > 200 else None

    x0, y0, x1, y1 = app.wait(find, msg="popup menu")
    inner = app.shot()[h + y0 + 2 : h + y1 - 2, x0 + 2 : x1 - 2]
    rows = np.flatnonzero(ink(inner, C["tab_bar_background"]).any(axis=1))
    rows = np.split(rows, np.flatnonzero(np.diff(rows) > 2) + 1)
    return [(x0 + (x1 - x0) // 2, h + y0 + 2 + int(row.mean())) for row in rows]


def chip_menu(app: App, box: tuple) -> list:
    """open the menu of the chip in `box` with a right click on its lower edge, which keeps the
    whole menu below the bar, and return the centre of its rows"""
    app.click((box[0] + box[2]) // 2, app.bar_height() - 6, button=3)
    return menu_rows(app)


def dialog(app: App) -> tuple:
    """bbox of the danger colored close button of the confirmation, None while it is closed"""
    mask = near(app.shot(), C["danger_button"])
    return mask_bbox(mask) if mask.sum() > 200 else None


def group_of(app: App, first: int, others: list) -> None:
    """put the tab numbered `first` and the tabs in `others` (1 based) into one group by hand"""
    app.key(f"alt+{first}")
    app.key("ctrl+shift+g")
    for ix in others:
        app.key(f"alt+{ix}")
        app.key("ctrl+shift+d")


def two_grouped(app: App) -> None:
    """two named tabs in the group of the first one, the second one active"""
    name_tabs(app, ["one", "two"])
    group_of(app, 1, [2])
    app.wait(lambda: underlines(app) and span(underlines(app)[0]) > 350, msg="both tabs under one line")
    app.key("alt+2")
    app.wait_title(lambda title: title.endswith(" two"))


def slept(app: App, number: int) -> bool:
    """whether the tab numbered `number` has its history packed away"""
    return bool(next(tab["asleep"] for tab in tab_list(app) if tab["number"] == number))


def overview_open(app: App) -> int:
    """pixels the overview draws of the frame around its picked preview; the terminal cursor is
    the only other thing in that color and is far smaller"""
    return count(app.shot(), C["cursor"], 2)


def ink_width(app: App, x0: int, x1: int) -> int:
    """width of the title ink drawn in the middle rows of the bar between x0 and x1, on the
    background of the active tab"""
    img = app.shot()
    h = app.bar_height()
    region = img[3 : h - 3, x0:x1]
    mask = ink(region, C["tab_active_background"], 40)
    cols = np.flatnonzero(mask.any(axis=0))
    return 0 if not len(cols) else int(cols[-1] - cols[0]) + 1


def test_chip_shows_group_colour_name_and_underline(app_factory):
    """new group from the palette puts a chip left of its tab: the fill is the first group color,
    the name is written inside it in the terminal background color, a line of the same color runs
    under the tab which is moved right by the chip and its margins, and a second group on the
    same tab takes the next color"""
    app = group_app(app_factory)
    title_tab(app, "one")
    app.wait_still("the bar settled")
    assert chips(app) == [] and not underlines(app)
    app.palette("new group")
    box = app.wait(lambda: chips(app) and chips(app)[0], msg="group chip")
    app.snap("one tab put into a new group by the palette")
    h = app.bar_height()
    assert chip_colour(app, box) == 1, "the first group made by hand takes the first group color"
    # 0.25rem of margin around the chip, the bar is 2rem tall
    assert abs(box[0] - 4) <= 2 and abs(box[1] - 4) <= 3, box
    # the chip is inset by the same margin at the bottom of the bar as at the top
    assert abs((h - 1 - box[3]) - box[1]) <= 2, (box, h)
    assert chip_label(app, box) > 20, f"the name of the group is drawn in the chip, {box}"
    line = app.wait(lambda: underlines(app) or None, msg="the line under the tab")
    assert line[0][2] == 1 and abs(span(line[0]) - 200) <= 3, line
    assert abs(line[0][0] - (box[2] + 4)) <= 2, (line, box)
    assert abs(edges(app)[0] - (box[2] + 4 + 199)) <= 2, (edges(app), box)
    app.palette("new group")
    app.wait(lambda: len(chips(app)) == 1 and chip_colour(app, chips(app)[0]) == 2,
             msg="the next group color on the tab")
    app.snap("a second group recolored the chip")


def test_first_group_uses_the_bundled_theme_colors(app_factory):
    """with no theme of its own the chip and its line use the ansi colors of the bundled dark
    theme, not colors of some other theme"""
    app = group_app(app_factory, custom=None)
    title_tab(app, "one")
    app.key("ctrl+shift+g")
    boxes = app.wait(lambda: chips(app, pal=BUNDLED) or None, msg="chip")
    x0, y0, x1, y1 = boxes[0]
    img = app.shot()
    fill = dominant(img[y0 + 2 : y1 - 2, x0 + 2 : x1 - 2])
    assert close_to(fill, BUNDLED["ansi"][1], 4), (fill, BUNDLED["ansi"][1])
    line = app.wait(lambda: underlines(app, pal=BUNDLED) or None, msg="the line of the group color")
    assert line[0][2] == 1 and abs(span(line[0]) - 200) <= 3, line
    app.snap("the group drawn in the bundled dark theme")


def test_add_and_remove_tab_from_group_and_forgotten_groups(app_factory):
    """add to group makes a new group when neither neighbour is in one, remove takes the tab out
    again and the emptied group is forgotten, while with no group at all both leave the tab
    alone"""
    app = group_app(app_factory)
    title_tab(app, "one")
    # one tab, nothing grouped next to it: add to group makes a new group
    app.palette("add to group")
    box = app.wait(lambda: chips(app) and chips(app)[0], msg="chip of the new group")
    assert chip_colour(app, box) == 1 and span(underlines(app)[0]) <= 205
    # the only tab of the group leaves it and the group is gone
    app.palette("remove from group")
    app.wait(lambda: not chips(app), msg="the emptied group forgot itself")
    assert not underlines(app)
    # nothing grouped: removing again does nothing
    app.palette("remove from group")
    app.palette("remove from group")
    nothing_for(1, lambda: chips(app))
    assert app.title().endswith(" one") and len(edges(app)) == 1
    app.snap("a group of one, removed, and a tab without a group")
    # a tab whose neighbour is in a group joins it
    app.key("ctrl+shift+t")
    title_tab(app, "two")
    app.key("ctrl+shift+g")
    app.wait(lambda: chips(app), msg="group of the second tab")
    app.key("alt+1")
    app.key("ctrl+shift+d")
    app.wait(lambda: underlines(app) and span(underlines(app)[0]) > 350, msg="both tabs under one line")
    assert chip_colour(app, chips(app)[0]) == 1
    app.snap("the first tab joined the group of the tab next to it")


def test_collapsed_group_hides_its_tabs_behind_the_chip(app_factory):
    """clicking the chip hides the tabs of its group, the active one stays on screen and the chip
    says how many it hides; clicking again shows them"""
    app = group_app(app_factory)
    name_tabs(app, ["one", "two", "three"])
    group_of(app, 2, [3])
    app.wait(lambda: len(edges(app)) == 3 and chips(app), msg="three tabs, the last two grouped")
    box = chips(app)[0]
    app.snap("three tabs, the second and third one grouped")
    expanded = chip_label(app, box)
    # the third tab is active: collapsing hides the second one
    app.click((box[0] + box[2]) // 2, app.bar_height() // 2)
    app.wait(lambda: len(edges(app)) == 2, msg="the second tab hidden behind the chip")
    app.snap("collapsed group: only the active tab of the group is drawn")
    back = chips(app)[0]
    assert chip_label(app, back) > expanded, "the chip says how many tabs it hides"
    # the active tab of a collapsed group always shows, whichever tab of it is active
    app.key("alt+2")
    app.wait_title(lambda title: title.endswith(" two"))
    app.wait_still("the bar settled on the other tab")
    assert len(edges(app)) == 2, edges(app)
    app.key("alt+1")
    app.wait_title(lambda title: title.endswith(" one"))
    app.wait(lambda: len(edges(app)) == 1, msg="both tabs of the group hidden")
    app.snap("with an ungrouped tab active the whole group is hidden")
    # a click brings them back
    app.click((back[0] + back[2]) // 2, app.bar_height() // 2)
    app.wait(lambda: len(edges(app)) == 3, msg="the tabs shown again")
    assert abs(chip_label(app, chips(app)[0]) - expanded) <= 1
    app.snap("the group expanded again")


def test_chip_menu_renames_recolors_collapses_ungroups_and_closes(app_factory):
    """the right click menu of a chip: rename gives the group a name of its own, collapse hides
    its tabs, change color walks the group colors, ungroup frees the tabs, close group closes
    them and leaves the tabs outside the group alone"""
    app = group_app(app_factory)
    name_tabs(app, ["one", "two", "three"])
    group_of(app, 1, [2])
    app.wait(lambda: underlines(app) and span(underlines(app)[0]) > 350, msg="two tabs grouped")
    before = chips(app)[0]
    rows = chip_menu(app, before)
    assert len(rows) == 5, rows
    app.snap("the menu of a group chip")
    # rename: the field opens with the name selected, so typing replaces it
    app.click(*rows[0])
    app.type("a much longer name")
    app.key("Return")
    app.wait(lambda: chip_width(chips(app)[0]) > chip_width(before) + 40,
             msg="the chip shows the new name")
    renamed = chips(app)[0]
    app.snap("the chip renamed")
    # collapse hides the second tab of the group, the third one stays
    app.key("alt+1")
    app.click(*chip_menu(app, renamed)[1])
    app.wait(lambda: len(edges(app)) == 2, msg="the collapsed group hides its second tab")
    app.snap("collapsed from the menu, the renamed chip")
    # change color walks to the next ansi color
    app.click(*chip_menu(app, chips(app)[0])[2])
    app.wait(lambda: chip_colour(app, chips(app)[0]) == 2, msg="the next group color")
    app.wait(lambda: underlines(app) and underlines(app)[0][2] == 2, msg="the line follows the color")
    # ungroup keeps the tabs and drops the chip
    app.click(*chip_menu(app, chips(app)[0])[3])
    app.wait(lambda: not chips(app), msg="the chip gone")
    assert not underlines(app) and len(edges(app)) == 3
    app.snap("ungrouped from the menu, three tabs without a chip")
    # close group closes its tabs and keeps the one outside
    group_of(app, 1, [2])
    app.wait(lambda: chips(app), msg="grouped again")
    app.click(*chip_menu(app, chips(app)[0])[4])
    app.wait(lambda: len(edges(app)) == 1, msg="the two tabs of the group closed")
    assert app.title().endswith(" three") and not chips(app)
    app.snap("close group left the ungrouped tab")


def test_project_group_follows_the_shell_between_repos(app_factory):
    """by_project: a tab whose shell moves into a git folder joins a group named after the
    project, tabs of the same project share it, another project gets a chip of its own, and
    moving out of the project leaves the group"""
    app = group_app(app_factory, tab_groups={"by_project": True})
    first = repo(app, "repo-alpha")
    second = repo(app, "a-much-longer-project-name")
    title_tab(app, "one")
    assert chips(app) == []
    app.run(f"cd {first}")
    box = app.wait(lambda: chips(app) and chips(app)[0], msg="the group of the project")
    label = chip_label(app, box)
    assert chip_colour(app, box) in range(1, 7) and label > 30
    app.snap("the shell moved into repo-alpha: a chip named after the project")
    # another tab of the same project joins the group
    app.key("ctrl+shift+t")
    title_tab(app, "two")
    app.run(f"cd {first}")
    app.wait(lambda: len(chips(app)) == 1 and underlines(app) and span(underlines(app)[0]) > 350,
             msg="both tabs in one project group")
    assert abs(chip_label(app, chips(app)[0]) - label) <= 1
    # another project gets a chip of its own, named after the longer folder
    app.key("ctrl+shift+t")
    title_tab(app, "three")
    app.run(f"cd {second}")
    app.wait(lambda: len(chips(app)) == 2, msg="a chip per project")
    other = max(chips(app), key=chip_width)
    assert chip_label(app, other) > label + 40, (chip_label(app, other), label)
    app.snap("two projects side by side, each tab under the chip of its project")
    # moving out of the project leaves the group and forgets it
    app.run("cd $T2_TMP")
    app.wait(lambda: len(chips(app)) == 1, msg="the group left with the shell")
    assert span(underlines(app)[0]) > 350, "the other project's group stayed whole"
    app.snap("the shell left the project and its group is gone")


def test_project_grouping_leaves_hand_made_groups_alone(app_factory):
    """a tab grouped by hand stays in its group when its shell enters a git folder, while a tab
    without a group joins the project's"""
    app = group_app(app_factory, tab_groups={"by_project": True})
    repo(app, "hand-project")
    title_tab(app, "one")
    app.key("ctrl+shift+g")
    box = app.wait(lambda: chips(app) and chips(app)[0], msg="the hand made chip")
    colour = chip_colour(app, box)
    h = app.bar_height()
    app.wait_still("the bar settled")
    before = app.shot()[:h].copy()
    app.run("cd $T2_TMP/hand-project")
    # the tab keeps its hand made group: nothing about the bar changes
    nothing_for(3, lambda: not np.array_equal(app.shot()[:h], before))
    app.snap("the shell is inside the project, the hand made group untouched")
    # a tab without a group does join the project
    app.key("ctrl+shift+t")
    title_tab(app, "two")
    app.run("cd $T2_TMP/hand-project")
    app.wait(lambda: len(chips(app)) == 2, msg="the project group of the new tab")
    assert chip_colour(app, chips(app)[0]) == colour, "the hand made chip kept its color"
    app.snap("the second tab grouped by its project, next to the hand made group")


def test_project_groups_follow_a_reload_of_by_project(app_factory):
    """turning by_project on while the app runs groups the tabs of a project on the next title
    refresh, leaving a hand made group untouched"""
    app = group_app(app_factory)
    repo(app, "later-project")
    title_tab(app, "one")
    app.key("ctrl+shift+g")
    box = app.wait(lambda: chips(app) and chips(app)[0], msg="the hand made chip")
    colour = chip_colour(app, box)
    app.key("ctrl+shift+t")
    title_tab(app, "two")
    app.run("cd $T2_TMP/later-project")
    nothing_for(2, lambda: len(chips(app)) > 1)
    assert chips(app)[0][0] == box[0]
    app.snap("two tabs, one hand made group, none of the project yet")
    app.write("settings.jsonc", {**app.settings, "tab_groups": {"by_project": True}})
    app.wait(lambda: len(chips(app)) == 2, msg="the project group after the reload")
    assert chip_colour(app, chips(app)[0]) == colour, "the hand made group kept its color"
    app.snap("by_project turned on live: the second tab joined its project")


def test_close_group_closes_all_its_tabs(app_factory):
    """close group from the palette closes every tab of the group and leaves the ones outside it"""
    app = group_app(app_factory, confirm_quit="never")
    name_tabs(app, ["one", "two", "three"])
    group_of(app, 2, [3])
    app.wait(lambda: underlines(app) and span(underlines(app)[0]) > 350, msg="grouped")
    app.palette("close group")
    app.wait(lambda: len(edges(app)) == 1, msg="the two tabs of the group closed")
    assert app.title().endswith(" one") and not chips(app)
    app.snap("the group closed, the tab outside it kept")


def test_close_group_with_a_running_program_closes_the_whole_group(app_factory):
    """close group asks once about the tab of the group running a program and closes every tab
    of the group after the answer"""
    app = group_app(app_factory, confirm_quit="never")
    name_tabs(app, ["one", "two", "three"])
    group_of(app, 2, [3])
    app.wait(lambda: underlines(app) and span(underlines(app)[0]) > 350, msg="grouped")
    app.key("alt+2")
    app.run("sleep 1000")
    app.wait_title(lambda title: " sleep " in title)
    app.key("alt+3")
    app.palette("close group")
    app.wait(lambda: dialog(app), msg="the group closed with a question")
    app.snap("closing a group whose first tab runs a program asked about it")
    app.key("Return")
    app.wait(lambda: len(edges(app)) == 1 and not chips(app), timeout=4,
             msg="the whole group closed after the answer")
    assert app.title().endswith(" one")
    app.snap("every tab of the group closed")


def test_closing_one_tab_of_a_group_asks_and_keeps_the_rest(app_factory):
    """closing a tab of a group with a program in it asks first, and the group stays with its
    remaining tab"""
    app = group_app(app_factory, confirm_quit="never")
    name_tabs(app, ["one", "two"])
    group_of(app, 1, [2])
    app.wait(lambda: underlines(app) and span(underlines(app)[0]) > 350, msg="grouped")
    app.key("alt+2")
    app.run("sleep 1000")
    app.wait_title(lambda title: " sleep " in title)
    app.key("ctrl+shift+w")
    app.wait(lambda: dialog(app), msg="the close dialog")
    app.snap("asked before closing a tab of the group")
    app.key("Return")
    app.wait(lambda: len(edges(app)) == 1, msg="the busy tab closed")
    assert chips(app), "the group stayed with its remaining tab"
    assert span(underlines(app)[0]) <= 205, underlines(app)
    assert app.title().endswith(" one")
    app.snap("the group left with one tab")


def test_closing_the_group_of_the_only_tab_closes_the_window(app_factory):
    """close group with a single grouped tab in the window closes it, like closing the last tab"""
    app = group_app(app_factory, confirm_quit="never")
    title_tab(app, "one")
    app.key("ctrl+shift+g")
    app.wait(lambda: chips(app), msg="chip")
    app.palette("close group")
    app.wait(lambda: not app.alive(), msg="the app quit with its last tab")


def test_pinned_tab_inside_a_group_keeps_the_group(app_factory):
    """pinning a tab of a group moves it to the front of the bar, drawn narrow, still under its
    chip and its colored line"""
    app = group_app(app_factory)
    two_grouped(app)
    chip = chips(app)[0]
    app.palette("pin or unpin")
    app.wait_title(lambda title: title.startswith("1 "))
    app.wait_still("the bar settled on the pinned tab")
    assert app.title().endswith(" two"), app.title()
    box = chips(app)[0]
    assert box[0] == chip[0], "the group's first tab is still the one behind the chip"
    assert chip_colour(app, box) == 1
    # a pinned tab is drawn as narrow as its icon
    assert edges(app)[0] - box[2] <= 40, (edges(app), box)
    app.wait(lambda: underlines(app) and span(underlines(app)[0]) > 210,
             msg="the pinned tab and its neighbour share one line")
    app.snap("the pinned tab of the group in front, narrow, still under the chip")
    app.key("alt+1")
    app.wait_title(lambda title: title.endswith(" two"))


def test_renaming_a_tab_inside_a_group_keeps_the_group(app_factory):
    """a double click renames a tab of a group; the chip and the line under the tab stay and the
    tab shows the typed title in place of the one from the shell"""
    app = group_app(app_factory)
    listening(app)
    two_grouped(app)
    app.key("alt+1")
    app.wait_title(lambda title: title.endswith(" one"))
    box = chips(app)[0]
    tab = edges(app)[0]
    app.wait_still("the bar settled")
    # the field opens with the old title selected and the close button sits at the right end of
    # the tab, so only the ink before it counts as the title
    before = ink_width(app, box[2] + 4, tab - 30)
    app.click((box[2] + 4 + tab) // 2, app.bar_height() // 2, repeat=2)
    app.wait(lambda: near(app.shot()[: app.bar_height()], C["selection"], 30).sum() > 500,
             msg="the old title selected in the field")
    app.snap("renaming the first tab of the group")
    app.type("a much longer title")
    app.key("Return")
    app.wait(lambda: any(tab_["title"] == "a much longer title" for tab_ in tab_list(app)),
             msg="the typed title on the tab")
    app.wait(lambda: ink_width(app, box[2] + 4, tab - 30) > before + 40,
             msg="the tab itself shows the longer title")
    assert chip_colour(app, chips(app)[0]) == 1
    assert span(underlines(app)[0]) > 350, "both tabs are still in the group"
    app.snap("the renamed tab of the group")


def test_tabs_of_a_group_stay_together_when_dragged(app_factory):
    """dragging a tab of a group out of its run puts the group back together where its first tab
    was, so the group never splits in the tab bar"""
    app = group_app(app_factory)
    name_tabs(app, ["one", "two", "three"])
    group_of(app, 2, [3])
    app.wait(lambda: underlines(app) and span(underlines(app)[0]) > 350, msg="two grouped")
    app.snap("before the drag: the second and third tab in one group")
    h = app.bar_height()
    # drag the third tab, in the group, onto the first, ungrouped one
    app.drag((550, h // 2), (100, h // 2))
    app.wait_title(lambda title: title.endswith(" three"))
    app.snap("the dragged tab went back into its group at the front")
    assert chips(app) and chips(app)[0][0] <= 6
    app.wait(lambda: underlines(app) and span(underlines(app)[0]) > 350, msg="the group stayed together")
    app.wait(lambda: len(edges(app)) == 3, msg="three tabs")
    # the group's tabs are the first two now, the ungrouped tab is last
    app.key("alt+3")
    app.wait_title(lambda title: title.endswith(" one"))


def test_group_tabs_move_between_panes_together(app_factory):
    """a tab of a group split into another pane takes its chip with it, both panes chip the group
    and dragging the rest of the group over puts all of it in the right pane"""
    app = group_app(app_factory)
    listening(app)
    name_tabs(app, ["one", "two", "three"])
    group_of(app, 2, [3])
    app.wait(lambda: underlines(app) and span(underlines(app)[0]) > 350, msg="two grouped")
    app.key("alt+3")
    app.palette("split right")
    w, _ = app.size()
    # each pane draws the chip of the group where its tabs are, with the line under its tab
    both = app.wait(lambda: len(chips(app)) == 2 and chips(app), msg="a chip per pane")
    assert both[0][2] < w // 2 < both[1][0], both
    assert chip_colour(app, both[0]) == chip_colour(app, both[1]) == 1
    app.wait(lambda: len(underlines(app)) == 2, msg="the line of the group in both panes")
    app.snap("the group's tab split into a right pane, both panes chipped")
    # drag the remaining tab of the group over into the right pane
    h = app.bar_height()
    app.drag((both[0][2] + 4 + 100, h // 2), (w * 3 // 4, h // 2))
    app.wait(lambda: len(chips(app)) == 1 and chips(app)[0][0] > w // 2,
             msg="only the right pane keeps the chip")
    # both tabs of the group are drawn narrow next to the chip, one line under both
    app.wait(lambda: underlines(app) and span(underlines(app)[0]) > 250,
             msg="both tabs of the group in the right pane")
    assert abs(underlines(app)[0][0] - (chips(app)[0][2] + 4)) <= 2, (underlines(app), chips(app))
    app.snap("the group moved over into the right pane")


@pytest.mark.x11_only
def test_a_group_tab_moved_out_of_the_window_leaves_the_group_whole(app_factory):
    """an ungrouped tab moved into a window of its own leaves the group complete in the first
    window, chip and line included. x11 only: with two windows of one process sway's screenshot
    tracks whichever window it finds first, so the first window can't be looked at"""
    app = group_app(app_factory)
    listening(app)
    name_tabs(app, ["one", "two", "three"])
    group_of(app, 2, [3])
    app.wait(lambda: underlines(app) and span(underlines(app)[0]) > 350, msg="two grouped")
    app.key("alt+3")
    app.palette("split right")
    w, _ = app.size()
    both = app.wait(lambda: len(chips(app)) == 2 and chips(app), msg="a chip per pane")
    h = app.bar_height()
    app.drag((both[0][2] + 4 + 100, h // 2), (w * 3 // 4, h // 2))
    app.wait(lambda: len(chips(app)) == 1 and chips(app)[0][0] > w // 2,
             msg="the group in the right pane")
    # the ungrouped tab, alone in the left pane, moves into a window of its own
    app.click(200, 400)
    app.key("alt+1")
    app.wait_title(lambda title: title.endswith(" one"))
    app.palette("move to new window")
    app.wait(lambda: len({tab["window"] for tab in tab_list(app)}) == 2, msg="a second window")
    app.wait(lambda: len(edges(app)) == 2, msg="the first window kept the group")
    assert chips(app) and chip_colour(app, chips(app)[0]) == 1
    app.wait(lambda: underlines(app) and span(underlines(app)[0]) > 350, msg="the group stayed whole")
    app.snap("the moved tab in its own window, the group whole in the first one")


def test_group_and_incognito_tab_with_session_restore(app_factory):
    """restore_session: a renamed hand made group comes back with its tabs and its color, and an
    incognito tab of the group is not restored"""
    app = group_app(app_factory, restore_session="always", confirm_quit="never",
                    window_title=["number", {"text": " "}, "folder"])
    app.wait_title("1 ~")
    # one more tab, moved into a folder of its own
    app.key("ctrl+shift+t")
    app.run("cd $T2_TMP; mkdir -p alpha; cd alpha")
    app.wait_title(f"{len(edges(app))} alpha")
    app.snap("two tabs, the second in a folder of its own")
    group_of(app, 1, [2])
    app.wait(lambda: underlines(app) and span(underlines(app)[0]) > 380, msg="two tabs grouped")
    # an incognito tab joins the group of the tab before it
    app.palette("new incognito tab")
    app.wait(lambda: len(edges(app)) == 3, msg="three tabs")
    app.key("ctrl+shift+d")
    app.wait(lambda: span(underlines(app)[0]) > 580, msg="three tabs in one line of the group")
    app.snap("two tabs and an incognito one in one group")
    app.click(*chip_menu(app, chips(app)[0])[0])
    app.type("the group to keep")
    app.key("Return")
    box = app.wait(lambda: chips(app) and chips(app)[0] if chip_label(app, chips(app)[0]) > 100 else None,
                   msg="the renamed group chip")
    colour = chip_colour(app, box)
    app.palette("quit")
    app.wait(lambda: not app.alive(), msg="app quit")
    app.start()
    back = app.wait(lambda: chips(app) and chips(app)[0], msg="the group came back")
    assert chip_colour(app, back) == colour
    assert abs(chip_width(back) - chip_width(box)) <= 3, (back, box)
    assert len(edges(app)) == 2, "the incognito tab is not restored"
    assert span(underlines(app)[0]) > 380, "the group kept its two tabs"
    app.snap("the group restored with its name, without the incognito tab")


def test_project_groups_come_back_with_the_session(app_factory):
    """with by_project and restore_session, the restored shell starts in its project folder and
    its project group is made again"""
    app = group_app(app_factory, tab_groups={"by_project": True}, restore_session="always",
                    confirm_quit="never", window_title=["number", {"text": " "}, "folder"])
    repo(app, "restored-project")
    app.wait_title("1 ~")
    app.run("cd $T2_TMP/restored-project")
    app.wait_title("1 restored-project")
    box = app.wait(lambda: chips(app) and chips(app)[0], msg="the project chip")
    app.snap("the shell moved into the project, chipped")
    app.palette("quit")
    app.wait(lambda: not app.alive(), msg="app quit")
    app.start()
    app.wait_title("1 restored-project")
    back = app.wait(lambda: chips(app) and chips(app)[0], msg="the project group made again")
    assert chip_colour(app, back) == chip_colour(app, box)
    assert span(underlines(app)[0]) <= 205
    app.snap("the shell restored in its project, the project group back")


def test_expanded_tabs_share_the_bar_with_a_group_chip(app_factory):
    """expand_tabs splits the width next to the chip between the tabs; the chip keeps its size
    and its color"""
    app = group_app(app_factory, expand_tabs=True, tab_width=50)
    name_tabs(app, ["one", "two", "three"])
    group_of(app, 1, [2])
    app.wait(lambda: len(edges(app)) == 3, msg="three tabs")
    app.wait_still("the bar settled")
    box = chips(app)[0]
    # the tabs share what is left of the row next to the chip, all equally wide
    widths = np.diff([box[2] + 4 - 1, *edges(app)])
    assert len(widths) == 3 and widths.max() - widths.min() <= 3, (widths, edges(app), box)
    assert widths.min() > 150, (widths, 900)
    assert chip_colour(app, box) == 1 and chip_label(app, box) > 20
    app.snap("expanded tabs sharing the bar with a group chip")


def test_group_with_a_hidden_bar_and_narrow_tabs(app_factory):
    """hide_bar_for_one_tab hides the bar, and with it the chip of a group of two whose second
    tab closes; a new second tab brings the chip back, and a renamed group grows wider than the
    narrow tabs beside it"""
    app = group_app(app_factory, hide_bar_for_one_tab=True, tab_width=120)
    name_tabs(app, ["one", "two"])
    group_of(app, 1, [2])
    app.wait(lambda: chips(app) and underlines(app), msg="the bar of two tabs and its chip")
    assert chip_width(chips(app)[0]) < 120
    app.snap("two narrow tabs in one group, chipped")
    # the second tab closes, the bar of the single tab left goes away with the chip
    h = app.bar_height()
    app.key("ctrl+shift+w")
    app.wait(lambda: not chips(app) and near(app.shot()[:h], C["tab_bar_background"], 4).sum() < 100,
             msg="the bar hidden with its chip")
    app.snap("hide_bar_for_one_tab hides the bar and the chip with it")
    # a second tab brings the bar back, the chip with it, and joining the group keeps it whole
    app.key("ctrl+shift+t")
    box = app.wait(lambda: chips(app) and chips(app)[0], msg="the bar and its chip back")
    assert chip_colour(app, box) == 1
    assert abs(box[0] - 4) <= 2
    app.key("ctrl+shift+d")
    app.wait(lambda: span(underlines(app)[0]) > 220, msg="both tabs back under the group's line")
    # a long name makes the chip wider than the 120px tabs, which move right
    app.click(*chip_menu(app, box)[0])
    app.type("a much longer group name than the tabs")
    app.key("Return")
    box = app.wait(lambda: chips(app) and chips(app)[0] if chip_width(chips(app)[0]) > 250 else None,
                   msg="a chip wider than a tab")
    assert abs(edges(app)[0] - (box[2] + 4 + 119)) <= 2, (edges(app), box)
    assert chip_colour(app, box) == 1
    app.snap("a chip wider than the tabs beside it")


def test_sleeping_tab_of_a_group_is_woken_from_the_overview(app_factory):
    """a tab hidden behind a collapsed group sleeps after sleep_tabs_after_minutes; the tab
    overview lists it and picking it there switches to it and wakes it with its output"""
    app = group_app(app_factory, sleep_tabs_after_minutes=0.02)
    listening(app)
    two_grouped(app)
    app.key("alt+1")
    # enough output to scroll, so the tab has a history to pack away
    app.run("seq 1 200; echo marker_one")
    first = str(next(tab["id"] for tab in tab_list(app) if tab["number"] == 1))
    wait_text(app, "marker_one", first)
    app.key("alt+2")
    app.wait_title(lambda title: title.endswith(" two"))
    app.wait(lambda: slept(app, 1), msg="the hidden tab asleep")
    # collapse the group: the sleeping tab is only reachable through the chip now
    box = chips(app)[0]
    app.click((box[0] + box[2]) // 2, app.bar_height() // 2)
    app.wait(lambda: len(edges(app)) == 1, msg="the sleeping tab behind the chip")
    app.snap("the sleeping tab hidden behind the collapsed group")
    # the overview lists it anyway, and picking it switches over and wakes it
    app.key("ctrl+shift+o")
    app.wait(lambda: overview_open(app) > 1500, msg="the overview with its picked tab")
    app.snap("the overview of both tabs of the group")
    app.click(120, 150)
    app.wait_title(lambda title: title.endswith(" one"))
    app.wait(lambda: not slept(app, 1), msg="the picked tab woke up")
    assert "marker_one" in pane_text(app, first)
    app.snap("the sleeping tab picked, awake and on screen")
    assert chip_colour(app, chips(app)[0]) == 1

