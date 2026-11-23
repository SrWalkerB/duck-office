#!/usr/bin/env python3
"""duck-sheets × OnlyOffice: tempo até os dados aparecerem na tela.

Só biblioteca padrão. Funciona no macOS e no Windows.

    python3 scripts/bench/vs_onlyoffice.py                 # gera e mede as planilhas sintéticas
    python3 scripts/bench/vs_onlyoffice.py arquivo.xlsx    # mede um arquivo seu
    python3 scripts/bench/vs_onlyoffice.py --local         # as planilhas `regress` de bench.local.toml
    python3 scripts/bench/vs_onlyoffice.py --runs 3 --timeout 240

duck-sheets: roda `duck-sheets ARQUIVO --bench` e lê "primeira tela com dados",
"todas as abas" e o pico de memória.

OnlyOffice: não tem modo de medição. O script abre o arquivo no app, captura a
imagem a cada ~0,5 s e procura o salto de conteúdo em que a tela "Loading
spreadsheet" vira a grade com dados (a captura fica bem maior). As capturas
ficam em target/bench-onlyoffice/ para conferir a olho; confira sempre o quadro
indicado.

No macOS a captura é da tela toda: deixe a janela do OnlyOffice na frente e não
mexa no computador. No Windows é só da janela do OnlyOffice, mesmo com outra por
cima; não a minimize.

Com --local, cada planilha é copiada com um nome neutro (planilha-1.xlsm...) e as
cópias são apagadas no fim; o nome real não aparece em nada. As capturas mostram
o conteúdo: confira e apague a pasta.

Feche o OnlyOffice antes de começar: entre as rodadas o script pede para ele
fechar (como o "Sair" do menu) e nunca mata o processo.
"""

import argparse
import ctypes
import json
import os
import platform
import re
import shutil
import struct
import subprocess
import sys
import time
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TARGET = ROOT / "target"
OUT = TARGET / "bench-onlyoffice"
SYSTEM = platform.system()
EXE = ".exe" if SYSTEM == "Windows" else ""

OO_APP_MAC = "/Applications/ONLYOFFICE.app"
OO_EXE_WIN = Path(os.environ.get("ProgramFiles", r"C:\Program Files")) / "ONLYOFFICE" / "DesktopEditors" / "DesktopEditors.exe"


def run(cmd, **kw):
    return subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", errors="replace", **kw)


# ---------------------------------------------------------------- arquivos

def ensure_files(files):
    if files:
        return [Path(f).resolve() for f in files]
    bench = TARGET / "release" / f"duck-bench{EXE}"
    out = []
    for name, rows in [("synth-100k.xlsx", 100_000), ("synth-600k.xlsx", 600_000)]:
        p = TARGET / name
        if not p.exists():
            print(f"gerando {p.name} ({rows} linhas)...")
            subprocess.run([str(bench), "gen", str(p), "--rows", str(rows)], check=True)
        out.append(p)
    return out


def local_copies():
    """Cópias com nome neutro das planilhas `regress = "..."` de bench.local.toml (dados reais)."""
    cfg = (ROOT / "bench.local.toml").read_text(encoding="utf-8")
    OUT.mkdir(parents=True, exist_ok=True)
    out = []
    for i, src in enumerate(re.findall(r'^regress\s*=\s*"(.*)"', cfg, re.M), 1):
        dest = OUT / f"planilha-{i}{Path(src).suffix}"
        shutil.copyfile(src, dest)
        out.append(dest)
    return out


# ---------------------------------------------------------------- duck-sheets

def bench_duck(path, runs):
    exe = TARGET / "release" / f"duck-sheets{EXE}"
    res = []
    for i in range(runs + 1):  # a primeira rodada só aquece o cache de disco
        txt = run([str(exe), str(path), "--bench"]).stdout
        ms = lambda label: float(re.search(rf"{label}\s+([\d.]+) ms", txt).group(1))
        peak = re.search(r"(\d+) MB de pico", txt)
        r = {"primeira_tela_s": ms("primeira tela com dados") / 1000,
             "todas_as_abas_s": ms("todas as abas") / 1000,
             "pico_mem_mb": int(peak.group(1)) if peak else None}
        if i > 0:
            res.append(r)
        print(f"  duck-sheets {'aquecimento' if i == 0 else f'rodada {i}'}: "
              f"{r['primeira_tela_s']:.2f} s primeira tela, {r['todas_as_abas_s']:.2f} s tudo, "
              f"pico {r['pico_mem_mb']} MB")
    return res


# ---------------------------------------------------------------- OnlyOffice

