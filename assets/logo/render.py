"""assemble face textures into logo.png, icon_256.png and logo_macos.png"""

import math
from pathlib import Path

from PIL import Image, ImageDraw

HERE = Path(__file__).parent

FACES = {
    "top": (8.66, -5, 8.66, 5, 41.4, 78),
    "left": (8.66, 5, 0, 10, 41.4, 78),
    "right": (8.66, -5, 0, 10, 128, 128),
}

SCALE = 1.2

# apple's icon template: 1024 canvas with an 824 tile, macos 26 puts any other shape on a gray tile
MACOS_CANVAS = 1024
MACOS_TILE = 824
MACOS_CUBE = 800
# apple's tile corners are a superellipse, not a rounded rect
MACOS_TILE_EXPONENT = 5

def render_png(size: int) -> Image.Image:

    k = size / 256
    out = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    for name, (a, b, c, d, e, f) in FACES.items():
        tex = Image.open(HERE / "textures" / f"{name}.png").convert("RGBA")
        e, f = 128 + (e - 128) * SCALE, 128 + (f - 128) * SCALE
        a, b, c, d = a * SCALE, b * SCALE, c * SCALE, d * SCALE
        a, b, c, d, e, f = a * k, b * k, c * k, d * k, e * k, f * k
        det = a * d - b * c
        inv = (d / det, -c / det, (c * f - d * e) / det, -b / det, a / det, (b * e - a * f) / det)
        face = tex.transform((size, size), Image.AFFINE, inv, Image.NEAREST, fillcolor=(0, 0, 0, 0))
        out.alpha_composite(face)
    return out


def render_macos_png() -> Image.Image:

    # drawn larger and scaled down to smooth the tile edge
    big = MACOS_CANVAS * 4
    half = MACOS_TILE * 4 / 2
    n = MACOS_TILE_EXPONENT
    points = []
    for i in range(1440):
        t = 2 * math.pi * i / 1440
        cos, sin = math.cos(t), math.sin(t)
        x = half * math.copysign(abs(cos) ** (2 / n), cos)
        y = half * math.copysign(abs(sin) ** (2 / n), sin)
        points.append((big / 2 + x, big / 2 + y))
    mask = Image.new("L", (big, big), 0)
    ImageDraw.Draw(mask).polygon(points, fill=255)
    mask = mask.resize((MACOS_CANVAS, MACOS_CANVAS), Image.LANCZOS)

    # near white, slightly darker at the bottom like most macos app tiles
    gradient = Image.new("RGBA", (1, MACOS_CANVAS))
    for y in range(MACOS_CANVAS):
        v = round(255 - 22 * y / (MACOS_CANVAS - 1))
        gradient.putpixel((0, y), (v, v, v, 255))
    gradient = gradient.resize((MACOS_CANVAS, MACOS_CANVAS))

    out = Image.new("RGBA", (MACOS_CANVAS, MACOS_CANVAS), (0, 0, 0, 0))
    out.paste(gradient, (0, 0), mask)
    cube = render_png(MACOS_CUBE)
    offset = (MACOS_CANVAS - MACOS_CUBE) // 2
    out.alpha_composite(cube, (offset, offset))
    return out


def main() -> None:

    render_png(512).save(HERE / "logo.png")
    render_png(256).save(HERE / "icon_256.png")
    render_macos_png().save(HERE / "logo_macos.png")


if __name__ == "__main__":
    main()
