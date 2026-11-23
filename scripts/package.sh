#!/usr/bin/env bash
# Empacota o duck-sheets já compilado em release (cargo build --release -p duck-sheets)
# para o sistema atual, em dist/:
#   Windows: duck-sheets-<versão>-windows-x64.zip   (duck-sheets.exe)
#   Linux:   duck-sheets-<versão>-linux-x64.tar.gz  (binário + .desktop)
#   macOS:   duck-sheets-<versão>-macos.zip         (duck-sheets.app, sem assinatura)
set -euo pipefail

cd "$(dirname "$0")/.."
VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)"/\1/')
OS=$(uname -s)
ARCH=$(uname -m)
T="${CARGO_TARGET_DIR:-target}"
rm -rf dist && mkdir -p dist/stage

case "$OS" in
  MINGW* | MSYS* | CYGWIN* | Windows_NT)
    NAME="duck-sheets-$VERSION-windows-x64"
    cp "$T/release/duck-sheets.exe" LICENSE dist/stage/
    powershell -NoProfile -Command "Compress-Archive -Path dist/stage/* -DestinationPath dist/$NAME.zip -Force"
    ;;
  Linux)
    NAME="duck-sheets-$VERSION-linux-$ARCH"
    mkdir -p "dist/stage/$NAME"
    cp "$T/release/duck-sheets" LICENSE "dist/stage/$NAME/"
    cat > "dist/stage/$NAME/duck-sheets.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=duck-sheets
Comment=Planilhas .xlsx, .xlsm e .csv
Exec=duck-sheets %f
Terminal=false
Categories=Office;Spreadsheet;
MimeType=application/vnd.openxmlformats-officedocument.spreadsheetml.sheet;application/vnd.ms-excel.sheet.macroEnabled.12;text/csv;
EOF
    tar -C dist/stage -czf "dist/$NAME.tar.gz" "$NAME"
    ;;
  Darwin)
    NAME="duck-sheets-$VERSION-macos-$ARCH"
    APP="dist/stage/duck-sheets.app"
    mkdir -p "$APP/Contents/MacOS"
    cp "$T/release/duck-sheets" "$APP/Contents/MacOS/"
    cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>duck-sheets</string>
  <key>CFBundleDisplayName</key><string>duck-sheets</string>
  <key>CFBundleIdentifier</key><string>dev.duck-office.sheets</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
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
      <array><string>xlsx</string><string>xlsm</string><string>csv</string></array>
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
