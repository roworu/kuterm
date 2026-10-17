"""what only shows in pixels: box drawing chars stroked as paths, and kitty images sized in
cells or deleted by the program"""

import base64
import io

import numpy as np
from PIL import Image

from harness import bundled_theme, mask_bbox, near

FEATURE = "box drawing strokes and images in cells"

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


def kitty_png(width: int, height: int, keys: str = "") -> bytes:
    """a red png as one kitty graphics command with extra `keys`"""
    png = io.BytesIO()
    Image.new("RGB", (width, height), (255, 0, 0)).save(png, format="PNG")
    data = base64.b64encode(png.getvalue())
    return b"\x1b_Ga=T,f=100" + keys.encode() + b";" + data + b"\x1b\\"


def red_image(app):
    return mask_bbox(near(app.shot(), (255, 0, 0), 10))


def test_image_sized_in_cells_is_stretched_over_them(app_factory):
    """an image placed with c= and r= covers that many cells, whatever its own pixel size"""
    app = app_factory({"theme": {"mode": "dark"}},
                      script='step 1; printf "above\\n"; cat "$T2_TMP/image.kitty"; printf "\\nbelow\\n"; '
                             'step 2; printf "\\033_Ga=d,d=A\\033\\\\"; printf "deleted\\n"')
    app.file("image.kitty").write_bytes(kitty_png(10, 10, ",c=6,r=3"))
    app.step(1)
    box = app.wait(lambda: red_image(app), msg="red image")
    app.snap("10x10 png stretched over 6 by 3 cells")
    cells = app.cell_rect(0, 1, 6, 3)
    # stretching blurs the edges into the background, so a few pixels of slack
    assert all(abs(got - want) <= 5 for got, want in zip(box, cells)), (box, cells)
    # deleting the image with its data takes it off the screen
    app.step(2)
    app.wait(lambda: red_image(app) is None, msg="deleted image still drawn")
    app.snap("image deleted by the program")