def oo_running():
    if SYSTEM == "Darwin":
        return run(["pgrep", "-f", "ONLYOFFICE.app"]).returncode == 0
    out = run(["tasklist", "/fi", "IMAGENAME eq DesktopEditors.exe", "/fo", "csv", "/nh"]).stdout
    return "DesktopEditors" in out


def oo_open(path):
    if SYSTEM == "Darwin":
        subprocess.Popen(["open", "-a", OO_APP_MAC, str(path)])
    else:
        subprocess.Popen([str(OO_EXE_WIN), str(path)])


def oo_quit():
    """Pede para o OnlyOffice fechar, como o "Sair" do menu. Nunca mata o processo."""
    if SYSTEM == "Darwin":
        run(["osascript", "-e", 'tell application "ONLYOFFICE" to quit'])
    else:
        run(["powershell", "-NoProfile", "-Command",
             "Get-Process DesktopEditors -ErrorAction SilentlyContinue | ForEach-Object { $_.CloseMainWindow() | Out-Null }"])
    for _ in range(60):
        if not oo_running():
            return True
        time.sleep(1)
    return False


def oo_memory_mb():
    """Soma da memória residente dos processos do OnlyOffice (conta duas vezes o que é compartilhado)."""
    if SYSTEM == "Darwin":
        out = run(["ps", "-axo", "rss=,command="]).stdout
        return sum(int(l.split(None, 1)[0]) for l in out.splitlines() if "ONLYOFFICE.app" in l) // 1024
    out = run(["tasklist", "/fo", "csv", "/nh"]).stdout
    total = 0
    for line in out.splitlines():
        cols = [c.strip('"') for c in line.split('","')]
        if len(cols) >= 5 and ("DesktopEditors" in cols[0] or "editors" in cols[0].lower()):
            total += int(re.sub(r"\D", "", cols[4]) or 0)
    return total // 1024


def oo_window():
    """A maior janela visível dos processos do OnlyOffice (Windows)."""
    from ctypes import wintypes
    user, kernel = ctypes.windll.user32, ctypes.windll.kernel32
    kernel.OpenProcess.restype = wintypes.HANDLE
    best = (0, None)

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def each(hwnd, _):
        nonlocal best
        if not user.IsWindowVisible(hwnd) or user.IsIconic(hwnd):
            return True
        pid, name, n = wintypes.DWORD(), ctypes.create_unicode_buffer(1024), wintypes.DWORD(1024)
        user.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
        h = kernel.OpenProcess(0x1000, False, pid.value)  # só consulta
        if not h:
            return True
        kernel.QueryFullProcessImageNameW(wintypes.HANDLE(h), 0, name, ctypes.byref(n))
        kernel.CloseHandle(wintypes.HANDLE(h))
        if Path(name.value).name.lower() in ("desktopeditors.exe", "editors.exe"):
            r = wintypes.RECT()
            user.GetWindowRect(hwnd, ctypes.byref(r))
            area = (r.right - r.left) * (r.bottom - r.top)
            if area > best[0]:
                best = (area, hwnd)
        return True

    user.EnumWindows(each, 0)
    return best[1]


def capture_window(dest):
    """Grava em PNG a imagem de uma janela do OnlyOffice, mesmo coberta por outra (Windows)."""
    from ctypes import wintypes
    user, gdi = ctypes.windll.user32, ctypes.windll.gdi32
    H = ctypes.c_void_p
    for f in (user.GetDC, gdi.CreateCompatibleDC, gdi.CreateCompatibleBitmap, gdi.SelectObject):
        f.restype = H
    hwnd = oo_window()
    if hwnd is None:
        return False
    r = wintypes.RECT()
    user.GetWindowRect(hwnd, ctypes.byref(r))
    w, h = r.right - r.left, r.bottom - r.top
    screen = H(user.GetDC(None))
    dc = H(gdi.CreateCompatibleDC(screen))
    bmp = H(gdi.CreateCompatibleBitmap(screen, w, h))
    old = H(gdi.SelectObject(dc, bmp))
    ok = user.PrintWindow(hwnd, dc, 2)  # 2 = também o que a GPU desenhou
    gdi.SelectObject(dc, old)
    buf = ctypes.create_string_buffer(w * h * 4)
    info = struct.pack("<IiiHHIIiiII", 40, w, -h, 1, 32, 0, 0, 0, 0, 0, 0)
    gdi.GetDIBits(dc, bmp, 0, h, buf, info, 0)
    gdi.DeleteObject(bmp)
    gdi.DeleteDC(dc)
    user.ReleaseDC(None, screen)
    if not ok:
        return False
    px = bytearray(buf.raw)
    px[0::4], px[2::4] = px[2::4], px[0::4]  # BGRA → RGBA
    px[3::4] = b"\xff" * (w * h)
    rows = b"".join(b"\0" + px[y * w * 4:(y + 1) * w * 4] for y in range(h))

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    dest.write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
                     + chunk(b"IDAT", zlib.compress(rows, 1)) + chunk(b"IEND", b""))
    return True


