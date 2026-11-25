#!/usr/bin/env bash
# Instala o pacote numa distribuição limpa (dentro de um contêiner) e abre.
set -euo pipefail

P="pacote/duck-sheets-$VERSAO-linux-x64"
confere() { # texto comando...
  local o=$1
  shift
  if "$@"; then echo "ok  $o"; else
    echo "falhou: $o"
    exit 1
  fi
}
versao() { [[ "$("$@" --version)" == "Duck Sheets $VERSAO" ]]; }

if command -v apt-get >/dev/null; then
  apt-get update -q
  DEBIAN_FRONTEND=noninteractive apt-get install -qy --no-install-recommends "./$P.deb" curl ca-certificates >/dev/null
else
  dnf install -qy --setopt=install_weak_deps=False "./$P.rpm" curl tar gzip
fi
confere "o pacote instalado abre" versao duck-sheets
for f in applications/duck-sheets.desktop metainfo/duck-sheets.metainfo.xml \
  icons/hicolor/256x256/apps/duck-sheets.png icons/hicolor/scalable/apps/duck-sheets.svg \
  doc/duck-sheets/LICENCAS-DE-TERCEIROS.txt; do
  confere "/usr/share/$f" test -s "/usr/share/$f"
done
confere "nenhuma biblioteca faltando" bash -c '! ldd /usr/bin/duck-sheets | grep "not found"'

mkdir -p /tmp/x && tar -C /tmp/x -xzf "$P.tar.gz"
confere "o .tar.gz abre" versao "/tmp/x/$(basename "$P")/duck-sheets"

cp "$P.AppImage" /tmp/Duck.AppImage
chmod +x /tmp/Duck.AppImage
app() { APPIMAGE=/tmp/Duck.AppImage APPIMAGE_EXTRACT_AND_RUN=1 /tmp/Duck.AppImage "$@"; }
confere "o AppImage abre" versao app

# --- atualização de ponta a ponta (só no ensaio) -----------------------------
if [[ -f ensaio/chave.key ]]; then
  if ! command -v minisign >/dev/null; then
    if command -v apt-get >/dev/null; then apt-get install -qy minisign >/dev/null; else dnf install -qy minisign; fi
  fi
  O=/tmp/origem
  mkdir -p $O
  nome=$(basename "$P.AppImage")
  # A versão nova é outro arquivo, para dar para ver a troca.
  cp "$P.AppImage" "$O/$nome"
  printf 'novo' >> "$O/$nome"
  soma=$(sha256sum "$O/$nome" | cut -d' ' -f1)
  for c in beta estavel; do
    printf 'duck-sheets 1\nversao=99.0.0-beta.1\narquivo=linux-appimage|%s|%s|%s\n' \
      "$nome" "$(stat -c %s "$O/$nome")" "$soma" > $O/$c.txt
    minisign -S -W -s ensaio/chave.key -x $O/$c.txt.minisig -m $O/$c.txt >/dev/null
  done
  export DUCK_ATUALIZACAO="file://$O" DUCK_RECUPERACAO=/tmp/estado
  cp $O/beta.txt /tmp/beta.txt
  for c in beta estavel; do sed -i 's/99\.0\.0/99.0.1/' $O/$c.txt; done
  confere "manifesto adulterado é recusado" bash -c '! APPIMAGE=/tmp/Duck.AppImage APPIMAGE_EXTRACT_AND_RUN=1 /tmp/Duck.AppImage --atualizar'
  for c in beta estavel; do cp /tmp/beta.txt $O/$c.txt; done
  app --atualizar
  confere "o AppImage foi trocado" test "$(sha256sum /tmp/Duck.AppImage | cut -d' ' -f1)" = "$soma"
  confere "abre depois de atualizar" versao app
  # O pacote do sistema só avisa.
  confere "o .deb ou .rpm só avisa" bash -c 'duck-sheets --atualizar | grep -q "instalar à mão"'
fi
