"""tab bar on screen: hiding, tab width, expanded tabs, title align, icons, new tab button, mouse"""

import time

import numpy as np
import pytest

from harness import App, as_rgb, close_to, ink, mask_bbox, near, unique_theme

FEATURE = "tab bar"

THEME = unique_theme(0x60)
C = as_rgb(THEME)
# the title text alone, no icon, so ink in a tab is its title and close button
PLAIN = {"tab_title": [{"text": "title"}]}


def bar_app(app_factory, **settings) -> App:
    # dividers are found by single pixels at the window middle, a thin one may miss them
    # a frame of its own, so the top row is only ever the tab bar
    base = {"theme": {"mode": "dark"}, "window_title": ["number"], "panes": {"divider_width": 4},
            "window": {"decorations": "server"}}
    app = app_factory({**base, **settings}, themes={"dark": THEME})
    # keep the pointer away from the bar, hover changes its colors
    app.mouse(450, 500)
    return app


def open_tabs(app: App, n: int) -> None:
    """press ctrl+shift+t until there are n tabs, the window title is the active tab number"""
    for i in range(2, n + 1):
        app.key("ctrl+shift+t")
        app.wait_title(str(i))


def bar_shown(app: App, img=None) -> bool:
    img = app.shot() if img is None else img
    return close_to(tuple(img[app.bar_height() - 1, 450]), C["border"])


