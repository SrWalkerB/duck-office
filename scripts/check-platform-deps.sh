#!/usr/bin/env bash
# Regra do ADR-003/ADR-007: nada em platform/ pode depender da suíte office.
# platform/ é uma biblioteca de uso geral e precisa poder virar um repositório
# próprio sem mudar uma linha.
#
# Para cada crate de platform/, lista todas as dependências (diretas e
# transitivas) e falha se aparecer um crate do workspace que não seja de platform/.
set -euo pipefail
cd "$(dirname "$0")/.."

metadata=$(cargo metadata --format-version 1 --no-deps)

# Crates do workspace: nome<TAB>caminho do manifesto (normalizado com "/").
members=$(printf '%s' "$metadata" | python3 -I -c '
import json, sys
m = json.load(sys.stdin)
for p in m["packages"]:
    print(p["name"] + "\t" + p["manifest_path"].replace("\\", "/"))
')

platform=$(printf '%s\n' "$members" | awk -F'\t' '$2 ~ /\/platform\// {print $1}')
others=$(printf '%s\n' "$members" | awk -F'\t' '$2 !~ /\/platform\// {print $1}')

fail=0
for crate in $platform; do
    deps=$(cargo tree -p "$crate" -e normal,build --prefix none --format '{p}' 2>/dev/null \
        | awk '{print $1}' | sort -u)
    for other in $others; do
        if printf '%s\n' "$deps" | grep -qx "$other"; then
            echo "ERRO: platform/$crate depende de '$other' (crate da suíte office)." >&2
            fail=1
        fi
    done
done

if [ "$fail" -ne 0 ]; then
    echo "platform/ precisa continuar independente (ver docs/ARCHITECTURE.md, ADR-007)." >&2
    exit 1
fi
echo "ok: platform/ não depende de nenhum crate da suíte ($(echo $platform | wc -w) crates verificados)."
