"""Draw teehee's chosen pair-loop app icon.

One continuous loop stands for two devices held in the same connection. The
two endpoint nodes are the sender and receiver, one pale and one mint. The
loop is drawn large and downsampled so it stays clean at 20 px in the title
bar and at full app-icon size.
"""

from __future__ import annotations

import math
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "assets" / "icon.png"
SIZE = 1024
SCALE = 2
W = SIZE * SCALE


def sc(value: float) -> float:
    return value * SCALE


def rounded_mask(size: int, radius: int) -> Image.Image:
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        (0, 0, size - 1, size - 1), radius=radius, fill=255
    )
    return mask


def background() -> Image.Image:
    y = np.linspace(0, 1, W, dtype=np.float32)[:, None]
    top = np.array([6, 23, 21], dtype=np.float32)
    bottom = np.array([10, 40, 35], dtype=np.float32)
    grad = top * (1 - y[..., None]) + bottom * y[..., None]
    grad = np.repeat(grad, W, axis=1)
    image = Image.fromarray(np.uint8(np.clip(grad, 0, 255)), "RGB").convert("RGBA")

    glow = Image.new("RGBA", (W, W), (0, 0, 0, 0))
    ImageDraw.Draw(glow).ellipse(
        (sc(90), sc(60), sc(934), sc(904)),
        fill=(20, 184, 166, 32),
    )
    glow = glow.filter(ImageFilter.GaussianBlur(sc(130)))
    image.alpha_composite(glow)

    out = Image.new("RGBA", (W, W), (0, 0, 0, 0))
    out.paste(image, (0, 0), rounded_mask(W, sc(220)))
    return out


def round_line(
    draw: ImageDraw.ImageDraw,
    pts: list[tuple[float, float]],
    width: int,
    fill=255,
) -> None:
    draw.line(pts, fill=fill, width=width, joint="curve")
    radius = width // 2
    step = max(1, len(pts) // 80)
    for x, y in pts[::step]:
        draw.ellipse((x - radius, y - radius, x + radius, y + radius), fill=fill)
    x, y = pts[-1]
    draw.ellipse((x - radius, y - radius, x + radius, y + radius), fill=fill)


def add_stroke(
    image: Image.Image,
    pts: list[tuple[float, float]],
    width: float,
    *,
    glow: float = 0.0,
    color: tuple[int, int, int, int] = (240, 253, 250, 255),
) -> None:
    mask = Image.new("L", (W, W), 0)
    round_line(ImageDraw.Draw(mask), pts, sc(width))

    if glow:
        glow_mask = mask.filter(ImageFilter.GaussianBlur(sc(34)))
        layer = Image.new("RGBA", (W, W), (45, 212, 191, 0))
        layer.putalpha(glow_mask.point(lambda value: int(value * glow * 0.55)))
        image.alpha_composite(layer)

    alpha = mask.point(lambda value: int(value * (color[3] / 255)))
    layer = Image.new("RGBA", (W, W), (color[0], color[1], color[2], 0))
    layer.putalpha(alpha)
    image.alpha_composite(layer)


def add_dot(
    image: Image.Image,
    x: float,
    y: float,
    radius: float,
    color: tuple[int, int, int, int],
) -> None:
    mask = Image.new("L", (W, W), 0)
    ImageDraw.Draw(mask).ellipse(
        (sc(x - radius), sc(y - radius), sc(x + radius), sc(y + radius)), fill=255
    )
    glow_mask = mask.filter(ImageFilter.GaussianBlur(sc(28)))
    glow = Image.new("RGBA", (W, W), (45, 212, 191, 0))
    glow.putalpha(glow_mask.point(lambda value: int(value * 0.65)))
    image.alpha_composite(glow)

    layer = Image.new("RGBA", (W, W), color)
    layer.putalpha(mask)
    image.alpha_composite(layer)


def loop_points() -> list[tuple[float, float]]:
    points = []
    for i in range(1600):
        t = math.tau * i / 1600
        denom = 1 + math.sin(t) ** 2
        x = 512 + 330 * math.cos(t) / denom
        y = 530 + 245 * math.sin(t) * math.cos(t) / denom
        points.append((sc(x), sc(y)))
    return points


def main() -> None:
    image = background()
    add_stroke(
        image,
        loop_points(),
        78,
        glow=0.8,
        color=(240, 253, 250, 255),
    )
    add_dot(image, 182, 530, 37, (240, 253, 250, 255))
    add_dot(image, 842, 530, 37, (94, 234, 212, 255))

    OUT.parent.mkdir(parents=True, exist_ok=True)
    image.resize((SIZE, SIZE), Image.Resampling.LANCZOS).save(OUT, "PNG")
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
