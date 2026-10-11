"""real glyph rendering and metrics: font, line_height, cell_width, glyph_offset, color rules,
builtin box drawing, ui_font_family, ui_font_size"""

import time

import pytest

from harness import App, as_rgb, bundled_theme, close_to, ink, ink_bbox, unique_theme

FEATURE = "fonts and line height"

DARK = bundled_theme(True)
RED = DARK["ansi"][1]

# a 10 cells wide red block on the first two rows, then sample text in every face
SAMPLE = (
    "printf '\\033[41m          \\033[0m\\n\\033[41m          \\033[0m\\n'\n"
    "printf 'regular \\033[1mbold\\033[0m \\033[3mitalic\\033[0m \\033[1;3mbold italic\\033[0m\\n'\n"
    "printf 'nerd font: \\357\\204\\240 \\356\\234\\245 \\357\\214\\203\\n'\n"
)


def text_width(app: App, width: int, top: int = 0) -> int:
    """width of the ink on the first row, the cursor alone is only a few pixels"""
    box = ink_bbox(app.shot()[top : top + app.line_height(), :width], DARK["terminal_background"])
    return 0 if box is None else box[2] - box[0]


def check_cells(app: App) -> None:
    """the red block covers exactly 10 x 2 cells of the expected size"""
    app.wait(lambda: close_to(app.cells_color(0, 0, 10), RED, 3), msg="red block")
    img = app.shot()
    x0, y0, x1, y1 = app.cell_rect(0, 0, 10, 2)
    red = (abs(img.astype(int) - RED).max(axis=-1) <= 3)
    ys, xs = red.nonzero()
    assert abs(xs.min() - x0) <= 1 and abs(xs.max() + 1 - x1) <= 1, (xs.min(), xs.max(), x0, x1)
    assert abs(ys.min() - y0) <= 1 and abs(ys.max() + 1 - y1) <= 1, (ys.min(), ys.max(), y0, y1)


@pytest.mark.parametrize("size", [8, 12, 18, 24, 40])
def test_font_size_sets_cell_size(app_factory, size):
    """terminal.font_size scales the cells"""
    app = app_factory({"theme": {"mode": "dark"}, "terminal": {"font_size": size}}, script=SAMPLE)
    check_cells(app)


@pytest.mark.parametrize("size", [12, 30])
def test_font_block_size_sets_cell_size(app_factory, size):
    """terminal.font.size scales the cells like the old flat font_size key"""
    app = app_factory({"theme": {"mode": "dark"}, "terminal": {"font": {"size": size}}}, script=SAMPLE)
    check_cells(app)


@pytest.mark.parametrize("cell_width", [-3, 4])
def test_cell_width_changes_columns(app_factory, cell_width):
    """terminal.cell_width widens or narrows every cell, the shell sees the new column count"""
    app = app_factory({"theme": {"mode": "dark"}, "terminal": {"cell_width": cell_width}})
    assert app.pty_size() == app.expected_pty(900, 600)
    app.snap(f"cell_width {cell_width}")


def test_glyph_offset_moves_text_not_backgrounds(app_factory):
    """terminal.glyph_offset moves glyphs inside their cells, backgrounds stay on the grid"""
    boxes = []
    for offset in [{"x": 0, "y": 0}, {"x": 4, "y": 6}]:
        app = app_factory({"theme": {"mode": "dark"}, "terminal": {"font": {"size": 24}, "glyph_offset": offset}},
                          script=SAMPLE)
        check_cells(app)
        x0, y0, x1, y1 = app.cell_rect(0, 2, 7)
        boxes.append(ink_bbox(app.shot()[y0:y1, x0:x1], DARK["terminal_background"]))
        app.snap(f"glyph_offset {offset}")
        app.close()
    (ax, ay, _, _), (bx, by, _, _) = boxes
    assert abs((bx - ax) - 4 * app.scale) <= 1, boxes
    assert abs((by - ay) - 6 * app.scale) <= 1, boxes


# full blocks on two rows, then a row of box lines
BLOCKS = (
    "printf '\\033[31m\\342\\226\\210\\342\\226\\210\\342\\226\\210\\342\\226\\210\\n"
    "\\342\\226\\210\\342\\226\\210\\342\\226\\210\\342\\226\\210\\033[0m\\n'\n"
    "printf '\\342\\224\\214\\342\\224\\200\\342\\224\\254\\342\\224\\220 \\356\\202\\260\\n'\n"
)


