"""cursor painting on screen: every cursor_shape, program-set shapes, hidden, unfocused look, colors, thickness"""

import subprocess

import pytest

from harness import BACKEND, App, bundled_theme, near, mask_bbox, session_env, swaymsg, xdo, wait_until

FEATURE = "cursor"

SHAPES = ["block", "bar", "underline", "hollow"]
# DECSCUSR codes programs send, to the shape drawn
PROGRAM_SHAPES = {2: "block", 4: "underline", 6: "bar"}


def footprint(app: App, theme: dict, img=None) -> tuple | None:
    """(w, h, pixels) of cursor colored pixels around the parked cell (0, 0)"""
    img = app.shot() if img is None else img
    x0, y0, x1, y1 = app.cell_rect(0, 0)
    region = img[max(y0 - 2, 0) : y1 + 2, max(x0 - 2, 0) : x1 + 2]
    mask = near(region, theme["cursor"], 3)
    box = mask_bbox(mask)
    if box is None:
        return None
    return box[2] - box[0], box[3] - box[1], int(mask.sum())


def matches(app: App, shape: str, fp: tuple | None) -> bool:
    if fp is None:
        return shape == "hidden"
    w, h, n = fp
    cw, lh = round(app.cell_width()), app.line_height()
    full_w, full_h = abs(w - cw) <= 1, abs(h - lh) <= 1
    return {
        "block": full_w and full_h and n >= 0.9 * w * h,
        "bar": w == 2 and full_h,
        "underline": full_w and h == 2,
        "hollow": full_w and full_h and n <= 2 * (w + h),
        "hidden": False,
    }[shape]


def wait_shape(app: App, theme: dict, shape: str) -> None:
    app.wait(lambda: matches(app, shape, footprint(app, theme)),
             msg=f"{shape} cursor, footprint {footprint(app, theme)}")


@pytest.mark.parametrize("mode", ["dark", "light"])
@pytest.mark.parametrize("shape", SHAPES)
def test_cursor_shape_setting(app_factory, shape, mode):
    """terminal.cursor_shape draws that shape in the theme cursor color"""
    app = app_factory({"theme": {"mode": mode}, "terminal": {"cursor_shape": shape, "font_size": 24}}, script="")
    wait_shape(app, bundled_theme(mode == "dark"), shape)


@pytest.mark.parametrize("code", list(PROGRAM_SHAPES))
def test_program_cursor_shape_overrides_setting(app_factory, code):
    """a program can change the shape with DECSCUSR, a reset goes back to the setting"""
    app = app_factory({"theme": {"mode": "dark"}, "terminal": {"cursor_shape": "hollow", "font_size": 24}},
                      script=f"step 1; printf '\\033[{code} q'; step 2; printf '\\033[0 q'")
    theme = bundled_theme(True)
    wait_shape(app, theme, "hollow")
    app.snap("setting: hollow")
    app.step(1)
    wait_shape(app, theme, PROGRAM_SHAPES[code])
    app.snap(f"program sent DECSCUSR {code}")
    app.step(2)
    wait_shape(app, theme, "hollow")


def test_hidden_cursor_is_not_painted(app_factory):
    """a program hiding the cursor removes it from the screen"""
    app = app_factory({"theme": {"mode": "dark"}, "terminal": {"cursor_shape": "block", "font_size": 24}},
                      script="step 1; printf '\\033[?25l'; step 2; printf '\\033[?25h'")
    theme = bundled_theme(True)
    wait_shape(app, theme, "block")
    app.step(1)
    app.wait(lambda: footprint(app, theme) is None, msg="no cursor")
    app.snap("cursor hidden")
    app.step(2)
    wait_shape(app, theme, "block")


def test_cursor_moves_with_output(app_factory):
    """the cursor follows printed text"""
    app = app_factory({"theme": {"mode": "dark"}, "terminal": {"cursor_shape": "block", "font_size": 24}},
                      script="printf 'abc'")
    theme = bundled_theme(True)
    x0, y0, x1, y1 = app.cell_rect(3, 0)

    def at_col_3():
        img = app.shot()
        return near(img[y0 + 2 : y1 - 2, x0 + 2 : x1 - 2], theme["cursor"], 3).mean() > 0.5

    app.wait(at_col_3, msg="block cursor after abc")


