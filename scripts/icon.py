#!/usr/bin/env python3
"""Desenha o ícone do duck-sheets e grava em apps/duck-sheets/assets/.

Só precisa rodar de novo se o desenho mudar (os arquivos gerados ficam no git):

    pip install pillow
    python scripts/icon.py
"""

from pathlib import Path

from PIL import Image, ImageDraw

OUT = Path(__file__).resolve().parent.parent / "apps" / "duck-sheets" / "assets"

# O mesmo desenho da página (32 × 32).
SVG = """<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">
  <rect width="32" height="32" rx="8" fill="#2F6BFF"/>
  <path d="M7 19c0-4.6 3.6-8.3 8.3-8.3 2.7 0 4.8 1.2 5.8 3.3l3.6.7c.8.1 1 1.1.3 1.5l-3.4 1.8c-.8 3.8-3.4 6.2-6.9 6.2C10.1 24.2 7 22.1 7 19z" fill="#fff"/>
  <circle cx="17.6" cy="14.5" r="1.35" fill="#2F6BFF"/>
  <path d="M21.6 15.5l3.5.6-3.4 1.8z" fill="#FFC21A"/>
</svg>
"""

BLUE, WHITE, YELLOW = "#2F6BFF", "#FFFFFF", "#FFC21A"

# O corpo em curvas de Bézier absolutas: (p0, p1, p2, p3); reta = dois pontos.
BODY = [
    ((7, 19), (7, 14.4), (10.6, 10.7), (15.3, 10.7)),
    ((15.3, 10.7), (18, 10.7), (20.1, 11.9), (21.1, 14)),
    ((21.1, 14), (24.7, 14.7)),
    ((24.7, 14.7), (25.5, 14.8), (25.7, 15.8), (25, 16.2)),
    ((25, 16.2), (21.6, 18)),
    ((21.6, 18), (20.8, 21.8), (18.2, 24.2), (14.7, 24.2)),
    ((14.7, 24.2), (10.1, 24.2), (7, 22.1), (7, 19)),
]
BEAK = [(21.6, 15.5), (25.1, 16.1), (21.7, 17.9)]


def points(curves):
    for c in curves:
        if len(c) == 2:
            yield from c
            continue
        for k in range(65):
            t = k / 64
            w = ((1 - t) ** 3, 3 * t * (1 - t) ** 2, 3 * t * t * (1 - t), t**3)
            yield tuple(sum(a * p[i] for a, p in zip(w, c)) for i in (0, 1))


def draw(size):
    over = 8 if size <= 256 else 4
    n = size * over
    k = n / 32
    im = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    d.rounded_rectangle((0, 0, n - 1, n - 1), radius=8 * k, fill=BLUE)
    d.polygon([(x * k, y * k) for x, y in points(BODY)], fill=WHITE)
    d.polygon([(x * k, y * k) for x, y in BEAK], fill=YELLOW)
    x, y, r = 17.6 * k, 14.5 * k, 1.35 * k
    d.ellipse((x - r, y - r, x + r, y + r), fill=BLUE)
    return im.resize((size, size), Image.LANCZOS)


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "icon.svg").write_text(SVG, encoding="utf-8", newline="\n")
    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    images = {s: draw(s) for s in sizes + [512, 1024]}
    images[256].save(
        OUT / "icon.ico", sizes=[(s, s) for s in sizes], append_images=[images[s] for s in sizes]
    )
    images[1024].save(OUT / "icon.icns", append_images=[images[s] for s in (16, 32, 64, 128, 256, 512)])
    for s in (64, 256):
        images[s].save(OUT / f"icon-{s}.png", optimize=True)


if __name__ == "__main__":
    main()
