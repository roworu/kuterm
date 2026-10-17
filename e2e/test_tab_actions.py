"""what one tab can do: its right click menu (rename, pin, duplicate, copy folder, split, move
to a new window, groups and the close family), the three rename paths, pinning by hand, and
moving a tab with its running programs into a window of its own"""

import time

import numpy as np
import pytest

from harness import (BACKEND, App, as_rgb, close_to, count, get_clipboard, ink, mask_bbox, near,
                     screen, session_env, sh, swaymsg, unique_theme, xdo)
from kit import integrated, msg, ready, send, tabs, text

FEATURE = "tab actions"

THEME = unique_theme(0x4C)
C = as_rgb(THEME)
# every tab is this wide, so a click lands on a known one
WIDTH = 200
# the bar shows with one tab too, the window title counts the active tab, dividers stay visible
BASE = {"theme": {"mode": "dark"}, "window_title": ["number"], "tab_width": WIDTH,
        "tab_title": [{"text": "title"}], "panes": {"divider_width": 4}}
# entries of the tab menu in the order it lists them, the separators left out
RENAME, PIN, DUPLICATE, COPY_FOLDER, MOVE_WINDOW, GROUP = range(6)
SPLIT_RIGHT, SPLIT_DOWN = 6, 7
CLOSE, CLOSE_OTHERS, CLOSE_RIGHT, CLOSE_LEFT, CLOSE_GROUP = range(8, 13)


def tab_app(app_factory, *, keybindings: dict | None = None, **extra) -> App:
    """app with an integrated bash, remote control, the unique theme and fixed width tabs"""
    app = app_factory(integrated(**{**BASE, **extra}), themes={"dark": THEME}, keybindings=keybindings)
    ready(app)
    # the pointer away from the bar, hover changes its colors
    app.mouse(450, 300)
    return app


def windows_of(app: App) -> dict:
    """the tabs of every window, window id -> its tabs left to right"""
    out: dict = {}
    for tab in tabs(app):
        out.setdefault(tab["window"], []).append(tab)
    return {window: sorted(ts, key=lambda tab: (tab["pane"], tab["number"]))
            for window, ts in out.items()}


def window_tabs(app: App, window: int | None = None) -> list:
    """the tabs of one window left to right, without one of the window opened first"""
    by_window = windows_of(app)
    return by_window.get(min(by_window) if window is None else window, [])


def titles_of(app: App, window: int | None = None) -> list:
    """the title every tab shows, left to right"""
    return [tab["title"] for tab in window_tabs(app, window)]


def ids_of(app: App, window: int | None = None) -> list:
    """the id of every tab, left to right"""
    return [str(tab["id"]) for tab in window_tabs(app, window)]


def one_window(app: App) -> int:
    """id of the only window the app has"""
    by_window = windows_of(app)
    assert len(by_window) == 1, sorted(by_window)
    return next(iter(by_window))


def new_tabs(app: App, n: int) -> None:
    """ctrl+shift+t until there are n tabs, the window title is the active tab number
    (a title naming the running command after it, when the settings ask for one)"""
    for number in range(2, n + 1):
        app.key("ctrl+shift+t")
        app.wait_title(lambda title, number=number: title in (str(number), f"{number} bash"))