@pytest.fixture
def thief():
    """something that can take the focus from the app: another x window, or on wayland a
    second output whose empty workspace gets focused while the app stays visible"""
    if BACKEND == "wayland":
        # made once and kept for the session, so every test does not add another output
        names = {o["name"] for o in swaymsg("-t", "get_outputs")}
        if "HEADLESS-2" not in names:
            swaymsg("create_output")
            wait_until(lambda: "HEADLESS-2" in {o["name"] for o in swaymsg("-t", "get_outputs")},
                       msg="second output")

        def steal() -> None:
            swaymsg("focus output HEADLESS-2")

        yield steal
        return
    proc = subprocess.Popen(["xmessage", "-geometry", "300x100+0+0", "focus thief"], env=session_env(),
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    # xmessage sets no _NET_WM_PID, so it is found by its class
    wid = wait_until(lambda: xdo("search", "--onlyvisible", "--classname", "xmessage", check=False).split(),
                     msg="thief window")[0]

    def steal() -> None:
        xdo("windowactivate", "--sync", wid, check=False)

    yield steal
    proc.kill()
    proc.wait()


@pytest.mark.parametrize("shape", ["block", "bar", "underline"])
def test_cursor_hollow_while_unfocused(app_factory, thief, shape):
    """any cursor is drawn hollow while the window is not focused, and comes back on focus"""
    app = app_factory({"theme": {"mode": "dark"}, "terminal": {"cursor_shape": shape, "font_size": 24}}, script="")
    theme = bundled_theme(True)
    wait_shape(app, theme, shape)
    app.snap("focused")
    thief()
    wait_shape(app, theme, "hollow")
    app.snap("another window focused")
    app.focus()
    wait_shape(app, theme, shape)


MAGENTA = (255, 0, 255)
GREEN = (0, 255, 0)


@pytest.mark.parametrize("shape, thickness", [("bar", 5), ("underline", 4)])
def test_cursor_color_and_thickness(app_factory, shape, thickness):
    """cursor.color paints the cursor in that color, cursor.thickness sets bar width and underline height"""
    app = app_factory({"theme": {"mode": "dark"}, "terminal": {
        "cursor_shape": shape, "font_size": 24,
        "cursor": {"color": "#ff00ff", "thickness": thickness}}}, script="")
    cw, lh = round(app.cell_width()), app.line_height()

    def sized():
        fp = footprint(app, {"cursor": MAGENTA})
        if fp is None:
            return False
        w, h, _ = fp
        return (w, abs(h - lh) <= 1) == (thickness, True) if shape == "bar" else \
            (abs(w - cw) <= 1, h) == (True, thickness)

    app.wait(sized, msg=f"{thickness}px magenta {shape}, footprint {footprint(app, {'cursor': MAGENTA})}")
    app.snap(f"magenta {shape} {thickness}px thick")


def test_cursor_text_color(app_factory):
    """cursor.text_color colors the character under a block cursor"""
    app = app_factory({"theme": {"mode": "dark"}, "terminal": {
        "cursor_shape": "block", "font_size": 24,
        "cursor": {"text_color": "#00ff00"}}}, script="printf 'W\\r'")
    x0, y0, x1, y1 = app.cell_rect(0, 0)
    app.wait(lambda: near(app.shot()[y0:y1, x0:x1], GREEN, 40).sum() > 10, msg="green W under the block")
    app.snap("green W under the block cursor")


@pytest.mark.parametrize("unfocused", ["same", "none"])
def test_unfocused_cursor_setting(app_factory, thief, unfocused):
    """cursor.unfocused keeps the focused shape or hides the cursor while not focused"""
    app = app_factory({"theme": {"mode": "dark"}, "terminal": {
        "cursor_shape": "bar", "font_size": 24, "cursor": {"unfocused": unfocused}}}, script="")
    theme = bundled_theme(True)
    wait_shape(app, theme, "bar")
    thief()
    wait_shape(app, theme, "bar" if unfocused == "same" else "hidden")
    app.snap(f"unfocused: {unfocused}")
    app.focus()
    wait_shape(app, theme, "bar")
