#!/usr/bin/env bash
# Empacota o duck-sheets já compilado em release (cargo build --release -p duck-sheets)
# para o sistema atual, em dist/:
#   Windows: duck-sheets-<versão>-windows-x64.zip   (duck-sheets.exe)
#   Linux:   duck-sheets-<versão>-linux-x64.tar.gz  (binário, .desktop e ícone)
#   macOS:   duck-sheets-<versão>-macos.zip         (duck-sheets.app, sem assinatura)
# A versão é a da etiqueta (v1.0.0-rc1 → 1.0.0-rc1) ou a do Cargo.toml.
set -euo pipefail

cd "$(dirname "$0")/.."
VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)"/\1/')
if [[ "${GITHUB_REF_NAME:-}" == v* ]]; then
  TAG="${GITHUB_REF_NAME#v}"
  if [[ "${TAG%%-*}" != "$VERSION" ]]; then
    echo "a etiqueta v$TAG não é da versão $VERSION do Cargo.toml" >&2
    exit 1
  fi
  VERSION="$TAG"
fi
# No Info.plist só cabem números.
NUMBER="${VERSION%%-*}"
ASSETS=apps/duck-sheets/assets
OS=$(uname -s)
ARCH=$(uname -m)
T="${CARGO_TARGET_DIR:-target}"
rm -rf dist && mkdir -p dist/stage

case "$OS" in
  MINGW* | MSYS* | CYGWIN* | Windows_NT)
    NAME="duck-sheets-$VERSION-windows-x64"
    # O runtime do C vai dentro do .exe (.cargo/config.toml): sem isso ele não
    # abre num Windows sem o "Visual C++ Redistributable".
    if grep -qai 'vcruntime140' "$T/release/duck-sheets.exe"; then
      echo "o .exe depende do VCRUNTIME140.dll: compile sem RUSTFLAGS" >&2
      exit 1
    fi
    cp "$T/release/duck-sheets.exe" LICENSE dist/stage/
    powershell -NoProfile -Command "Compress-Archive -Path dist/stage/* -DestinationPath dist/$NAME.zip -Force"
    ;;
  Linux)
    NAME="duck-sheets-$VERSION-linux-$ARCH"
    mkdir -p "dist/stage/$NAME"
    cp "$T/release/duck-sheets" LICENSE "dist/stage/$NAME/"
    cp "$ASSETS/icon-256.png" "dist/stage/$NAME/duck-sheets.png"
    cat > "dist/stage/$NAME/duck-sheets.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=duck-sheets
Comment=Planilhas .xlsx, .xlsm, .xlsb e .csv
Exec=duck-sheets %f
Icon=duck-sheets
StartupWMClass=duck-sheets
Terminal=false
Categories=Office;Spreadsheet;
MimeType=application/vnd.openxmlformats-officedocument.spreadsheetml.sheet;application/vnd.ms-excel.sheet.macroEnabled.12;application/vnd.ms-excel.sheet.binary.macroEnabled.12;text/csv;
EOF
    tar -C dist/stage -czf "dist/$NAME.tar.gz" "$NAME"
    ;;
  Darwin)
    NAME="duck-sheets-$VERSION-macos-$ARCH"
    APP="dist/stage/duck-sheets.app"
    mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
    cp "$T/release/duck-sheets" "$APP/Contents/MacOS/"
    cp "$ASSETS/icon.icns" "$APP/Contents/Resources/duck-sheets.icns"
    cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>duck-sheets</string>
  <key>CFBundleDisplayName</key><string>duck-sheets</string>
  <key>CFBundleIdentifier</key><string>dev.duck-office.sheets</string>
  <key>CFBundleIconFile</key><string>duck-sheets</string>
  <key>CFBundleVersion</key><string>$NUMBER</string>
  <key>CFBundleShortVersionString</key><string>$NUMBER</string>
  <key>CFBundleExecutable</key><string>duck-sheets</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>Planilha</string>
      <key>CFBundleTypeRole</key><string>Editor</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>CFBundleTypeExtensions</key>
      <array><string>xlsx</string><string>xlsm</string><string>xlsb</string><string>csv</string></array>
    </dict>
  </array>
</dict>
</plist>
EOF
    # O binário sai do linker com assinatura ad-hoc só dele; o bundle precisa
    # da sua, senão o macOS acusa "app danificado" ao abrir o .zip baixado.
    codesign --force -s - "$APP"
    cp LICENSE "dist/stage/"
    (cd dist/stage && zip -qry "../$NAME.zip" duck-sheets.app LICENSE)
    ;;
  *)
    echo "sistema não suportado: $OS" >&2
    exit 1
    ;;
esac
rm -rf dist/stage
ls -l dist