def tab_edges(app: App, img=None) -> list:
    """x of every tab border in the middle row of the bar, the right edge of each tab"""
    img = app.shot() if img is None else img
    row = near(img[app.bar_height() // 2:app.bar_height() // 2 + 1], C["border"])[0]
    return [int(x) for x in np.flatnonzero(row)]


def tab_center(app: App, ix: int, img=None) -> int:
    """x of the middle of the ix-th tab, whatever width the tabs have"""
    edges = [0, *tab_edges(app, img)]
    assert ix + 1 < len(edges), (ix, edges)
    return (edges[ix] + edges[ix + 1]) // 2


def menu_open(app: App, x: int) -> bool:
    """whether the menu of the tab right clicked at x shows right below the bar"""
    img = app.shot()
    h = app.bar_height()
    return int(near(img[h + 3:h + 18, x:x + 200], C["tab_bar_background"]).sum()) > 500


def open_menu(app: App, tab: int = 0, at: int | None = None) -> int:
    """right click a tab near the bar's bottom edge, waits for the menu, returns the x used"""
    x = tab_center(app, tab) if at is None else at
    app.click(x, app.bar_height() - 4, button=3)
    app.wait(lambda: menu_open(app, x), msg="the tab menu")
    return x


def menu_state(app: App, x: int) -> tuple:
    """(box, bands) of the menu opened at x: its rectangle, and one (top, bottom, left, right)
    per band of ink in it, the 1 pixel separators included"""
    img = app.shot()
    h = app.bar_height()
    # stops short of the bottom of the window, so notifications never land in here
    region = img[h:h + 440, x:x + 400]
    box = mask_bbox(near(region, C["tab_bar_background"]))
    assert box is not None, f"no tab menu below x {x}"
    x0, y0, x1, y1 = box
    # the row separators are 1px lines of `border` against the menu background, a low
    # threshold, or the faintest of them is left out of the rows
    inside = ink(region[y0:y1, x0:x1], C["tab_bar_background"], 30)
    bands, start = [], None
    for row, has_ink in enumerate([*inside.any(axis=1), False]):
        if has_ink and start is None:
            start = row
        elif not has_ink and start is not None:
            columns = np.flatnonzero(inside[start:row].any(axis=0))
            bands.append((h + y0 + start, h + y0 + row, x + x0 + int(columns[0]),
                          x + x0 + int(columns[-1]) + 1))
            start = None
    return (x + x0, h + y0, x + x1, h + y1), bands


def menu_rows(app: App, x: int) -> tuple:
    """(entries, separators) of the menu opened at x: the labeled rows and the 1px lines
    between them, both as (top, bottom, left, right). a stray dot of ink, or a row the
    screenshot caught half drawn, is neither: entries carry a label, separators the width"""
    _, bands = menu_state(app, x)
    entries = [band for band in bands if band[1] - band[0] > 2 and band[3] - band[2] > 20]
    separators = [band for band in bands if band[1] - band[0] <= 2 and band[3] - band[2] > 100]
    return entries, separators


def menu_entries(app: App, x: int) -> list:
    """(top, bottom, left, right) of every labeled entry of the menu, separators left out"""
    return menu_rows(app, x)[0]


def menu_click(app: App, x: int, entry: int, *, closes: bool = True) -> None:
    """click entry `entry` of the menu opened at x, waiting for the menu to close"""
    (x0, _, x1, _), _ = menu_state(app, x)
    top, bottom, _, _ = menu_entries(app, x)[entry]
    app.click((x0 + x1) // 2, (top + bottom) // 2)
    if closes:
        app.wait(lambda: not menu_open(app, x), msg=f"menu entry {entry} closed the menu")


def renaming(app: App) -> bool:
    """whether a rename field shows in the bar: its title selected for typing"""
    return bool(near(app.shot()[:app.bar_height()], C["selection"], 20).any())


def bar_shown(app: App) -> bool:
    """whether the tab bar is drawn, its bottom border sits above the grid"""
    return close_to(tuple(app.shot()[app.bar_height() - 1, 450]), C["border"])


def bar_region(app: App, x0: int, x1: int) -> np.ndarray:
    """the tab bar between x0 and x1, its border rows left out"""
    return app.shot()[2:app.bar_height() - 2, x0:x1]


def inked(app: App, x0: int, x1: int, img=None) -> np.ndarray:
    """mask of the text drawn in the bar between x0 and x1"""
    img = app.shot() if img is None else img
    return ink(img[2:app.bar_height() - 2, x0:x1], C["tab_bar_background"], 60)


def dialog(app: App) -> tuple | None:
    """bbox of the danger colored "close" button, none while no confirmation is up"""
    mask = near(app.shot(), C["danger_button"])
    return mask_bbox(mask) if mask.sum() > 200 else None


def alike(a: np.ndarray, b: np.ndarray) -> bool:
    """two inked texts match: every pixel of one is within a pixel of a pixel of the other"""
    def grown(mask: np.ndarray) -> np.ndarray:
        out = mask.copy()
        for dy in (-1, 0, 1):
            for dx in (-1, 0, 1):
                out |= np.roll(np.roll(mask, dy, axis=0), dx, axis=1)
        return out
    return not (a & ~grown(b)).any() and not (b & ~grown(a)).any()


def loop(app: App, pane: str) -> None:
    """a bash loop in that tab writing a line every 200ms, so a moved tab shows it never stopped"""
    send(app, 'while true; do echo tick >> "$T2_TMP/ticks"; sleep 0.2; done', pane)


def ticks(app: App) -> int:
    """lines the loop wrote so far"""
    path = app.file("ticks")
    return len(path.read_text().splitlines()) if path.exists() else 0


def other_window(app: App) -> str | None:
    """id of an app window that is not the one the harness drives"""
    if BACKEND == "wayland":
        found, stack = [], [swaymsg("-t", "get_tree")]
        while stack:
            node = stack.pop()
            if node.get("pid") == app.proc.pid and node.get("visible") and str(node["id"]) != app.wid:
                found.append(str(node["id"]))
            stack.extend(node.get("nodes", []) + node.get("floating_nodes", []))
        return found[0] if found else None
    found = [wid for wid in xdo("search", "--onlyvisible", "--pid", str(app.proc.pid), check=False).split()
             if wid != app.wid]
    return found[0] if found else None


def window_shot(app: App, wid: str) -> np.ndarray | None:
    """screenshot of another window of the app, x11 only"""
    if BACKEND == "wayland":
        return None
    info = sh("xwininfo", "-id", wid, env=session_env())
    val = dict(line.strip().split(":", 1) for line in info.splitlines() if ":" in line)
    return screen(int(val["Absolute upper-left X"]), int(val["Absolute upper-left Y"]),
                  int(val["Width"]), int(val["Height"]))


def test_menu_lists_the_entries_of_a_tab(app_factory):
    """a right click on a tab opens its menu at the pointer: every action in the place it
    belongs, a separator before the splits and one before the close family"""
    app = tab_app(app_factory)
    new_tabs(app, 2)
    x = open_menu(app, 1)
    box, _ = menu_state(app, x)
    entries, separators = menu_rows(app, x)
    app.snap("the tab menu: rename, pin, duplicate, copy folder, move, group, splits, closes")
    assert len(entries) == 12, entries
    # a separator after the sixth entry (the group one) and one after the two splits
    rows = sorted(entries + separators)
    assert [rows.index(separator) for separator in separators] == [6, 9], rows
    # it opens at the tab that was right clicked: its box starts at the click and hangs
    # from the bar's bottom edge on down
    assert abs(box[0] - x) <= 2 and abs(box[1] - app.bar_height()) <= 6, (box, x)
    # every entry starts at the same left padding, "rename" is wider than the "pin" below it
    lefts = [band[2] for band in entries]
    assert max(lefts) - min(lefts) <= 2, entries
    assert entries[0][3] - entries[0][2] > entries[1][3] - entries[1][2], entries[:2]


@pytest.mark.parametrize("settings,at,ix", [({}, 500, 2), ({"expand_tabs": True}, 337, 1)])
def test_the_menu_opens_at_the_clicked_tab(app_factory, settings, at, ix):
    """fixed width and expanded tabs alike: the menu shows at the tab it was opened on and its
    entries act on that tab, not on the active one"""
    app = tab_app(app_factory, **settings)
    new_tabs(app, 4)
    ids = ids_of(app)
    assert app.title() == "4"
    x = open_menu(app, at=at)
    box, _ = menu_state(app, x)
    assert abs(box[0] - at) <= 2 and box[2] <= 900, (box, at)
    menu_click(app, x, CLOSE)
    app.wait(lambda: len(ids_of(app)) == 3, msg="one tab less")
    assert ids_of(app) == ids[:ix] + ids[ix + 1:], (ids_of(app), ids)


PATHS = {
    "double click": lambda app: app.click(tab_center(app, 0), app.bar_height() // 2, repeat=2),
    "palette": lambda app: app.palette("tabs: rename"),
    "menu": lambda app: menu_click(app, open_menu(app, 0), RENAME),
}


@pytest.mark.parametrize("path", list(PATHS), ids=list(PATHS))
def test_every_rename_path_asks_for_a_title(app_factory, path):
    """a double click, the palette command and the menu entry all show a field over the tab
    with the old title selected, so typing replaces it"""
    app = tab_app(app_factory, tab_title=[{"text": "old"}])
    PATHS[path](app)
    app.wait(lambda: renaming(app), msg="the title selected for typing")
    app.snap(f"{path}: the old title selected in the tab")
    app.type("new name")
    app.key("Return")
    app.wait(lambda: titles_of(app) == ["new name"], msg="the typed title shown")
    assert not renaming(app), "the field stayed open"
    app.snap(f"{path}: the tab renamed")


def test_rename_to_nothing_shows_the_title_blocks_again(app_factory):
    """clearing the title of a renamed tab puts the `tab_title` blocks back on it"""
    app = tab_app(app_factory, tab_title=[{"text": "blocks"}])
    app.palette("tabs: rename")
    app.wait(lambda: renaming(app), msg="the rename field")
    app.type("mine")
    app.key("Return")
    app.wait(lambda: titles_of(app) == ["mine"], msg="renamed")
    app.palette("tabs: rename")
    app.wait(lambda: renaming(app), msg="the rename field again")
    # the whole title is selected, so backspace clears it
    app.key("BackSpace")
    app.key("Return")
    app.wait(lambda: titles_of(app) == ["blocks"], msg="the blocks are back")
    app.snap("the title cleared, the tab shows its `tab_title` blocks again")


def test_escape_cancels_a_rename(app_factory):
    """escape leaves the field and keeps the title the tab had"""
    app = tab_app(app_factory, tab_title=[{"text": "keep"}])
    # the pointer stays on the tab, so the hovered look is the same before and after
    app.click(tab_center(app, 0), app.bar_height() // 2)
    app.wait_title("1")
    app.wait_still()
    before = bar_region(app, 0, WIDTH)
    app.click(tab_center(app, 0), app.bar_height() // 2, repeat=2)
    app.wait(lambda: renaming(app), msg="the rename field")
    app.type("discarded")
    app.key("Escape")
    app.wait(lambda: not renaming(app), msg="the field closed")
    app.wait(lambda: np.array_equal(bar_region(app, 0, WIDTH), before), msg="the tab as before")
    assert titles_of(app) == ["keep"], titles_of(app)
    app.snap("escape left the title alone")


def test_a_rename_names_the_tab_pane_lookups_find(app_factory):
    """a title typed into a tab is what `--pane` finds it by, but a `rename_pane` name of
    another tab wins: names are looked up before titles"""
    app = tab_app(app_factory)
    new_tabs(app, 2)
    menu_click(app, open_menu(app, 0), RENAME)
    app.wait(lambda: renaming(app), msg="the rename field")
    app.type("alpha")
    app.key("Return")
    app.wait(lambda: titles_of(app) == ["alpha", "title"], msg="the first tab titled")
    app.run("echo FIRST_TAB")
    app.wait(lambda: "FIRST_TAB" in text(app), msg="the first tab printed")
    app.snap("a tab renamed to `alpha`, found by the --pane alpha that is its title")
    assert "FIRST_TAB" in text(app, "alpha")
    # the second tab takes the same string as its name for remote control. the name field
    # opens empty (nothing to select), so the name shows up in list-tabs, not in the bar
    app.click(tab_center(app, 1), app.bar_height() // 2)
    app.wait_title("2")
    app.palette("tabs: name for remote control")
    app.type("alpha")
    app.key("Return")
    app.wait(lambda: [tab["name"] for tab in window_tabs(app)] == [None, "alpha"], msg="name set")
    app.run("echo SECOND_TAB")
    app.wait(lambda: "SECOND_TAB" in text(app), msg="the second tab printed")
    app.snap("two tabs, the first titled `alpha`, the second named `alpha`")
    # the name wins over the title of the other tab
    assert "SECOND_TAB" in text(app, "alpha")
    assert titles_of(app) == ["alpha", "title"], titles_of(app)


@pytest.mark.parametrize("title", ["a long title with many words in it", "日本語のタブ名前です長い"],
                         ids=["words", "cjk"])
def test_a_long_renamed_title_is_cut_like_a_title_of_blocks(app_factory, title):
    """a long or unicode title of its own is drawn exactly like the same text in `tab_title`:
    cut at its end with an ellipsis where the next glyph would stand"""
    app = tab_app(app_factory, show_tab_close_button=False, tab_title=[{"text": title}])
    new_tabs(app, 3)
    app.click(tab_center(app, 1), app.bar_height() // 2)
    app.wait_title("2")
    if title.isascii():
        app.palette("tabs: rename")
        app.wait(lambda: renaming(app), msg="the rename field")
        app.type(title)
        app.key("Return")
    else:
        # no key can type this one: xdotool mangles it, so the title comes in over the socket
        done = msg(app, "set-tab-title", "--pane", ids_of(app)[1], title)
        assert done.returncode == 0, done.stderr
    app.wait(lambda: titles_of(app) == [title] * 3, msg="the tabs titled the same")
    # the third tab is active, so the two long titles below are drawn the same way
    app.click(tab_center(app, 2), app.bar_height() // 2)
    app.wait_title("3")
    app.mouse(450, 300)
    app.wait_still()
    app.snap("the same long title, one from the blocks and one typed in, cut the same way")
    blocks = inked(app, 0, WIDTH)
    renamed = inked(app, WIDTH, 2 * WIDTH)
    assert blocks.any() and renamed.any(), "a title is missing"
    # the title runs up to the tab's padding: it is cut, not short
    assert int(np.flatnonzero(renamed.any(axis=0))[-1]) > WIDTH - 40, "the title is not cut"
    assert alike(blocks, renamed), "the typed title is drawn unlike the one from the blocks"


def test_the_menu_closes_clicking_outside_without_acting(app_factory):
    """a click outside the menu closes it and does nothing to the tabs"""
    app = tab_app(app_factory)
    new_tabs(app, 3)
    ids = ids_of(app)
    for somewhere in [(450, 450), (100, app.bar_height() // 2)]:
        x = open_menu(app, 2)
        app.click(*somewhere)
        app.wait(lambda: not menu_open(app, x), msg=f"the menu closed by a click at {somewhere}")
        assert ids_of(app) == ids, (ids_of(app), ids)
    app.snap("the tabs as they were after the menu was closed by clicks outside")


def test_escape_closes_the_menu_without_acting(app_factory):
    """escape takes the menu away and leaves the tabs alone, like a click outside does"""
    app = tab_app(app_factory)
    new_tabs(app, 3)
    ids = ids_of(app)
    x = open_menu(app, 2)
    app.key("Escape")
    app.wait(lambda: not menu_open(app, x), msg="the menu closed by escape")
    assert ids_of(app) == ids, ids_of(app)
    app.snap("escape closed the menu, the tabs untouched")


def test_typing_with_the_menu_open_does_not_reach_the_tab(app_factory):
    """keys typed while a menu covers a tab go to the menu, not to the shell below it"""
    app = tab_app(app_factory)
    new_tabs(app, 2)
    x = open_menu(app, 1)
    app.type("echo typed > $T2_TMP/typed")
    app.key("Return")
    time.sleep(0.5)
    assert not app.file("typed").exists(), "the typed line ran in the tab below the menu"
    app.click(450, 450)
    app.wait(lambda: not menu_open(app, x), msg="the menu closed")
    assert not app.file("typed").exists(), "the typed line ran in the tab below the menu"
    app.snap("nothing was typed into the tab, the menu covered it")


def test_copy_folder_path_entry_puts_the_folder_on_the_clipboard(app_factory):
    """the copy folder entry puts the folder the tab's shell is in on the clipboard"""
    app = tab_app(app_factory)
    app.run("cd /etc")
    app.wait(lambda: window_tabs(app)[0]["cwd"] == "/etc", msg="the shell in /etc")
    menu_click(app, open_menu(app, 0), COPY_FOLDER)
    app.wait(lambda: get_clipboard() == "/etc", msg="the folder on the clipboard")
    app.snap("the tab's folder copied, told by a notification")


@pytest.mark.parametrize("entry,sideways", [(SPLIT_RIGHT, True), (SPLIT_DOWN, False)])
def test_split_entries_move_the_tab_to_a_new_pane(app_factory, entry, sideways):
    """the split entries of the menu split the pane of the clicked tab and put that tab in the
    new pane, on the side the entry names"""
    app = tab_app(app_factory)
    new_tabs(app, 2)
    before = window_tabs(app)
    x = open_menu(app, 1)
    menu_click(app, x, entry)
    app.wait(lambda: len({tab["pane"] for tab in window_tabs(app)}) == 2, msg="a second pane")
    moved = next(tab for tab in window_tabs(app) if str(tab["id"]) == str(before[1]["id"]))
    left = next(tab for tab in window_tabs(app) if str(tab["id"]) == str(before[0]["id"]))
    app.snap(f"the tab split off into a pane of its own, {'to the right' if sideways else 'below'}")
    assert moved["pane"] != left["pane"], (moved, left)
    assert moved["number"] == 1, moved
    # the divider between the panes runs along the side the entry names
    grid = near(app.shot()[app.bar_height() + 10:-10, 10:-10], C["border"])
    assert (int(grid.sum(axis=0).max()) > 400) == sideways, int(grid.sum(axis=0).max())


def test_close_other_tabs_entry_keeps_the_active_and_the_pinned(app_factory):
    """close other tabs closes the unpinned ones, asking once when a program still runs in
    one of them, and leaves the pinned tab alone"""
    app = tab_app(app_factory, window_title=["number", {"text": " "}, "command"])
    new_tabs(app, 3)
    # the third tab is pinned and drawn in front, the second one runs a program
    app.click(tab_center(app, 2), app.bar_height() // 2)
    app.wait_title("3 bash")
    menu_click(app, open_menu(app, 2), PIN)
    app.wait(lambda: tab_edges(app)[0] < 60, msg="the pinned tab in front")
    pinned = ids_of(app)[0]
    app.click(tab_center(app, 1), app.bar_height() // 2)
    app.wait_title("2 bash")
    app.run("sleep 300")
    app.wait_title("2 sleep")
    x = open_menu(app, 2)
    menu_click(app, x, CLOSE_OTHERS)
    app.wait(lambda: dialog(app), msg="the confirmation for the running program")
    app.snap("close other tabs asks about the tab running sleep")
    app.key("y")
    app.wait(lambda: len(ids_of(app)) == 2, msg="the other tabs closed")
    assert ids_of(app)[0] == pinned, (ids_of(app), pinned)
    assert titles_of(app) == ["title", "title"], titles_of(app)
    app.snap("only the pinned tab and the tab asked about are left")


@pytest.mark.parametrize("entry", [CLOSE_RIGHT, CLOSE_LEFT])
def test_close_to_the_side_entries_skip_pinned_tabs(app_factory, entry):
    """close tabs to the left and to the right of the clicked tab close the unpinned ones on
    that side and leave the pinned tab standing"""
    app = tab_app(app_factory)
    new_tabs(app, 4)
    # the third tab is pinned and drawn in front, the others keep their order behind it
    app.click(tab_center(app, 2), app.bar_height() // 2)
    app.wait_title("3")
    menu_click(app, open_menu(app, 2), PIN)
    app.wait(lambda: tab_edges(app)[0] < 60, msg="the pinned tab")
    # the second tab from the left is the first one that is not pinned
    app.click(tab_center(app, 1), app.bar_height() // 2)
    app.wait_title("2")
    menu_click(app, open_menu(app, 1), entry)
    if entry == CLOSE_LEFT:
        # the pinned tab is the only one on that side, so nothing closes at all
        app.wait_still()
        assert len(ids_of(app)) == 4, titles_of(app)
        app.snap("close tabs to the left left the pinned tab standing")
        return
    app.wait(lambda: len(ids_of(app)) == 2, msg="the tabs on the right closed")
    app.wait_still()
    assert tab_edges(app)[0] < 60, tab_edges(app)
    assert tab_edges(app)[1] > 150, tab_edges(app)
    app.snap("tabs to the right closed, the pinned tab and the clicked one are left")


def test_close_entry_on_the_last_tab_takes_the_window_with_it(app_factory):
    """the close entries are listed for the last tab too: the ones needing a neighbour do
    nothing, closing the only tab of a window ends the app"""
    app = tab_app(app_factory)
    x = open_menu(app, 0)
    assert len(menu_entries(app, x)) == 12, menu_entries(app, x)
    for entry in (CLOSE_OTHERS, CLOSE_RIGHT, CLOSE_LEFT):
        menu_click(app, x, entry)
        app.wait_still()
        assert len(ids_of(app)) == 1, f"menu entry {entry} closed the only tab"
        x = open_menu(app, 0)
    app.snap("the menu of the only tab, its neighbours' entries doing nothing")
    menu_click(app, x, CLOSE, closes=False)
    app.wait(lambda: not app.alive(), msg="the app quit with its last tab")


def test_pin_entry_makes_the_tab_small_and_first(app_factory):
    """pinning through the menu moves the tab in front of the others, draws it as wide as its
    icon and takes its close button away"""
    app = tab_app(app_factory)
    new_tabs(app, 3)
    ids = ids_of(app)
    assert all(edge > 150 for edge in tab_edges(app)), tab_edges(app)
    # the third tab, made active so its title is the bright one
    app.click(tab_center(app, 2), app.bar_height() // 2)
    app.wait_title("3")
    menu_click(app, open_menu(app, 2), PIN)
    app.wait(lambda: ids_of(app)[0] == ids[2], msg="the pinned tab in front")
    app.wait_still()
    edges = tab_edges(app)
    app.snap("the pinned tab: its icon only, in front of the other two")
    # no title of its own and no close button fit in this width
    assert edges[0] < 55, edges
    # the tabs after it keep the width they had
    assert [edges[1] - edges[0], edges[2] - edges[1]] == [WIDTH, WIDTH], edges


def test_pin_entry_unpins_the_tab_again(app_factory):
    """the entry toggles: a tab pinned with the palette command gets its width back when it
    is unpinned through the menu"""
    app = tab_app(app_factory)
    new_tabs(app, 2)
    # the palette command pins the tab in front, the first one
    app.click(tab_center(app, 0), app.bar_height() // 2)
    app.wait_title("1")
    ids = ids_of(app)
    app.palette("tabs: pin or unpin")
    app.wait(lambda: tab_edges(app)[0] < 55, msg="pinned")
    app.snap("the first tab pinned by the palette command")
    menu_click(app, open_menu(app, 0), PIN)
    app.wait(lambda: tab_edges(app)[0] > 150, msg="wide again")
    app.wait_still()
    assert ids_of(app) == ids, ids_of(app)
    # both tabs are a full tab wide again
    edges = tab_edges(app)
    assert edges[1] - edges[0] == WIDTH and edges[0] > WIDTH - 10, edges
    app.snap("unpinned again, back to the width of the other tab")


def test_middle_click_does_not_close_a_pinned_tab(app_factory):
    """a pinned tab only closes on purpose: the middle click that closes other tabs leaves it
    standing"""
    app = tab_app(app_factory)
    new_tabs(app, 3)
    app.click(tab_center(app, 2), app.bar_height() // 2)
    app.wait_title("3")
    menu_click(app, open_menu(app, 2), PIN)
    app.wait(lambda: tab_edges(app)[0] < 60, msg="pinned")
    pinned = ids_of(app)[0]
    app.click(tab_center(app, 0), app.bar_height() // 2, button=2)
    time.sleep(0.5)
    assert pinned in ids_of(app), "the middle click closed the pinned tab"
    app.snap("the pinned tab still there after a middle click")
    # the very same click closes a plain tab
    app.click(tab_center(app, 1), app.bar_height() // 2, button=2)
    app.wait(lambda: len(ids_of(app)) == 2, msg="the plain tab closed")


def test_a_pinned_tab_still_answers_for_a_running_program(app_factory):
    """pinning hides the close button, not the close: closing a pinned tab with a program
    running asks first, and no answer keeps it"""
    app = tab_app(app_factory, window_title=["number", {"text": " "}, "command"])
    app.run("sleep 300")
    app.wait_title("1 sleep")
    app.palette("tabs: pin or unpin")
    app.wait(lambda: tab_edges(app)[0] < 55, msg="pinned")
    pane = ids_of(app)[0]
    done = msg(app, "close-tab", "--pane", pane)
    assert done.returncode == 0, done.stderr
    app.wait(lambda: dialog(app), msg="the confirmation for the pinned tab")
    app.snap("closing the pinned tab from remote control asks about the program in it")
    app.key("n")
    app.wait(lambda: dialog(app) is None, msg="the dialog closed")
    time.sleep(0.5)
    assert pane in ids_of(app), "the pinned tab closed without an answer"
    msg(app, "close-tab", "--pane", pane)
    app.wait(lambda: dialog(app), msg="the confirmation again")
    app.key("y")
    app.wait(lambda: not app.alive(), msg="the pinned tab closed on y and took the window")


def test_a_pinned_titled_tab_comes_back_pinned(app_factory):
    """with restore_session, a pinned tab with a title of its own comes back the same: in
    front, drawn as wide as its title and not as wide as the other tab"""
    app = tab_app(app_factory, restore_session="always", confirm_quit="never")
    new_tabs(app, 2)
    # the palette command renames and pins the active tab, the second one
    app.palette("tabs: rename")
    app.wait(lambda: renaming(app), msg="the rename field")
    app.type("keeper")
    app.key("Return")
    app.wait(lambda: titles_of(app) == ["title", "keeper"], msg="renamed")
    app.palette("tabs: pin or unpin")
    app.wait(lambda: tab_edges(app)[0] < WIDTH - 40, msg="pinned in front")
    app.wait_still()
    width = tab_edges(app)[0]
    app.snap("the pinned tab with a title of its own before the restart")
    app.palette("quit")
    app.wait(lambda: not app.alive(), msg="app quit")
    app.start()
    ready(app)
    app.mouse(450, 300)
    app.wait(lambda: titles_of(app) == ["keeper", "title"], msg="both tabs restored")
    app.wait_still()
    app.snap("after the restart: the pinned tab back in front, as small as before")
    assert abs(tab_edges(app)[0] - width) <= 4, (tab_edges(app), width)
    assert tab_edges(app)[1] - tab_edges(app)[0] == WIDTH, tab_edges(app)


def test_duplicate_entry_opens_the_same_profile_and_folder_next_to_it(app_factory):
    """duplicating a tab opens one with the same profile and the folder its shell is in, right
    after it, and nothing the original ran shows up in it"""
    bash = {"name": "bash", "command": {"program": "/bin/bash"}, "env": {"PS1": "$ "}}
    profile = {"name": "dup", "command": {"program": "/bin/bash"}, "env": {"DUP_TAB": "yes"},
               "working_directory": "/tmp"}
    app = tab_app(app_factory, profiles=[bash, profile])
    one_window(app)
    msg(app, "new-tab", "--profile", "dup")
    app.wait(lambda: len(window_tabs(app)) == 2, msg="the tab with the profile")
    original = window_tabs(app)[1]
    send(app, "echo MARK=$DUP_TAB", str(original["id"]))
    app.wait(lambda: "MARK=yes" in text(app, str(original["id"])), msg="the profile in the tab")
    send(app, "cd /etc && echo ORIGINAL_ONLY", str(original["id"]))
    app.wait(lambda: window_tabs(app)[1]["cwd"] == "/etc", msg="the shell in /etc")
    app.click(tab_center(app, 1), app.bar_height() // 2)
    app.wait_title("2")
    menu_click(app, open_menu(app, 1), DUPLICATE)
    app.wait(lambda: len(window_tabs(app)) == 3, msg="the copy")
    copy = window_tabs(app)[2]
    send(app, "echo MARK=$DUP_TAB", str(copy["id"]))
    app.wait(lambda: "MARK=yes" in text(app, str(copy["id"])), msg="the same profile in the copy")
    app.snap("the copy next to the original, same profile, same folder")
    assert copy["cwd"] == "/etc", copy
    assert copy["number"] == 3 and original["number"] == 2, (copy, original)
    assert "ORIGINAL_ONLY" not in text(app, str(copy["id"])), "the original's output was copied"


def test_move_to_new_window_entry_keeps_the_programs_running(app_factory):
    """the move entry takes the tab to a window of its own, the same tab, with its shell and
    its running program still going"""
    app = tab_app(app_factory)
    new_tabs(app, 2)
    ids = ids_of(app)
    first = one_window(app)
    loop(app, ids[1])
    app.wait(lambda: ticks(app) > 2, msg="the loop running")
    menu_click(app, open_menu(app, 1), MOVE_WINDOW)
    app.wait(lambda: len(windows_of(app)) == 2, msg="a window of its own")
    by_window = windows_of(app)
    app.snap("the tab moved out, the first window keeping its own")
    assert ids_of(app, first) == [ids[0]], by_window
    elsewhere = [window for window in by_window if window != first]
    assert len(elsewhere) == 1, by_window
    assert ids_of(app, elsewhere[0]) == [ids[1]], by_window
    moved = by_window[elsewhere[0]][0]
    assert moved["title"] == "title", moved
    was = ticks(app)
    app.wait(lambda: ticks(app) > was + 2, msg="the loop still writing in the moved tab")
    assert "tick" in text(app, ids[1]), "the moved tab shows what its shell kept printing"


def test_move_to_new_window_from_a_keybinding(app_factory):
    """the move action bound to a key takes the tab out the same way, without a menu"""
    app = tab_app(app_factory, keybindings={"move_tab_to_new_window": "ctrl-shift-y"})
    new_tabs(app, 2)
    ids = ids_of(app)
    first = one_window(app)
    app.key("ctrl+shift+y")
    app.wait(lambda: len(windows_of(app)) == 2, msg="a window of its own")
    app.snap("the tab moved out by the key")
    by_window = windows_of(app)
    elsewhere = [window for window in by_window if window != first]
    assert len(elsewhere) == 1, by_window
    assert ids_of(app, first) == [ids[0]], by_window
    assert ids_of(app, elsewhere[0]) == [ids[1]], by_window
    assert [tab["number"] for tab in by_window[elsewhere[0]]] == [1], by_window


def test_moving_the_only_tab_of_a_window_leaves_one_window(app_factory):
    """moving the only tab of a window out opens a window for it and closes the emptied one,
    with the shell in the tab still running"""
    app = tab_app(app_factory)
    loop(app, ids_of(app)[0])
    app.wait(lambda: ticks(app) > 2, msg="the loop running")
    before = one_window(app)
    was = ticks(app)
    app.palette("tabs: move to new window")
    app.wait(lambda: [window for window in windows_of(app)] != [before], msg="the window closed")
    assert len(windows_of(app)) == 1 and one_window(app) != before, windows_of(app)
    app.wait(lambda: ticks(app) > was + 2, msg="the loop still writing in the moved tab")
    assert "tick" in text(app), "the moved tab shows what its shell kept printing"


def test_a_titled_and_pinned_tab_moves_with_its_state(app_factory):
    """the moved tab arrives with the title and the pin it had: the new window draws it small
    and in front, without a close button"""
    app = tab_app(app_factory)
    new_tabs(app, 2)
    app.click(tab_center(app, 1), app.bar_height() // 2)
    app.wait(lambda: window_tabs(app)[1]["active"], msg="the second tab in front")
    app.palette("tabs: rename")
    app.wait(lambda: renaming(app), msg="the rename field")
    app.type("traveller")
    app.key("Return")
    app.wait(lambda: titles_of(app) == ["title", "traveller"], msg="renamed")
    app.palette("tabs: pin or unpin")
    app.wait(lambda: tab_edges(app)[0] < WIDTH - 40, msg="pinned in front")
    moved_id = ids_of(app)[0]
    first = one_window(app)
    app.palette("tabs: move to new window")
    app.wait(lambda: len(windows_of(app)) == 2, msg="a window of its own")
    by_window = windows_of(app)
    elsewhere = [window for window in by_window if window != first]
    moved = by_window[elsewhere[0]][0]
    app.snap("the titled tab in the window of its own, still pinned")
    assert str(moved["id"]) == moved_id, (moved, moved_id)
    assert moved["title"] == "traveller", moved
    # the new window draws it as wide as its own title, not as wide as a tab
    wid = other_window(app)
    img = window_shot(app, wid) if wid else None
    if img is not None:
        edges = tab_edges(app, img)
        assert edges and WIDTH - 40 > edges[0] > 20, edges


def test_a_named_tab_keeps_its_name_when_it_moves(app_factory):
    """a tab named for `--pane` is still found by that name in the window it moved to"""
    app = tab_app(app_factory)
    new_tabs(app, 2)
    app.click(tab_center(app, 1), app.bar_height() // 2)
    app.wait_title("2")
    app.palette("tabs: name for remote control")
    app.type("mover")
    app.key("Return")
    app.wait(lambda: window_tabs(app)[1]["name"] == "mover", msg="the name set")
    app.run("echo NAMED_TAB")
    app.wait(lambda: "NAMED_TAB" in text(app, "mover"), msg="the named tab printed")
    first = one_window(app)
    app.palette("tabs: move to new window")
    app.wait(lambda: len(windows_of(app)) == 2, msg="a window of its own")
    by_window = windows_of(app)
    moved = [tab for window, ts in by_window.items() if window != first for tab in ts][0]
    app.snap("the named tab in the window of its own")
    assert moved["name"] == "mover", moved
    assert "NAMED_TAB" in text(app, "mover"), "the name no longer finds the moved tab"


def test_moving_an_incognito_tab_keeps_it_incognito(app_factory):
    """an incognito tab moved to a window of its own is the same tab: it still refuses to be
    read from outside"""
    app = tab_app(app_factory)
    new_tabs(app, 2)
    app.palette("tabs: new incognito tab")
    app.wait(lambda: len(window_tabs(app)) == 3, msg="the incognito tab")
    private = ids_of(app)[-1]
    first = one_window(app)
    app.palette("tabs: move to new window")
    app.wait(lambda: len(windows_of(app)) == 2, msg="a window of its own")
    by_window = windows_of(app)
    elsewhere = [window for window in by_window if window != first]
    assert ids_of(app, elsewhere[0]) == [private], by_window
    done = msg(app, "get-text", "--pane", private)
    app.snap("the incognito tab in the window of its own")
    assert done.returncode != 0 and "incognito" in done.stderr + done.stdout, done


def test_the_group_entries_group_and_ungroup_a_tab(app_factory):
    """the menu entry of an ungrouped tab puts it in a new group, drawn with the group color
    under it, and the same entry of a grouped tab takes it out again"""
    app = tab_app(app_factory)
    new_tabs(app, 3)
    line = C["ansi"][1]
    assert count(app.shot(), line, 3) == 0, "a group color is drawn already"
    app.click(tab_center(app, 1), app.bar_height() // 2)
    app.wait_title("2")
    x = open_menu(app, 1)
    assert len(menu_entries(app, x)) == 12, menu_entries(app, x)
    menu_click(app, x, GROUP)
    app.wait(lambda: count(app.shot(), line, 3) > 100, msg="the group color under the tab")
    app.snap("the tab in a group of its own, the group's color under it")
    x = open_menu(app, 1)
    # a grouped tab has the take-it-out entry and the group's own entry at the end
    assert len(menu_entries(app, x)) == 13, menu_entries(app, x)
    menu_click(app, x, GROUP)
    app.wait(lambda: count(app.shot(), line, 3) == 0, msg="out of the group")
    app.wait_still()
    x = open_menu(app, 1)
    assert len(menu_entries(app, x)) == 12, menu_entries(app, x)
    app.snap("the tab out of the group again, the menu back to its usual entries")


def test_close_group_entry_closes_the_tabs_of_a_group(app_factory):
    """close group, the extra entry a grouped tab has, closes every tab of that group and
    leaves the others standing"""
    app = tab_app(app_factory)
    new_tabs(app, 4)
    # the second and the third tab join one group
    app.click(tab_center(app, 1), app.bar_height() // 2)
    app.wait_title("2")
    menu_click(app, open_menu(app, 1), GROUP)
    app.wait(lambda: count(app.shot(), C["ansi"][1], 3) > 100, msg="the group")
    under = count(app.shot(), C["ansi"][1], 3)
    app.click(tab_center(app, 2), app.bar_height() // 2)
    app.wait_title("3")
    # the menu entry always starts a group of its own, the palette command joins the
    # neighbour's, which is what puts the two tabs in one group
    app.palette("tabs: add to group")
    app.wait(lambda: count(app.shot(), C["ansi"][1], 3) > under + 100, msg="both under it")
    assert len(ids_of(app)) == 4, ids_of(app)
    grouped = ids_of(app)[1:3]
    app.snap("two tabs in one group, the group's color under both of them")
    x = open_menu(app, 1)
    assert len(menu_entries(app, x)) == 13, menu_entries(app, x)
    menu_click(app, x, CLOSE_GROUP)
    app.wait(lambda: len(ids_of(app)) == 2, msg="the group closed")
    assert not set(grouped) & set(ids_of(app)), (grouped, ids_of(app))
    app.wait(lambda: count(app.shot(), C["ansi"][1], 3) == 0, msg="the color is gone too")
    app.snap("the group closed with its tabs, the other two left")


def test_menu_on_a_tab_of_the_pane_behind(app_factory):
    """a right click in a pane without the focus opens that pane's menu and closes the tab it
    was opened on, not a tab of the focused pane"""
    app = tab_app(app_factory)
    new_tabs(app, 2)
    app.palette("tabs: split right")
    app.wait(lambda: len({tab["pane"] for tab in window_tabs(app)}) == 2, msg="a second pane")
    panes = {tab["pane"] for tab in window_tabs(app)}
    left = min(panes)
    assert len([tab for tab in window_tabs(app) if tab["pane"] == left]) == 1, window_tabs(app)
    # the left pane's bar sits at the left edge, the click lands on the tab it shows
    at = min(tab_center(app, 0), 300)
    app.click(at, app.bar_height() - 4, button=3)
    app.wait(lambda: menu_open(app, at), msg="the menu of the tab in the pane behind")
    box, _ = menu_state(app, at)
    app.snap("the menu of the tab in the pane behind the focused one")
    assert box[0] <= at + 2, box
    menu_click(app, at, CLOSE)
    app.wait(lambda: len(window_tabs(app)) == 1, msg="the tab of the pane behind closed")
    assert len({tab["pane"] for tab in window_tabs(app)}) == 1, window_tabs(app)


def test_menu_with_the_bar_hidden_for_one_tab(app_factory):
    """with the bar hidden while one tab is open there is no tab to right click, and with a
    second tab the bar and its menus are back"""
    app = tab_app(app_factory, hide_bar_for_one_tab=True)
    assert not bar_shown(app), "the bar is drawn for one tab"
    app.snap("one tab, no bar to right click")
    new_tabs(app, 2)
    app.wait(lambda: bar_shown(app), msg="the bar")
    x = open_menu(app, 0)
    box, _ = menu_state(app, x)
    assert abs(box[1] - app.bar_height()) <= 6, box
    app.snap("the bar back with a second tab, the menu of the first tab with it")
    menu_click(app, x, RENAME)
    app.wait(lambda: renaming(app), msg="the rename field")
    app.snap("renaming from the menu of a tab in a bar that was hidden a moment ago")
