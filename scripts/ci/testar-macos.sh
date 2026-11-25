#!/usr/bin/env bash
# Instala a partir do .dmg, abre, e (no ensaio) atualiza de ponta a ponta.
set -euo pipefail

DMG="pacote/duck-sheets-$VERSAO-macos.dmg"
ZIP="pacote/duck-sheets-$VERSAO-macos-atualizacao.zip"
confere() { # texto comando...
  local o=$1
  shift
  if "$@"; then echo "ok  $o"; else
    echo "falhou: $o"
    exit 1
  fi
}
versao() { [[ "$("$1" --version)" == "Duck Sheets $VERSAO" ]]; }

hdiutil verify -quiet "$DMG"
M=$(hdiutil attach -nobrowse -readonly "$DMG" | awk -F'\t' '/\/Volumes\//{print $3}')
echo "montado em $M"
confere "atalho para Aplicativos" test -L "$M/Aplicativos"
confere "fundo da janela" test -s "$M/.background.tiff" -o -d "$M/.background"
if [[ -f ensaio/chave.key ]]; then
  mkdir -p fotos
  (open "$M" && sleep 6 && screencapture -x fotos/dmg.png) || true
fi
D="$RUNNER_TEMP/Aplicativos"
mkdir -p "$D"
cp -R "$M/Duck Sheets.app" "$D/"
hdiutil detach -quiet "$M" || hdiutil detach -force -quiet "$M"
APP="$D/Duck Sheets.app"
EXE="$APP/Contents/MacOS/duck-sheets"
confere "assinatura do .app" codesign --verify --strict "$APP"
confere "Apple Silicon e Intel" lipo "$EXE" -verify_arch arm64 x86_64
confere "abre" versao "$EXE"
confere "ícone" test -s "$APP/Contents/Resources/duck-sheets.icns"
confere "licenças" test -s "$APP/Contents/Resources/LICENCAS-DE-TERCEIROS.txt"
confere "macOS 11" bash -c "otool -l '$EXE' | grep -A3 LC_BUILD_VERSION | grep -q 'minos 11.0'"

if [[ -f ensaio/chave.key ]]; then
  cargo install rsign2 --locked --version "$RSIGN" --quiet
  O="$RUNNER_TEMP/origem"
  mkdir -p "$O"
  cp "$ZIP" "$O/"
  nome=$(basename "$ZIP")
  soma=$(shasum -a 256 "$ZIP" | cut -d' ' -f1)
  for c in beta estavel; do
    printf 'duck-sheets 1\nversao=99.0.0-beta.1\narquivo=macos-app|%s|%s|%s\n' \
      "$nome" "$(stat -f %z "$ZIP")" "$soma" > "$O/$c.txt"
    rsign sign -W -s ensaio/chave.key -x "$O/$c.txt.minisig" "$O/$c.txt"
  done
  export DUCK_ATUALIZACAO="file://$O" DUCK_RECUPERACAO="$RUNNER_TEMP/estado"
  antes=$(stat -f %i "$APP")
  "$EXE" --atualizar
  confere "o .app foi trocado" test "$(stat -f %i "$APP")" != "$antes"
  confere "assinatura depois de atualizar" codesign --verify --strict "$APP"
  confere "abre depois de atualizar" versao "$EXE"
  confere "nada sobrou na pasta" test "$(ls -A "$D" | wc -l)" -eq 1
fi
