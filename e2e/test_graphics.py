"""kitty graphics protocol: a png sent by a program is drawn at the cursor and scrolls with text"""

import base64
import io

import numpy as np
from PIL import Image

from harness import App, bundled_theme, mask_bbox, near

FEATURE = "images"

DARK = bundled_theme(True)
RED = (255, 0, 0)


def kitty_png(width: int, height: int) -> bytes:
    """a red png as kitty graphics commands, sent in chunks like real programs do"""
    png = io.BytesIO()
    Image.new("RGB", (width, height), RED).save(png, format="PNG")
    data = base64.b64encode(png.getvalue())
    chunks = [data[i : i + 4096] for i in range(0, len(data), 4096)]
    out = b""
    for ix, chunk in enumerate(chunks):
        more = 1 if ix < len(chunks) - 1 else 0
        keys = f"a=T,f=100,m={more}" if ix == 0 else f"m={more}"
        out += b"\x1b_G" + keys.encode() + b";" + chunk + b"\x1b\\"
    return out


def red_box(app: App):
    return mask_bbox(near(app.shot(), RED, 10))


def test_image_is_drawn_and_scrolls(app_factory):
    """an image shows at the cursor, output below pushes it up with the text"""
    app = app_factory({"theme": {"mode": "dark"}},
                      script='step 1; printf "above\\n"; cat "$T2_TMP/image.kitty"; printf "\\nbelow\\n"; '
                             'step 2; i=0; while [ $i -lt 40 ]; do echo more_$i; i=$((i+1)); done')
    app.file("image.kitty").write_bytes(kitty_png(120, 60))
    app.step(1)
    box = app.wait(lambda: red_box(app), msg="red image")
    app.snap("png image drawn under the first line")
    x0, y0, x1, y1 = box
    assert abs((x1 - x0) - 120) <= 2 and abs((y1 - y0) - 60) <= 2, box
    # it starts on the second line, at the left edge
    lx0, ly0, _, ly1 = app.cell_rect(0, 1)
    assert abs(x0 - lx0) <= 2 and abs(y0 - ly0) <= 2, (box, (lx0, ly0))
    app.step(2)
    app.wait(lambda: red_box(app) is None or red_box(app)[1] < y0, msg="image moved up with the text")
