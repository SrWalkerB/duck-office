#!/usr/bin/env python3
"""Confere a página do site e a põe na beta mais nova, antes de publicar.

    gh api repos/SrWalkerB/duck-office/releases > releases.json
    python scripts/site.py releases.json site/index.html

Troca a versão (links, nomes e tamanhos dos arquivos) pela da release mais nova e
confere: todo arquivo para baixar existe na release, todo link interno e imagem
existe, e o ícone é o do programa. Uma release sem sufixo (a 1.0.0) não entra
sozinha, porque a página inteira diz que é beta: só avisa.
"""

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
REPO = "https://github.com/SrWalkerB/duck-office"


def update(html, releases):
    current = re.search(rf"{REPO}/releases/tag/v([\w.-]+)", html).group(1)
    published = {r["tag_name"]: r for r in releases if not r["draft"]}
    newest = max(published.values(), key=lambda r: r["published_at"])["tag_name"].removeprefix("v")
    if "-" in newest:
        html = html.replace(current, newest)
        current = newest
    elif newest != current:
        print(f"::warning::a {newest} já saiu e a página ainda fala da beta {current}: atualize o texto")

    if f"v{current}" not in published:
        sys.exit(f"não há release v{current}")
    sizes = {a["name"]: a["size"] for a in published[f"v{current}"]["assets"]}

    def size(m):
        mb = sizes[f"duck-sheets-{current}-{m[1]}"] / 1048576
        return f'<span data-tamanho="{m[1]}">' + f"{mb:.1f}".replace(".", ",") + " MB</span>"

    missing = [f for f in re.findall(rf'{REPO}/releases/download/[^/"]+/([^"]+)"', html) if f not in sizes]
    missing += [m for m in re.findall(r'data-tamanho="([^"]+)"', html) if f"duck-sheets-{current}-{m}" not in sizes]
    if missing:
        sys.exit(f"a release v{current} não tem: {', '.join(missing)}")
    return re.sub(r'<span data-tamanho="([^"]+)">[^<]*</span>', size, html), current


def check(html, folder):
    ids = set(re.findall(r'\bid="([^"]+)"', html))
    bad = []
    for link in re.findall(r'\b(?:href|src)="([^"]+)"', html):
        if link.startswith("#"):
            ok = link[1:] in ids
        elif link.startswith("https://"):
            ok = True
        else:
            ok = (folder / link).is_file()
        if not ok:
            bad.append(link)
    if bad:
        sys.exit(f"links quebrados na página: {', '.join(bad)}")
    icon = (ROOT / "apps/duck-sheets/assets/icon.svg").read_text(encoding="utf-8")
    if (folder / "img/icone.svg").read_text(encoding="utf-8") != icon:
        sys.exit("img/icone.svg não é o ícone do programa (apps/duck-sheets/assets/icon.svg)")


if __name__ == "__main__":
    releases, page = Path(sys.argv[1]), Path(sys.argv[2])
    html, version = update(page.read_text(encoding="utf-8"), json.loads(releases.read_text(encoding="utf-8")))
    check(html, page.parent)
    page.write_text(html, encoding="utf-8", newline="\n")
    print(f"página conferida, na versão {version}")