def borders(app: App, img=None) -> list:
    """x of every border column in the middle row of the bar, the right edge of each tab"""
    img = app.shot() if img is None else img
    row = near(img[app.bar_height() // 2 : app.bar_height() // 2 + 1], C["border"])[0]
    return [int(x) for x in np.flatnonzero(row)]


def muted_ink(app: App, img, x0: int = 0, x1: int = 900) -> tuple | None:
    """bbox of text_muted glyphs in the bar between x0 and x1, like the "+" and inactive titles"""
    h = app.bar_height()
    region = img[1 : h - 2, x0:x1]
    return mask_bbox(near(region, C["text_muted"], 40) & ink(region, C["tab_bar_background"]))


def test_bar_hidden_for_one_tab_by_default(app_factory):
    """hide_bar_for_one_tab hides the bar with one tab, shows it with two, hides it again"""
    app = bar_app(app_factory)
    assert not bar_shown(app)
    app.snap("one tab, no bar")
    open_tabs(app, 2)
    app.wait(lambda: bar_shown(app), msg="bar with two tabs")
    app.snap("two tabs, bar shown")
    app.key("ctrl+shift+w")
    app.wait_title("1")
    app.wait(lambda: not bar_shown(app), msg="bar hidden again")


def test_bar_always_shown_when_not_hiding(app_factory):
    """hide_bar_for_one_tab false keeps the bar with a single tab"""
    app = bar_app(app_factory, hide_bar_for_one_tab=False)
    app.wait(lambda: bar_shown(app), msg="bar")
    img = app.shot()
    h = app.bar_height()
    assert close_to(tuple(img[h // 2, 5]), C["tab_active_background"])
    assert close_to(tuple(img[h // 2, 895]), C["tab_bar_background"])
    assert close_to(tuple(img[h + 2, 450]), C["terminal_background"])


def test_terminal_starts_below_bar(app_factory):
    """the terminal gives the bar its rows, the shell sees fewer lines"""
    app = bar_app(app_factory, hide_bar_for_one_tab=False)
    assert app.pty_size() == app.expected_pty(900, 600, top=app.bar_height())


@pytest.mark.parametrize("width", [120, 220, 260])
def test_fixed_tab_width(app_factory, width):
    """tab_width sets the width of every tab"""
    app = bar_app(app_factory, tab_width=width, **PLAIN)
    open_tabs(app, 3)
    app.wait(lambda: borders(app)[:3] == [width - 1, 2 * width - 1, 3 * width - 1],
             msg=f"tab edges every {width}px, got {borders(app)}")


@pytest.mark.parametrize("tabs", [2, 4])
def test_expanded_tabs_share_the_bar(app_factory, tabs):
    """expand_tabs splits the bar equally, ignoring tab_width"""
    app = bar_app(app_factory, expand_tabs=True, tab_width=50, **PLAIN)
    open_tabs(app, tabs)

    def equal() -> bool:
        edges = borders(app)[:tabs]
        widths = np.diff([-1, *edges])
        return len(edges) == tabs and widths.min() > 100 and widths.max() - widths.min() <= 1

    app.wait(equal, msg=f"equal tabs, edges {borders(app)}")


@pytest.mark.parametrize("align", ["left", "center", "right"])
def test_title_align(app_factory, align):
    """tab_title_align puts the title at the left, center or right of its tab"""
    app = bar_app(app_factory, tab_title_align=align, tab_width=300, **PLAIN)
    open_tabs(app, 2)
    # first tab is inactive, its close button hidden, so its only ink is the title
    app.wait(lambda: muted_ink(app, app.shot(), 0, 299), msg="title of the first tab")
    x0, _, x1, _ = muted_ink(app, app.shot(), 0, 299)
    middle = (x0 + x1) / 2
    expected = {"left": middle < 100, "center": 100 <= middle <= 200, "right": middle > 200}[align]
    assert expected, (align, x0, x1)


@pytest.mark.parametrize("icon_first", [True, False])
def test_tab_icon_block_places_icon(app_factory, icon_first):
    """the "icon" block of tab_title draws the icon where it sits among the other blocks"""
    # spaces keep the wide icon and the narrow letter apart, each half of the ink holds one
    title = ["icon", {"text": "        i"}] if icon_first else [{"text": "i        "}, "icon"]
    app = bar_app(app_factory, tab_width=300, tab_title=title, icons={"dynamic": False, "default": "WWW"})
    open_tabs(app, 2)
    app.wait(lambda: muted_ink(app, app.shot(), 0, 299), msg="icon and title")
    img = app.shot()
    x0, _, x1, _ = muted_ink(app, img, 0, 299)
    mid = (x0 + x1) // 2
    left, right = muted_ink(app, img, x0, mid), muted_ink(app, img, mid, x1 + 1)
    assert left and right, (x0, x1)
    assert (left[2] - left[0] > right[2] - right[0]) == icon_first, (left, right)


@pytest.mark.parametrize("dynamic", [True, False])
def test_dynamic_tab_icon_follows_program(app_factory, dynamic):
    """icons.dynamic shows the icon of the running program, false keeps the default one"""
    app = app_factory({"theme": {"mode": "dark"}, "hide_bar_for_one_tab": False,
                       "tab_title": ["icon", {"text": "t"}], "icons": {"dynamic": dynamic, "default": "D"}},
                      themes={"dark": THEME},
                      tab_icons={"groups": [{"icon": "", "commands": ["sleep"]}]})
    app.mouse(450, 500)
    h = app.bar_height()
    app.wait(lambda: mask_bbox(ink(app.shot()[1 : h - 2, :100], C["tab_active_background"])), msg="icon")
    before = app.shot()[1 : h - 2, :100].copy()
    app.snap("shell running")
    app.run("sleep 30")
    if dynamic:
        app.wait(lambda: (app.shot()[1 : h - 2, :100] != before).any(), msg="icon change")
        app.snap("sleep running, its icon shown")
    else:
        # titles and icons refresh every second
        time.sleep(2.5)
        assert (app.shot()[1 : h - 2, :100] == before).all()


@pytest.mark.parametrize("placement", ["left", "right", "after_tabs"])
def test_new_tab_button(app_factory, placement):
    """new_tab_button sits left, right or after the tabs, a click opens a tab"""
    app = bar_app(app_factory, hide_bar_for_one_tab=False, new_tab_button=placement, tab_width=200, **PLAIN)
    app.wait(lambda: bar_shown(app), msg="bar")
    img = app.shot()
    tab_edges = borders(app, img)
    if placement == "left":
        plus = muted_ink(app, img, 0, 60)
        assert plus is not None and tab_edges[0] > 200, tab_edges
    elif placement == "right":
        plus = muted_ink(app, img, 840, 900)
        assert plus is not None and tab_edges[0] == 199
    else:
        plus = muted_ink(app, img, 200, 260)
        assert plus is not None and tab_edges[0] == 199
    x0, y0, x1, y1 = plus
    base = {"left": 0, "right": 840, "after_tabs": 200}[placement]
    app.click(base + (x0 + x1) // 2, 1 + (y0 + y1) // 2)
    app.wait_title("2")


def test_after_tabs_button_moves_with_tabs(app_factory):
    """the after_tabs button follows the last tab as tabs open"""
    app = bar_app(app_factory, hide_bar_for_one_tab=False, new_tab_button="after_tabs", tab_width=150, **PLAIN)
    open_tabs(app, 3)
    app.mouse(450, 500)
    app.wait(lambda: muted_ink(app, app.shot(), 450, 510), msg="plus after the third tab")


def test_click_tab_activates_it(app_factory):
    """clicking a tab switches to it"""
    app = bar_app(app_factory, tab_width=200, **PLAIN)
    open_tabs(app, 3)
    app.click(100, app.bar_height() // 2)
    app.wait_title("1")
    app.wait(lambda: close_to(tuple(app.shot()[app.bar_height() // 2, 5]), C["tab_active_background"]),
             msg="first tab painted active")


def test_close_button_closes_tab(app_factory):
    """the x on the active tab closes it"""
    app = bar_app(app_factory, tab_width=200, **PLAIN)
    open_tabs(app, 3)
    h = app.bar_height()
    img = app.shot()
    # the active third tab shows its close button at its right end
    region = img[1 : h - 2, 400 + 150 : 400 + 198]
    box = mask_bbox(ink(region, C["tab_active_background"]))
    assert box is not None
    app.click(400 + 150 + (box[0] + box[2]) // 2, 1 + (box[1] + box[3]) // 2)
    app.wait_title("2")


@pytest.mark.parametrize("show", [True, False])
def test_show_tab_close_button(app_factory, show):
    """show_tab_close_button false hides the x on the active and hovered tab, middle click still closes"""
    app = bar_app(app_factory, tab_width=200, show_tab_close_button=show, **PLAIN)
    open_tabs(app, 2)
    h = app.bar_height()
    # hover the first tab too, hovered tabs show their x like the active one
    app.mouse(100, h // 2)
    time.sleep(0.3)
    img = app.shot()
    active = mask_bbox(ink(img[1 : h - 2, 200 + 150 : 200 + 198], C["tab_active_background"]))
    hovered = mask_bbox(ink(img[1 : h - 2, 150:198], C["tab_bar_background"]))
    assert (active is not None) == show and (hovered is not None) == show, (active, hovered)
    app.snap("x shown" if show else "no x, first tab hovered")
    app.click(100, h // 2, button=2)
    app.wait_title("1")


def test_middle_click_closes_tab(app_factory):
    """a middle click on a tab closes it"""
    app = bar_app(app_factory, tab_width=200, **PLAIN)
    open_tabs(app, 3)
    app.click(100, app.bar_height() // 2, button=2)
    app.wait(lambda: len(borders(app)) == 2, msg=f"two tabs left, edges {borders(app)}")


def test_drag_reorders_tabs(app_factory):
    """dragging the first tab past the others moves it, the active tab follows its tab"""
    app = bar_app(app_factory, tab_width=200, **PLAIN)
    open_tabs(app, 3)
    h = app.bar_height()
    app.drag((100, h // 2), (500, h // 2))
    # the first tab is now last, so the active third tab sits in the middle
    app.wait_title("2")
    app.snap("first tab dragged to the end")
    app.click(100, h // 2)
    app.wait_title("1")


def test_disabled_drag_tabs_keeps_order(app_factory):
    """drag_tabs false ignores the mouse drag, the tabs keep their order"""
    app = bar_app(app_factory, tab_width=200, drag_tabs=False, **PLAIN)
    open_tabs(app, 3)
    h = app.bar_height()
    app.drag((100, h // 2), (500, h // 2))
    time.sleep(0.5)
    assert app.title() == "3", app.title()
    app.click(100, h // 2)
    app.wait_title("1")


def divider_shown(app: App, vertical: bool) -> bool:
    """true when a border colored divider crosses the middle of the window"""
    w, h = app.size()
    mid = w // 2 if vertical else h // 2
    img = app.shot()
    strip = img[h // 2 - 8 : h // 2 + 8, mid] if vertical else img[mid, w // 2 - 8 : w // 2 + 8]
    return near(strip.reshape(-1, 1, 3), C["border"]).any()


def test_drag_tab_to_a_side_splits_the_pane(app_factory):
    """dragging a tab to the right half of the terminal splits it into two panes"""
    app = bar_app(app_factory, tab_width=200, **PLAIN)
    open_tabs(app, 2)
    w, h = app.size()
    app.drag((100, app.bar_height() // 2), (w * 3 // 4, h // 2))
    app.wait(lambda: divider_shown(app, vertical=True), msg="no vertical divider between the panes")
    app.snap("tab dragged to the right half split the pane")


def test_focus_follows_mouse(app_factory):
    """with focus_follows_mouse, moving the mouse over a pane focuses it, typing goes there"""
    app = bar_app(app_factory, tab_width=200, focus_follows_mouse=True, **PLAIN)
    # the prompt and the cursor after it
    app.wait_line(0, 3, top=app.top())
    app.run("export WHO=first")
    open_tabs(app, 2)
    app.run("export WHO=second")
    w, h = app.size()
    # the first tab moves into a new right pane, which gets focus
    app.drag((100, app.bar_height() // 2), (w * 3 // 4, h // 2))
    app.wait(lambda: divider_shown(app, vertical=True), msg="no vertical divider between the panes")
    app.mouse(w // 4, h // 2)
    time.sleep(0.3)
    app.run('echo "$WHO" > "$T2_TMP/who"')
    assert app.wait_file("who").strip() == "second"
    app.snap("mouse over the left pane focused it")


def test_drag_tab_to_the_bottom_half_splits_the_pane(app_factory):
    """dragging a tab to the bottom half of the terminal splits it into two panes"""
    app = bar_app(app_factory, tab_width=200, **PLAIN)
    open_tabs(app, 2)
    w, h = app.size()
    app.drag((100, app.bar_height() // 2), (w // 2, h * 3 // 4))
    app.wait(
        lambda: divider_shown(app, vertical=False),
        msg="no horizontal divider between the panes",
    )
    app.snap("tab dragged to the bottom half split the pane")


def border_at(app: App, x: int, y: int) -> bool:
    """true when the pixel at (x, y) has the theme border color"""
    return near(app.shot()[y : y + 1, x : x + 1], C["border"]).any()


def divider_axis(app: App) -> str | None:
    """orientation of the split divider crossing the middle of the window, none when unsplit"""
    w, h = app.size()
    if not border_at(app, w // 2, h // 2):
        return None
    if border_at(app, w // 2, h // 4):
        return "vertical"
    if border_at(app, w // 4, h // 2):
        return "horizontal"
    return None


def dialog(app: App) -> tuple | None:
    """bbox of the danger colored close button, none while no confirmation is up"""
    mask = near(app.shot(), C["danger_button"])
    return mask_bbox(mask) if mask.sum() > 200 else None


@pytest.mark.parametrize(
    "command,axis",
    [
        ("split left", "vertical"),
        ("split right", "vertical"),
        ("split up", "horizontal"),
        ("split down", "horizontal"),
    ],
)
def test_split_commands_split_the_focused_pane(app_factory, command, axis):
    """the bundled split commands move the active tab into a new pane on the given side"""
    app = bar_app(app_factory, tab_width=200, **PLAIN)
    open_tabs(app, 2)
    app.palette(command)
    app.wait(lambda: divider_axis(app) == axis, msg=f"no {axis} divider after {command}")
    app.snap(f"{command} split the focused pane")


def test_closing_one_nested_pane_keeps_its_sibling(app_factory):
    """closing the bottom of two stacked panes keeps the top one beside the left pane"""
    app = bar_app(app_factory, tab_width=200, **PLAIN)
    open_tabs(app, 2)
    app.palette("split right")
    app.wait(lambda: divider_axis(app) == "vertical", msg="no side by side split")
    app.palette("split down")
    w, h = app.size()
    app.wait(lambda: border_at(app, w * 3 // 4, h // 2), msg="no stacked split in the right pane")
    # the new bottom pane has focus, closing its only tab must not close the top pane too
    app.key("ctrl+shift+w")
    app.wait(lambda: not border_at(app, w * 3 // 4, h // 2), msg="the bottom pane did not close")
    assert divider_axis(app) == "vertical", "the top pane was closed with the bottom one"
    app.snap("closing the bottom pane kept the top one")


def test_closing_the_surviving_pane_after_a_nested_collapse(app_factory):
    """closing a nested pane gives focus to its sibling, so the next close acts on it"""
    app = bar_app(app_factory, tab_width=200, **PLAIN)
    open_tabs(app, 2)
    app.palette("split right")
    app.wait(lambda: divider_axis(app) == "vertical", msg="no side by side split")
    app.palette("split down")
    w, h = app.size()
    app.wait(lambda: border_at(app, w * 3 // 4, h // 2), msg="no stacked split in the right pane")
    # a program runs in the top pane, then the focused bottom pane closes
    app.click(w * 3 // 4, h // 4)
    app.run("sleep 1000")
    time.sleep(0.5)
    app.click(w * 3 // 4, h * 3 // 4)
    app.key("ctrl+shift+w")
    app.wait(lambda: not border_at(app, w * 3 // 4, h // 2), msg="the bottom pane did not close")
    # the top pane took its place and keeps focus, so closing it must ask about the program
    app.key("ctrl+shift+w")
    app.wait(lambda: dialog(app), msg="the surviving pane did not get focus")
    app.snap("closing the surviving pane asked about its program")


def test_overflowing_tabs_keep_width_and_scroll(app_factory):
    """tabs that do not fit keep their width, the wheel scrolls them into view"""
    app = bar_app(app_factory, tab_width=250, new_tab_button="right", **PLAIN)
    open_tabs(app, 6)
    h = app.bar_height()
    # the active last tab is scrolled into view at the right
    app.wait(lambda: close_to(tuple(app.shot()[h // 2, 800]), C["tab_active_background"]), msg="last tab")
    app.snap("six tabs, scrolled to the active one")
    app.wheel(300, h // 2, up=True, clicks=10)
    app.wait(lambda: not close_to(tuple(app.shot()[h // 2, 800]), C["tab_active_background"]),
             msg="tabs scrolled by the wheel")


# truncation checks: every tab looks alike, so each can be compared with one uncut reference
CUT_THEME = {**THEME, "text_muted": THEME["text"], "tab_active_background": THEME["tab_bar_background"]}
CUT_BG = as_rgb(CUT_THEME)["tab_bar_background"]
CUT_BASE = {"show_tab_close_button": False,
            "hide_bar_for_one_tab": False}
LONG = "a long title with several words in it"
# widest char of the titles below, in ui font pixels: cjk glyphs are about one em
EM = 18 * 0.875
_references: dict = {}


def cut_app(app_factory, title: str, **settings) -> App:
    app = app_factory({"theme": {"mode": "dark"}, "window_title": ["number"], **CUT_BASE,
                       "tab_title": [{"text": title}], **settings},
                      themes={"dark": CUT_THEME})
    app.mouse(450, 500)
    return app


def text_mask(app: App, img, x0: int, x1: int):
    """ink of the title rows between x0 and x1, the bar border rows left out"""
    h = app.bar_height()
    return ink(img[3 : h - 3, x0:x1], CUT_BG)


def trim(mask):
    """the mask from its first to its last inked column"""
    cols = np.flatnonzero(mask.any(axis=0))
    return mask[:, cols[0] : cols[-1] + 1] if len(cols) else mask[:, :0]


def reference(app_factory, title: str):
    """the title drawn uncut in a tab much wider than it"""
    if title not in _references:
        app = cut_app(app_factory, title, expand_tabs=False, tab_width=880)
        app.wait_still("the title drawn")
        # stop short of the tab's border, it is ink too
        _references[title] = trim(text_mask(app, app.shot(), 0, 860))
        app.close()
    return _references[title]


def dilate(mask):
    """grow ink by one pixel in every direction"""
    out = mask.copy()
    for dy in (-1, 0, 1):
        for dx in (-1, 0, 1):
            out |= np.roll(np.roll(mask, dy, axis=0), dx, axis=1)
    return out


def same_shape(a, b) -> bool:
    """two inked texts match within a pixel, glyphs drawn at other subpixel offsets differ that much"""
    w = max(a.shape[1], b.shape[1]) + 2
    pad = lambda m: np.pad(m, ((0, 0), (1, w - m.shape[1] - 1)))
    a, b = pad(a), pad(b)
    return not (a & ~dilate(b)).any() and not (b & ~dilate(a)).any()


def runs(cols) -> list:
    """(first, last) of each group of adjacent columns"""
    groups = np.split(cols, np.flatnonzero(np.diff(cols) > 1) + 1) if len(cols) else []
    return [(int(g[0]), int(g[-1])) for g in groups]


def check_tab(mask, ref, ellipsis, room: int, where: str) -> None:
    """a tab title is the whole reference when it has room, otherwise a start of it and an
    ellipsis, keeping as much as fits"""
    text = trim(mask)
    assert text.shape[1], f"{where}: no title"
    if ref.shape[1] <= room:
        assert same_shape(text, ref), f"{where}: title fits in {room}px but is not drawn whole"
        return
    width = text.shape[1]
    # antialiasing blurs where the kept text ends and the ellipsis starts by a pixel or two
    splits = range(width - ellipsis.shape[1] - 3, width - ellipsis.shape[1] + 3)
    assert any(same_shape(trim(text[:, x:]), ellipsis) for x in splits), \
        f"{where}: cut title does not end in an ellipsis"
    assert any((kept := trim(text[:, :x])).shape[1] and same_shape(kept, ref[:, : kept.shape[1]])
               for x in splits), f"{where}: the kept text is not the start of the title"
    # one more char and its space would not have fit
    assert room - text.shape[1] < 2 * EM, f"{where}: cut {room - text.shape[1]}px short of its room"


def check_tabs(app: App, ref, ellipsis, count: int) -> None:
    img = app.shot()
    edges = borders(app, img)[:count]
    assert len(edges) == count, edges
    padding = round(0.75 * 18)
    for ix, right in enumerate(edges):
        left = edges[ix - 1] + 1 if ix else 0
        mask = text_mask(app, img, left, right)
        start = int(np.flatnonzero(mask.any(axis=0))[0]) if mask.any() else padding
        check_tab(mask, ref, ellipsis, right - left - padding - start + 1, f"tab {ix + 1} of {count}")


@pytest.mark.parametrize("align", ["left", "center", "right"])
@pytest.mark.parametrize("layout", [
    {"expand_tabs": True, "tabs": 2}, {"expand_tabs": True, "tabs": 3}, {"expand_tabs": True, "tabs": 5},
    {"expand_tabs": True, "tabs": 8}, {"expand_tabs": False, "tab_width": 120, "tabs": 3},
    {"expand_tabs": False, "tab_width": 260, "tabs": 3},
], ids=["expand-2", "expand-3", "expand-5", "expand-8", "fixed-120", "fixed-260"])
def test_long_titles_are_cut_only_at_the_end(app_factory, align, layout):
    """every tab shows the start of a long title and an ellipsis, as much as fits"""
    layout = dict(layout)
    tabs = layout.pop("tabs")
    ref = reference(app_factory, LONG)
    app = cut_app(app_factory, LONG, tab_title_align=align, **layout)
    open_tabs(app, tabs)
    app.wait_still("the tabs laid out")
    check_tabs(app, ref, reference(app_factory, "…"), tabs)
    app.snap(f"{tabs} tabs with a long title, {align} aligned")


@pytest.mark.parametrize("title", ["tab", "x  y  z  w  v  u  t  s  r  q", "日本語のタブの名前です長い"],
                         ids=["short", "spaces", "cjk"])
def test_titles_of_every_kind_are_cut_only_at_the_end(app_factory, title):
    """short titles stay whole, spaced and cjk titles are cut at the end only"""
    ref = reference(app_factory, title)
    app = cut_app(app_factory, title, expand_tabs=True)
    open_tabs(app, 6)
    app.wait_still("the tabs laid out")
    check_tabs(app, ref, reference(app_factory, "…"), 6)
    app.snap(f"six expanded tabs titled {title!r}")


def test_cut_follows_window_resize(app_factory):
    """expanded tabs get narrower with the window and the titles are cut again to fit"""
    ref = reference(app_factory, LONG)
    app = cut_app(app_factory, LONG, expand_tabs=True)
    open_tabs(app, 3)
    for width in (900, 640, 480, 900):
        app.resize(width, 600)
        app.wait(lambda: app.size()[0] == width, msg=f"width {width}")
        app.wait_still("the tabs laid out")
        check_tabs(app, ref, reference(app_factory, "…"), 3)
    app.snap("titles cut again after resizing")
