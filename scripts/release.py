#!/usr/bin/env python3
"""Textos de uma release, a partir dos pacotes prontos.

    python scripts/release.py manifesto PASTA VERSAO SAIDA.txt
    python scripts/release.py notas PASTA VERSAO REPO SAIDA.md
    python scripts/release.py somas PASTA SAIDA.txt

O manifesto é o que o programa lê para se atualizar (apps/duck-sheets/src/update.rs)
e precisa ser assinado. As notas vêm do CHANGELOG.md.
"""

import hashlib
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Fim do nome → (tipo no manifesto, sistema, descrição).
KINDS = [
    ("windows-x64-instalador.exe", "windows-instalador", "Windows", "Instalador (recomendado), atualiza sozinho"),
    ("windows-x64-portatil.zip", None, "Windows", "Portátil, sem instalar"),
    ("macos.dmg", None, "macOS", "Apple Silicon e Intel, atualiza sozinho"),
    ("macos-atualizacao.zip", "macos-app", None, None),
    ("linux-x64.AppImage", "linux-appimage", "Linux", "AppImage (qualquer distribuição), atualiza sozinho"),
    ("linux-x64.deb", None, "Linux", "Debian, Ubuntu e derivados"),
    ("linux-x64.rpm", None, "Linux", "Fedora, openSUSE e derivados"),
    ("linux-x64.tar.gz", None, "Linux", "Só o programa"),
]


def packages(folder, version):
    out = []
    for suffix, kind, system, text in KINDS:
        p = Path(folder) / f"duck-sheets-{version}-{suffix}"
        if not p.is_file():
            sys.exit(f"falta {p.name}")
        out.append((p, kind, system, text))
    return out


def sha256(p):
    h = hashlib.sha256()
    with p.open("rb") as f:
        while b := f.read(1 << 20):
            h.update(b)
    return h.hexdigest()


def write(path, text):
    Path(path).write_text(text, encoding="utf-8", newline="\n")


def manifest(folder, version, out):
    lines = ["duck-sheets 1", f"versao={version}"]
    for p, kind, _, _ in packages(folder, version):
        if kind:
            lines.append(f"arquivo={kind}|{p.name}|{p.stat().st_size}|{sha256(p)}")
    write(out, "\n".join(lines) + "\n")


def sums(folder, out):
    files = sorted(p for p in Path(folder).iterdir() if p.is_file() and p.name != Path(out).name)
    write(out, "".join(f"{sha256(p)}  {p.name}\n" for p in files))


def changes(version):
    text = (ROOT / "CHANGELOG.md").read_text(encoding="utf-8")
    m = re.search(rf"^## {re.escape(version)}\b.*?\n(.*?)(?=^## |\Z)", text, re.S | re.M)
    if not m:
        sys.exit(f"o CHANGELOG.md não tem a versão {version}")
    return m.group(1).strip()


def notes(folder, version, repo, out):
    base = f"https://github.com/{repo}/releases/download/v{version}"
    rows, last = [], None
    for p, _, system, text in packages(folder, version):
        if system:
            mb = f"{p.stat().st_size / 1048576:.1f}".replace(".", ",")
            rows.append(f"| {'**' + system + '**' if system != last else ''} | [{p.name}]({base}/{p.name}) | {text} | {mb} MB |")
            last = system
    install = (ROOT / "docs" / "INSTALAR.md").read_text(encoding="utf-8").split("\n", 1)[1].strip()
    beta = ""
    if "-" in version:
        beta = "> [!WARNING]\n> Versão beta: guarde uma cópia das planilhas importantes e [conte o que der errado](https://github.com/%s/issues).\n\n" % repo
    table = "\n".join(["| Sistema | Baixar | O que é | Tamanho |", "|---|---|---|---|", *rows])
    write(
        out,
        f"{beta}{changes(version)}\n\n## Baixar\n\n{table}\n\n"
        f"Os códigos SHA-256 estão em [SHA256SUMS.txt]({base}/SHA256SUMS.txt). "
        "O `macos-atualizacao.zip` é o que o programa usa para se atualizar.\n\n"
        f"<details>\n<summary><b>Como instalar</b></summary>\n\n{install}\n\n</details>\n",
    )


if __name__ == "__main__":
    cmd, *args = sys.argv[1:]
    {"manifesto": manifest, "notas": notes, "somas": sums}[cmd](*args)
