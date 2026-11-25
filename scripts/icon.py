#!/usr/bin/env python3
"""Desenha o ícone do Duck Sheets e grava em apps/duck-sheets/assets/.

Só precisa rodar de novo se o desenho mudar (os arquivos gerados ficam no git):

    pip install pillow
    python scripts/icon.py
"""

import re
from pathlib import Path

from PIL import Image, ImageDraw

OUT = Path(__file__).resolve().parent.parent / "apps" / "duck-sheets" / "assets"

SIZE = 512
RADIUS = 116
# Fundo: verde, de cima para baixo.
TOP, BOTTOM = (21, 168, 107), (11, 107, 74)
# A letra D: a haste são três células, a do meio escolhida.
SHAPES = [
    ("rect", (112, 104, 92, 92, 20), "#FFFFFF"),
    ("rect", (112, 210, 92, 92, 20), "#FFCB2E"),
    ("rect", (112, 316, 92, 92, 20), "#FFFFFF"),
    (
        "path",
        "M218 124 Q218 104 238 104 H252 C342 104 400 168 400 256 C400 344 342 408 252 408 H238 Q218 408 218 388"
        " V336 Q218 316 238 316 H246 C286 316 308 290 308 256 C308 222 286 196 246 196 H238 Q218 196 218 176 Z",
        "#FFFFFF",
    ),
]


def hexa(c):
    return "#%02X%02X%02X" % c


def svg():
    body = []
    for kind, g, fill in SHAPES:
        if kind == "rect":
            x, y, w, h, r = g
            body.append(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{r}" fill="{fill}"/>')
        elif kind == "circle":
            body.append(f'<circle cx="{g[0]}" cy="{g[1]}" r="{g[2]}" fill="{fill}"/>')
        else:
            body.append(f'<path d="{g}" fill="{fill}"/>')
    lines = "\n  ".join(body)
    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {SIZE} {SIZE}">
  <defs><linearGradient id="fundo" x2="0" y2="1"><stop stop-color="{hexa(TOP)}"/><stop offset="1" stop-color="{hexa(BOTTOM)}"/></linearGradient></defs>
  <rect width="{SIZE}" height="{SIZE}" rx="{RADIUS}" fill="url(#fundo)"/>
  {lines}
</svg>
"""


def flatten(path):
    """Pontos de um caminho com comandos absolutos M, H, V, L, C, Q e Z."""
    out, cur = [], (0, 0)
    for cmd, args in re.findall(r"([MHVLCQZ])([^MHVLCQZ]*)", path):
        n = [float(x) for x in re.findall(r"-?[\d.]+", args)]
        if cmd == "H":
            cur = (n[0], cur[1])
        elif cmd == "V":
            cur = (cur[0], n[0])
        elif cmd in "ML":
            cur = (n[0], n[1])
        elif cmd in "CQ":
            p = [cur] + list(zip(n[::2], n[1::2]))
            for k in range(1, 65):
                t, q = k / 64, p
                while len(q) > 1:
                    q = [((1 - t) * a[0] + t * b[0], (1 - t) * a[1] + t * b[1]) for a, b in zip(q, q[1:])]
                out.append(q[0])
            cur = p[-1]
            continue
        if cmd != "Z":
            out.append(cur)
    return out


def gradient(w, h, a, b):
    col = Image.new("RGB", (1, h))
    for y in range(h):
        t = y / max(1, h - 1)
        col.putpixel((0, y), tuple(round(p + (q - p) * t) for p, q in zip(a, b)))
    return col.resize((w, h))


def mark(n, box=(0, 0, SIZE, SIZE)):
    """O desenho sem o fundo, com `box` (do quadro de 512) ocupando `n` de largura."""
    k = n / (box[2] - box[0])
    im = Image.new("RGBA", (n, round((box[3] - box[1]) * k)), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    at = lambda x, y: ((x - box[0]) * k, (y - box[1]) * k)
    for kind, g, fill in SHAPES:
        if kind == "rect":
            x, y, w, h, r = g
            d.rounded_rectangle((*at(x, y), *at(x + w, y + h)), radius=r * k, fill=fill)
        elif kind == "circle":
            x, y, r = g
            d.ellipse((*at(x - r, y - r), *at(x + r, y + r)), fill=fill)
        else:
            d.polygon([at(*p) for p in flatten(g)], fill=fill)
    return im


def draw(size):
    n = size * (8 if size <= 256 else 4)
    im = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    shape = Image.new("L", (n, n), 0)
    ImageDraw.Draw(shape).rounded_rectangle((0, 0, n - 1, n - 1), radius=RADIUS * n / SIZE, fill=255)
    im.paste(gradient(n, n, TOP, BOTTOM), (0, 0), shape)
    im.alpha_composite(mark(n))
    return im.resize((size, size), Image.LANCZOS)


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "icon.svg").write_text(svg(), encoding="utf-8", newline="\n")
    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    images = {s: draw(s) for s in sizes + [512, 1024]}
    images[256].save(OUT / "icon.ico", sizes=[(s, s) for s in sizes], append_images=[images[s] for s in sizes])
    images[1024].save(OUT / "icon.icns", append_images=[images[s] for s in (16, 32, 64, 128, 256, 512)])
    for s in (64, 256):
        images[s].save(OUT / f"icon-{s}.png", optimize=True)


if __name__ == "__main__":
    main()
