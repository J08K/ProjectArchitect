"""Draw the app icon: assets/icon.svg, icon-<n>.png, icon.ico and icon-64.rgba (the window icon, raw RGBA so the
GUI needs no PNG decoder) from one geometry (python tools/icon.py).

A dark rounded tile with three roadmap bars (now / next / later) in the theme's blue, green and orange.
"""

from pathlib import Path

from PIL import Image, ImageDraw

OUT = Path(__file__).resolve().parent.parent / "assets"
TILE, RADIUS = "#1d1e22", 0.22
# (x, y, width, height, colour) on a unit square.
BARS = [
    (0.20, 0.24, 0.60, 0.13, "#7cb0ff"),
    (0.20, 0.435, 0.44, 0.13, "#3ecf8e"),
    (0.20, 0.63, 0.30, 0.13, "#f5a524"),
]


def svg() -> str:
    s = 256
    parts = [f'<rect width="{s}" height="{s}" rx="{RADIUS * s:g}" fill="{TILE}"/>']
    for x, y, w, h, c in BARS:
        parts.append(f'<rect x="{x * s:g}" y="{y * s:g}" width="{w * s:g}" height="{h * s:g}" rx="{h * s / 2:g}" fill="{c}"/>')
    body = "\n  ".join(parts)
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {s} {s}">\n  {body}\n</svg>\n'


def png(size: int) -> Image.Image:
    k = 4  # supersampling
    s = size * k
    im = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    d.rounded_rectangle((0, 0, s - 1, s - 1), radius=RADIUS * s, fill=TILE)
    for x, y, w, h, c in BARS:
        d.rounded_rectangle((x * s, y * s, (x + w) * s, (y + h) * s), radius=h * s / 2, fill=c)
    return im.resize((size, size), Image.LANCZOS)


if __name__ == "__main__":
    OUT.mkdir(exist_ok=True)
    (OUT / "icon.svg").write_text(svg(), encoding="utf-8")
    for n in (48, 128, 256):
        png(n).save(OUT / f"icon-{n}.png")
    (OUT / "icon-64.rgba").write_bytes(png(64).tobytes())
    png(256).save(OUT / "icon.ico", sizes=[(n, n) for n in (16, 24, 32, 48, 64, 128, 256)])
    print("wrote", ", ".join(sorted(p.name for p in OUT.iterdir())))