def capture(dest):
    if SYSTEM == "Darwin":
        run(["screencapture", "-x", "-t", "jpg", str(dest)])
        return dest.exists()
    return capture_window(dest)


def find_data_frame(frames):
    """Primeiro salto grande de tamanho depois da tela de carregamento: é quando a grade com dados aparece."""
    sizes = [(t, p.stat().st_size, p) for t, p in frames]
    for (t0, s0, _), (t1, s1, p1) in zip(sizes, sizes[1:]):
        if t1 > 2.0 and s1 > s0 * 1.5:
            return t1, p1
    return None, None


def bench_onlyoffice(path, runs, timeout):
    res = []
    for i in range(runs + 1):
        if oo_running() and not oo_quit():
            sys.exit("O OnlyOffice não fechou. Feche-o (salvando o que precisar) e rode de novo.")
        time.sleep(2)
        folder = OUT / f"{path.stem}-{i}"
        shutil.rmtree(folder, ignore_errors=True)
        folder.mkdir(parents=True)
        frames, peak = [], 0
        t0 = time.perf_counter()
        oo_open(path)
        found_t = found_p = None
        while True:
            el = time.perf_counter() - t0
            f = folder / f"{int(el * 1000):06d}.{'jpg' if SYSTEM == 'Darwin' else 'png'}"
            if capture(f):
                frames.append((el, f))
            peak = max(peak, oo_memory_mb())
            found_t, found_p = find_data_frame(frames)
            # Depois que os dados aparecem, segue mais 10 s para pegar o pico de memória.
            if found_t is not None and el > found_t + 10:
                break
            if el > timeout:
                break
            time.sleep(0.3)
        oo_quit()
        r = {"dados_na_tela_s": round(found_t, 2) if found_t else None,
             "quadro": str(found_p) if found_p else None,
             "pico_mem_mb": peak}
        if i > 0:
            res.append(r)
        msg = f"{found_t:.1f} s" if found_t else f"não apareceu em {timeout} s"
        print(f"  OnlyOffice {'aquecimento' if i == 0 else f'rodada {i}'}: dados na tela em {msg}, "
              f"pico {peak} MB (confira: {found_p})")
    return res


# ---------------------------------------------------------------- main

def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("files", nargs="*")
    ap.add_argument("--runs", type=int, default=2, help="rodadas medidas, fora o aquecimento (padrão 2)")
    ap.add_argument("--timeout", type=int, default=240, help="limite por abertura no OnlyOffice, em s")
    ap.add_argument("--skip-onlyoffice", action="store_true")
    ap.add_argument("--local", action="store_true", help="mede cópias das planilhas `regress` de bench.local.toml")
    a = ap.parse_args()

    if SYSTEM not in ("Darwin", "Windows"):
        sys.exit("Por enquanto só macOS e Windows.")
    if not a.skip_onlyoffice and not (Path(OO_APP_MAC).exists() if SYSTEM == "Darwin" else OO_EXE_WIN.exists()):
        sys.exit("OnlyOffice não encontrado (use --skip-onlyoffice para medir só o duck-sheets).")
    print("compilando em release...")
    subprocess.run(["cargo", "build", "--release", "-p", "duck-sheets", "-p", "duck-bench"], cwd=ROOT, check=True)

    report = {"sistema": f"{SYSTEM} {platform.release()}", "cpu": platform.processor() or platform.machine(),
              "data": time.strftime("%Y-%m-%d"), "arquivos": {}}
    copies = local_copies() if a.local else []
    try:
        for f in copies or ensure_files(a.files):
            print(f"\n== {f.name} ({f.stat().st_size / 1e6:.0f} MB)")
            entry = {"duck_sheets": bench_duck(f, a.runs)}
            if not a.skip_onlyoffice:
                entry["onlyoffice"] = bench_onlyoffice(f, a.runs, a.timeout)
            report["arquivos"][f.name] = entry
    finally:
        for f in copies:
            f.unlink(missing_ok=True)

    OUT.mkdir(parents=True, exist_ok=True)
    out = OUT / f"resultado-{SYSTEM.lower()}.json"
    out.write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")
    print(f"\nresultado em {out}")


if __name__ == "__main__":
    main()
