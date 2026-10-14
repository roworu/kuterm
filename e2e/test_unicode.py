"""wide and combining characters: cell positions, wrapping, selection, window and tab titles"""

from harness import App, bundled_theme, get_clipboard, ink, mask_bbox, near

FEATURE = "unicode text"

DARK = bundled_theme(True)
RED = DARK["ansi"][1]
BG = DARK["terminal_background"]

# the marker after the tested text is red, so its cell is found by color alone
MARK = "\\033[31mX\\033[0m"


def printed(app_factory, text: str, **settings) -> App:
    """app whose program prints text, the block cursor waits right after it"""
    return app_factory({"theme": {"mode": "dark"}, "terminal": {"cursor_shape": "block"}, **settings},
                       script=f"printf '{text}'")


def red_box(app: App, img=None) -> tuple | None:
    img = app.shot() if img is None else img
    return mask_bbox(near(img, RED, 40) & ~near(img, BG, 40))


def inside(box: tuple, rect: tuple, slack: int = 1) -> bool:
    return (box[0] >= rect[0] - slack and box[1] >= rect[1] - slack
            and box[2] <= rect[2] + slack and box[3] <= rect[3] + slack)


def wait_mark(app: App, col: int, row: int = 0) -> None:
    rect = app.cell_rect(col, row)
    app.wait(lambda: (box := red_box(app)) is not None and inside(box, rect),
             msg=f"red marker in cell ({col}, {row}) {rect}, found at {red_box(app)}")


def cursor_box(app: App) -> tuple | None:
    return mask_bbox(near(app.shot(), DARK["cursor"], 3))


def wait_cursor(app: App, col: int, row: int = 0) -> None:
    rect = app.cell_rect(col, row)
    app.wait(lambda: (box := cursor_box(app)) is not None and inside(box, rect) and box[2] - box[0] >= rect[2] - rect[0] - 2,
             msg=f"block cursor in cell ({col}, {row}) {rect}, found at {cursor_box(app)}")


def test_cjk_takes_two_cells_each(app_factory):
    """two cjk characters fill four cells, the next letter and the cursor follow them"""
    app = printed(app_factory, f"日本{MARK}")
    wait_mark(app, 4)
    wait_cursor(app, 5)
    # the wide glyphs are drawn inside their four cells
    x0, y0, x1, y1 = app.cell_rect(0, 0, 4)
    glyphs = ink(app.shot()[y0:y1], BG) & ~near(app.shot()[y0:y1], RED, 40)
    box = mask_bbox(glyphs[:, : x1 + 1])
    assert box is not None and box[0] >= x0 - 1 and box[2] <= x1 + 1, (box, (x0, x1))
    app.snap("日本 then a red X in cell 4, the cursor in cell 5")


def test_emoji_takes_two_cells(app_factory):
    """an emoji fills two cells"""
    app = printed(app_factory, f"😀{MARK}")
    wait_mark(app, 2)
    wait_cursor(app, 3)
    app.snap("an emoji then a red X in cell 2")


def glyph_box(app: App, img, row: int, cols: int) -> tuple | None:
    """bbox of ink in the first cols cells of a line, x in window pixels, y within the row"""
    _, y0, x1, y1 = app.cell_rect(0, row, cols)
    # stop short of the next cell, the cursor may sit there
    return mask_bbox(ink(img[y0:y1, : x1 - 2], BG))


def test_combining_accent_stays_in_its_cell(app_factory):
    """e with a combining acute is one cell, the accent is drawn above the e"""
    # two spaces keep the marker away, so only the e and its accent are measured
    app = printed(app_factory, f"e\\314\\201  {MARK}\\ne")
    wait_mark(app, 3)
    app.wait(lambda: glyph_box(app, app.shot(), 1, 1), msg="plain e")
    img = app.shot()
    accented = glyph_box(app, img, 0, 3)
    # the cursor waits right after the plain e
    plain = glyph_box(app, img, 1, 1)
    assert accented is not None and plain is not None
    x0, _, x1, _ = app.cell_rect(0, 0)
    assert accented[0] >= x0 - 1 and accented[2] <= x1 + 1, (accented, (x0, x1))
    # both boxes are relative to their row, the accent reaches higher than the plain e
    assert accented[1] < plain[1] - 1, (accented, plain)
    app.snap("é made of e and a combining accent, then a red X in cell 3, a plain e below")


def test_wide_char_at_last_column_wraps(app_factory):
    """a wide char that does not fit in the last column goes to the start of the next line"""
    app = app_factory({"theme": {"mode": "dark"}},
                      script='c=$(stty size | cut -d" " -f2); printf "%*s" $((c - 1)) ""; '
                             "printf '\\033[31m日\\033[0m'")
    rect = app.cell_rect(0, 1, 2)
    app.wait(lambda: (box := red_box(app)) is not None and inside(box, rect),
             msg=f"red wide char at the start of row 1 {rect}, found at {red_box(app)}")
    app.snap("a red 日 wrapped to the next line instead of splitting over the edge")


def test_double_click_copies_wide_word(app_factory):
    """a double click on a cjk word copies its exact utf-8"""
    app = printed(app_factory, f"日本語 abc{MARK}")
    wait_mark(app, 10)
    x0, y0, x1, y1 = app.cell_rect(2, 0)
    app.click((x0 + x1) // 2, (y0 + y1) // 2, repeat=2)
    app.key("ctrl+shift+c")
    app.wait(lambda: get_clipboard() == "日本語", msg=f"clipboard, got {get_clipboard()!r}")
    app.snap("日本語 selected with a double click")


def test_wide_window_title(app_factory):
    """an osc 2 title with cjk and emoji is the window title exactly"""
    app = printed(app_factory, "\\033]2;日本語 😀 title\\007", window_title=["title"])
    app.wait_title("日本語 😀 title")
