"""what only shows in pixels: box drawing chars stroked as paths"""

import numpy as np

from harness import bundled_theme

FEATURE = "box drawing strokes"

DARK = bundled_theme(True)
RED = DARK["ansi"][1]

# a red rising diagonal and a red falling one, side by side
DIAGONALS = "printf '\\033[31m\\342\\225\\261\\342\\225\\262\\033[0m\\n'\n"


def red_corners(img, rect) -> dict:
    """which corners of the cell at rect have red ink, a quarter of the cell each way"""
    x0, y0, x1, y1 = rect
    qx, qy = (x1 - x0) // 4, (y1 - y0) // 4
    red = abs(img.astype(int) - RED).max(axis=-1) <= 40
    corner = {
        "top left": red[y0 : y0 + qy, x0 : x0 + qx],
        "top right": red[y0 : y0 + qy, x1 - qx : x1],
        "bottom left": red[y1 - qy : y1, x0 : x0 + qx],
        "bottom right": red[y1 - qy : y1, x1 - qx : x1],
    }
    return {name: bool(np.any(part)) for name, part in corner.items()}


def test_builtin_diagonals_are_stroked_corner_to_corner(app_factory):
    """╱ and ╲ are stroked from corner to corner of their cell, not left out or filled"""
    app = app_factory({"theme": {"mode": "dark"}, "terminal": {"font": {"size": 40}}}, script=DIAGONALS)
    rising = lambda: red_corners(app.shot(), app.cell_rect(0, 0))
    app.wait(lambda: rising()["bottom left"] and rising()["top right"], msg="rising diagonal")
    img = app.wait_still("diagonals drawn")
    assert red_corners(img, app.cell_rect(0, 0)) == {
        "top left": False, "top right": True, "bottom left": True, "bottom right": False}
    assert red_corners(img, app.cell_rect(1, 0)) == {
        "top left": True, "top right": False, "bottom left": False, "bottom right": True}
    app.snap("builtin box drawing strokes the diagonals ╱╲")