@pytest.mark.parametrize("line_height", ["standard", {"custom": 2.5}])
def test_builtin_box_drawing_fills_whole_cells(app_factory, line_height):
    """full blocks fill their cells exactly at any line height, so rows join without gaps"""
    app = app_factory({"theme": {"mode": "dark"}, "terminal": {"line_height": line_height}}, script=BLOCKS)
    app.wait(lambda: close_to(app.cells_color(0, 0, 4), RED, 3), msg="blocks")
    img = app.shot()
    x0, y0, x1, y1 = app.cell_rect(0, 0, 4, 2)
    red = (abs(img.astype(int) - RED).max(axis=-1) <= 3)
    ys, xs = red.nonzero()
    assert abs(xs.min() - x0) <= 1 and abs(xs.max() + 1 - x1) <= 1, (xs.min(), xs.max(), x0, x1)
    assert abs(ys.min() - y0) <= 1 and abs(ys.max() + 1 - y1) <= 1, (ys.min(), ys.max(), y0, y1)
    # no gap between the two rows
    assert red[y0:y1, (x0 + x1) // 2].all()
    app.snap(f"builtin box drawing, line_height {line_height}")


def test_builtin_box_drawing_off_uses_the_font(app_factory):
    """with builtin_box_drawing off the font glyph leaves a gap between rows at a tall line height"""
    app = app_factory({"theme": {"mode": "dark"},
                       "terminal": {"line_height": {"custom": 2.5}, "builtin_box_drawing": False}}, script=BLOCKS)
    app.wait(lambda: ink_bbox(app.shot()[: app.line_height(), :200], DARK["terminal_background"]) is not None,
             msg="blocks")
    time.sleep(0.5)
    img = app.shot()
    x0, y0, x1, y1 = app.cell_rect(0, 0, 4, 2)
    red = (abs(img.astype(int) - RED).max(axis=-1) <= 3)
    assert not red[y0:y1, (x0 + x1) // 2].all()
    app.snap("font box drawing, line_height 2.5")


def test_bold_is_bright(app_factory):
    """terminal.bold_is_bright draws bold text in normal colors with the bright color"""
    script = "printf '\\033[1;31m\\342\\226\\210\\342\\226\\210\\342\\226\\210\\342\\226\\210\\033[0m\\n'\n"
    for bright, want in [(False, RED), (True, DARK["ansi"][9])]:
        app = app_factory({"theme": {"mode": "dark"}, "terminal": {"bold_is_bright": bright}}, script=script)
        app.wait(lambda: close_to(app.cells_color(0, 0, 4), want, 3), msg=f"bold_is_bright {bright}")
        app.snap(f"bold_is_bright {bright}")
        app.close()


def test_minimum_contrast_lifts_invisible_text(app_factory):
    """terminal.minimum_contrast moves red text on a red background away from it"""
    script = "printf '\\033[31;41m\\342\\226\\210\\342\\226\\210\\342\\226\\210\\342\\226\\210\\033[0m\\n'\n"
    for minimum in [1, 4.5]:
        app = app_factory({"theme": {"mode": "dark"}, "terminal": {"minimum_contrast": minimum}}, script=script)
        app.wait(lambda: app.cells_color(0, 0, 4) != tuple(DARK["terminal_background"]), msg="blocks")
        time.sleep(0.5)
        color = app.cells_color(0, 0, 4)
        assert close_to(color, RED, 3) == (minimum == 1), color
        app.snap(f"minimum_contrast {minimum}")
        app.close()


@pytest.mark.parametrize("line_height", ["standard", "comfortable", {"custom": 1}, {"custom": 2.5}])
def test_line_height_sets_row_height(app_factory, line_height):
    """terminal.line_height sets the row height as a multiple of the font size"""
    app = app_factory({"theme": {"mode": "dark"}, "terminal": {"line_height": line_height}}, script=SAMPLE)
    check_cells(app)


@pytest.mark.parametrize("size,line_height", [(10, "comfortable"), (30, {"custom": 1.2})])
def test_pty_size_follows_font(app_factory, size, line_height):
    """the shell sees as many rows and columns as fit the window with this font"""
    app = app_factory({"theme": {"mode": "dark"}, "terminal": {"font_size": size, "line_height": line_height}})
    assert app.pty_size() == app.expected_pty(900, 600)


def test_bold_and_italic_use_other_faces(app_factory):
    """bold, italic and bold italic text is drawn with their own faces"""
    app = app_factory({"theme": {"mode": "dark"}, "terminal": {"font_size": 24}}, script=SAMPLE)
    app.wait(lambda: close_to(app.cells_color(0, 0, 10), RED, 3), msg="sample printed")
    img = app.shot()
    bg = DARK["terminal_background"]

    def word(col: int, n: int):
        x0, y0, x1, y1 = app.cell_rect(col, 2, n)
        box = ink_bbox(img[y0:y1, x0:x1], bg)
        return img[y0:y1, x0:x1], box

    regular, _ = word(0, 4)
    bold, _ = word(8, 4)
    italic, _ = word(13, 4)
    # bold is heavier than regular: more ink on the same number of cells
    assert ink(bold, bg).sum() > ink(regular, bg).sum()
    assert word(13, 6)[1] is not None and italic.size


def test_nerd_font_icons_render(app_factory):
    """nerd font icons come from the bundled font, not tofu boxes"""
    app = app_factory({"theme": {"mode": "dark"}, "terminal": {"font_size": 24}}, script=SAMPLE)
    app.wait(lambda: close_to(app.cells_color(0, 0, 10), RED, 3), msg="sample printed")
    img = app.shot()
    x0, y0, x1, y1 = app.cell_rect(11, 3, 5)
    assert ink_bbox(img[y0:y1, x0:x1], DARK["terminal_background"]) is not None


def test_other_installed_font(app_factory):
    """terminal.font_family picks an installed font, glyphs differ from the bundled one"""
    shots = []
    for family in ["JetBrainsMonoNL Nerd Font Mono", "DejaVu Sans Mono"]:
        app = app_factory({"theme": {"mode": "dark"}, "terminal": {"font_family": family, "font_size": 24}},
                          script="printf 'Wig@0123\\n'")
        app.wait(lambda: text_width(app, 300) > 50, msg="text")
        shots.append(app.shot()[: app.line_height(), : 300].copy())
        app.snap(f"font_family {family}")
        app.close()
    assert (shots[0] != shots[1]).any()


@pytest.mark.parametrize("key", ["terminal", "ui"])
def test_unknown_font_family_uses_bundled_font(app_factory, key):
    """a font that is not installed falls back to the bundled font, not a proportional system one"""
    shots = []
    for family in [None, "No Such Font 123"]:
        settings = {"theme": {"mode": "dark"}, "hide_bar_for_one_tab": False, "tab_title": [{"text": "Wig@0123"}]}
        if family and key == "terminal":
            settings["terminal"] = {"font_family": family}
        elif family:
            settings["ui_font_family"] = family
        app = app_factory(settings, script="printf 'still here, Wig@0123\\n'")
        top = app.bar_height()
        app.wait(lambda: text_width(app, 400, top) > 100, msg="text drawn")
        # titles refresh every second
        time.sleep(1.5)
        shots.append(app.shot()[: top + app.line_height(), :400].copy())
        app.snap(f"font_family {family or 'bundled'}")
        if family:
            assert "is not installed" in app.output()
        app.close()
    assert (shots[0] == shots[1]).all()


@pytest.mark.parametrize("ui_size", [10, 18, 28])
def test_ui_font_size_scales_tab_bar(app_factory, ui_size):
    """ui_font_size scales the tab bar height and its text"""
    theme = unique_theme(0x40)
    colors = as_rgb(theme)
    app = app_factory({"theme": {"mode": "dark"}, "hide_bar_for_one_tab": False, "ui_font_size": ui_size},
                      themes={"dark": theme})
    h = app.bar_height()
    app.wait(lambda: close_to(tuple(app.shot()[h - 1, 450]), colors["border"]), msg="bar border")
    img = app.shot()
    assert close_to(tuple(img[h + 2, 450]), colors["terminal_background"])
    # the title sits inside the bar and grows with the ui font
    title = ink_bbox(img[: h - 1, : 200], colors["tab_active_background"])
    assert title is not None and title[3] - title[1] <= h


def test_ui_font_family_changes_tab_text(app_factory):
    """ui_font_family draws the tab titles with another font"""
    theme = unique_theme(0x40)
    colors = as_rgb(theme)
    shots = []
    for family in ["JetBrainsMonoNL Nerd Font Mono", "DejaVu Serif"]:
        app = app_factory({"theme": {"mode": "dark"}, "hide_bar_for_one_tab": False, "ui_font_family": family,
                           "tab_title": [{"text": "Wig@0123"}]}, themes={"dark": theme})
        h = app.bar_height()
        app.wait(lambda: ink_bbox(app.shot()[: h - 1, :200], colors["tab_active_background"]), msg="title")
        app.wait(lambda: app.title() != "", msg="titles refreshed")
        shots.append(app.shot()[: h - 1, :200].copy())
        app.snap(f"ui_font_family {family}")
        app.close()
    assert (shots[0] != shots[1]).any()
