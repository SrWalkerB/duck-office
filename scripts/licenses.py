#!/usr/bin/env python3
"""Junta as licenças das bibliotecas que vão dentro do duck-sheets.

    python scripts/licenses.py SAIDA.txt ALVO [ALVO...]

Só entra o que o programa usa nesses alvos (nada de testes nem do que roda só
na compilação). Uma biblioteca sem licença declarada para o pacote.
"""

import json
import re
import subprocess
import sys
from pathlib import Path

NAMES = re.compile(r"^(licen[sc]e|copying|notice|unlicense|copyright)", re.I)


def metadata(target):
    cmd = ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", target]
    return json.loads(subprocess.run(cmd, check=True, capture_output=True, text=True, encoding="utf-8").stdout)


def used(meta):
    """Os pacotes que o duck-sheets usa, com as dependências delas."""
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    packages = {p["id"]: p for p in meta["packages"]}
    start = next(i for i, p in packages.items() if p["name"] == "duck-sheets")
    seen, queue = set(), [start]
    while queue:
        i = queue.pop()
        if i in seen:
            continue
        seen.add(i)
        for dep in nodes[i]["deps"]:
            if any(k["kind"] is None for k in dep["dep_kinds"]):
                queue.append(dep["pkg"])
    return [packages[i] for i in seen if packages[i]["source"]]


def texts(package):
    root = Path(package["manifest_path"]).parent
    # `c_src/mimalloc/LICENSE`: código em C que vai junto.
    found = [p for d in (root, *root.glob("*/"), *root.glob("c_src/*/")) if d.is_dir() for p in d.iterdir()]
    files = sorted({p for p in found if p.is_file() and NAMES.match(p.name) and p.stat().st_size < 200_000})
    out = []
    for p in files:
        if p.suffix.lower() in {".rs", ".py", ".toml", ".json", ".c", ".h", ".html"}:
            continue
        text = p.read_text(encoding="utf-8", errors="replace").replace("\r\n", "\n").strip()
        if len(text) > 40:
            out.append(text)
    return out


def main():
    out, targets = Path(sys.argv[1]), sys.argv[2:]
    packages = {}
    for t in targets:
        for p in used(metadata(t)):
            packages[(p["name"], p["version"])] = p
    missing = [f"{n} {v}" for (n, v), p in packages.items() if not p["license"] and not p["license_file"]]
    if missing:
        sys.exit(f"sem licença declarada: {', '.join(missing)}")
    groups = {}
    listing, bare = [], []
    for (name, version), p in sorted(packages.items()):
        listing.append(f"{name} {version}: {p['license'] or 'veja o texto'}")
        found = texts(p)
        if not found:
            bare.append(f"{name} {version} ({p['license']}) {p['repository'] or ''}".rstrip())
        for text in found:
            groups.setdefault(text, []).append(f"{name} {version}")
    bar = "=" * 78
    parts = [
        "Licenças das bibliotecas que vão dentro do Duck Sheets.",
        "O Duck Sheets em si é MIT (veja LICENSE).",
        "A biblioteca padrão do Rust é MIT ou Apache-2.0 (https://www.rust-lang.org).",
        "",
        *listing,
    ]
    if bare:
        parts += ["", bar, "Sem um arquivo de licença no pacote; vale a licença declarada:", "", *bare]
    for text, names in sorted(groups.items(), key=lambda g: g[1]):
        parts += ["", bar, *names, bar, "", text]
    out.write_text("\n".join(parts) + "\n", encoding="utf-8", newline="\n")
    print(f"{out}: {len(packages)} bibliotecas, {len(groups)} textos, {len(bare)} sem arquivo de licença")


if __name__ == "__main__":
    main()
