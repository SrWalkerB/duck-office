#!/usr/bin/env python3
"""Desenha as imagens dos instaladores em apps/duck-sheets/assets/instalador/.

Só precisa rodar de novo se o desenho mudar (os arquivos gerados ficam no git).
A fonte é a Ubuntu variável (licença UFL; no Ubuntu, fonts-ubuntu):

    pip install pillow
    python scripts/installer_art.py "Ubuntu[wdth,wght].ttf"
"""

import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont

from icon import BOTTOM, TOP, draw as draw_icon, gradient, mark

OUT = Path(__file__).resolve().parent.parent / "apps" / "duck-sheets" / "assets" / "instalador"

WHITE, YELLOW, GREEN = (255, 255, 255), (255, 203, 46), (14, 132, 88)
INK, MUTED, PALE = (18, 32, 58), (96, 110, 122), (206, 240, 224)
# O desenho do ícone, sem o quadrado.
MARK = (112, 104, 400, 408)


def lines(im, cell, color):
    layer = Image.new("RGBA", im.size, (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    w, h = im.size
    for x in range(cell[0], w, cell[0]):
        d.line((x, 0, x, h), fill=color)
    for y in range(cell[1], h, cell[1]):
        d.line((0, y, w, y), fill=color)
    return Image.alpha_composite(im.convert("RGBA"), layer)


def wizard(w, h, fonts):
    """A faixa da esquerda do instalador do Windows."""
    k = w / 164
    im = lines(gradient(w, h, TOP, (7, 78, 54)), (round(41 * k), round(18 * k)), (255, 255, 255, 18))
    m = mark(round(88 * k) * 4, MARK)
    m = m.resize((m.width // 4, m.height // 4), Image.LANCZOS)
    pos = (round(38 * k), round(72 * k))
    shadow = Image.new("RGBA", im.size, (0, 0, 0, 0))
    shadow.paste((2, 40, 26, 120), (pos[0], pos[1] + round(5 * k)), m)
    im = Image.alpha_composite(im, shadow.filter(ImageFilter.GaussianBlur(6 * k)))
    im.alpha_composite(m, pos)
    d = ImageDraw.Draw(im)
    d.text((w / 2, 200 * k), "Duck Sheets", font=fonts(700, 23 * k), fill=WHITE, anchor="mm")
    d.text((w / 2, 224 * k), "planilhas gigantes,", font=fonts(400, 11.5 * k), fill=PALE, anchor="mm")
    d.text((w / 2, 240 * k), "sem travar", font=fonts(400, 11.5 * k), fill=PALE, anchor="mm")
    return im.convert("RGB")


def dmg(s, fonts):
    """O fundo da janela do .dmg (660 × 420)."""
    w, h = 660 * s, 420 * s
    im = lines(gradient(w, h, (252, 255, 253), (228, 244, 236)), (66 * s, 28 * s), (*GREEN, 16))
    d = ImageDraw.Draw(im)
    d.text((330 * s, 62 * s), "Duck Sheets", font=fonts(700, 32 * s), fill=INK, anchor="mm")
    text = "Arraste o ícone para a pasta Aplicativos"
    d.text((330 * s, 98 * s), text, font=fonts(400, 15 * s), fill=MUTED, anchor="mm")
    # A seta entre os dois ícones (centros em x = 170 e 490, y = 225).
    y, x0, x1 = 222 * s, 268 * s, 392 * s
    d.rounded_rectangle((x0, y - 3 * s, x1 - 10 * s, y + 3 * s), radius=3 * s, fill=GREEN)
    d.polygon([(x1 - 18 * s, y - 15 * s), (x1 + 4 * s, y), (x1 - 18 * s, y + 15 * s)], fill=GREEN)
    note = "Na primeira vez: Ajustes do Sistema › Privacidade e Segurança › Abrir Mesmo Assim"
    d.text((330 * s, 376 * s), note, font=fonts(400, 12.5 * s), fill=MUTED, anchor="mm")
    return im.convert("RGB")


def main():
    def fonts(weight, px):
        f = ImageFont.truetype(sys.argv[1], round(px))
        axes = {a["name"]: a["default"] for a in f.get_variation_axes()}
        f.set_variation_by_axes([weight if a in (b"Weight", "Weight") else v for a, v in axes.items()])
        return f

    OUT.mkdir(parents=True, exist_ok=True)
    # Os tamanhos que o Inno Setup usa em cada escala da tela.
    for w, h in [(164, 314), (246, 459), (328, 604), (410, 797)]:
        wizard(w * 2, h * 2, fonts).resize((w, h), Image.LANCZOS).save(OUT / f"lateral-{w}.png", optimize=True)
    for s in (55, 83, 110, 138):
        draw_icon(s).save(OUT / f"topo-{s}.png", optimize=True)
    for scale, name in ((1, "dmg.png"), (2, "dmg@2x.png")):
        im = dmg(scale * 2, fonts).resize((660 * scale, 420 * scale), Image.LANCZOS)
        im.save(OUT / name, optimize=True, dpi=(72 * scale, 72 * scale))


if __name__ == "__main__":
    main()
