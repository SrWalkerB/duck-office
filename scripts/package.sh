#!/usr/bin/env bash
# Empacota o duck-sheets já compilado em release para o sistema atual, em dist/:
#   Windows: instalador (.exe) e versão portátil (.zip)
#   Linux:   .AppImage, .deb, .rpm e .tar.gz
#   macOS:   .dmg com o Duck Sheets.app (Apple Silicon e Intel; assinatura ad-hoc)
#            e o .zip que o programa baixa para se atualizar
#
# Antes: cargo build --release --locked -p duck-sheets
# (no macOS, para os dois alvos: aarch64-apple-darwin e x86_64-apple-darwin).
# Precisa de: Python 3; Inno Setup 6.6+ (Windows); dmgbuild (macOS);
# dpkg-deb, rpmbuild e strip (Linux).
set -euo pipefail

cd "$(dirname "$0")/.."
VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)"/\1/')
if [[ "${GITHUB_REF_TYPE:-}" == tag && "$GITHUB_REF_NAME" != "v$VERSION" ]]; then
  echo "a etiqueta $GITHUB_REF_NAME não é da versão $VERSION do Cargo.toml" >&2
  exit 1
fi
# Só os números (1.0.0-beta.1 → 1.0.0), onde não cabe mais que isso.
NUMBER="${VERSION%%-*}"
ASSETS=apps/duck-sheets/assets
LICENSES=LICENCAS-DE-TERCEIROS.txt
OS=$(uname -s)
T="${CARGO_TARGET_DIR:-target}"
PY=$(command -v python3 || command -v python)
rm -rf dist && mkdir -p dist/stage
cp LICENSE dist/stage/

case "$OS" in
  MINGW* | MSYS* | CYGWIN* | Windows_NT)
    EXE="$T/release/duck-sheets.exe"
    # O runtime do C vai dentro do .exe (.cargo/config.toml): sem isso ele não
    # abre num Windows sem o "Visual C++ Redistributable".
    if grep -qai 'vcruntime140' "$EXE"; then
      echo "o .exe depende do VCRUNTIME140.dll: compile sem RUSTFLAGS" >&2
      exit 1
    fi
    "$PY" scripts/licenses.py "dist/stage/$LICENSES" x86_64-pc-windows-msvc
    cp "$EXE" dist/stage/
    ZIP="duck-sheets-$VERSION-windows-x64-portatil.zip"
    powershell -NoProfile -Command "Compress-Archive -Path dist/stage/* -DestinationPath 'dist/$ZIP' -Force"
    cp "$ASSETS/icon.ico" dist/stage/
    ISCC="${ISCC:-$(command -v iscc || echo '/c/Program Files (x86)/Inno Setup 6/ISCC.exe')}"
    # Caminhos do Windows; o MSYS não mexe nas opções com barra.
    MSYS_NO_PATHCONV=1 "$ISCC" /Qp "/DVersion=$VERSION" "/DNumber=$NUMBER" \
      "/DStage=$(cygpath -wa dist/stage)" "/DArt=$(cygpath -wa $ASSETS/instalador)" \
      "/DOut=$(cygpath -wa dist)" "$(cygpath -wa scripts/package/windows.iss)"
    ;;
  Linux)
    "$PY" scripts/licenses.py "dist/stage/$LICENSES" x86_64-unknown-linux-gnu
    cp "$T/release/duck-sheets" dist/stage/
    bash scripts/package/linux.sh "$VERSION" "$NUMBER" dist/stage
    ;;
  Darwin)
    NAME="duck-sheets-$VERSION-macos"
    APP="dist/stage/Duck Sheets.app"
    "$PY" scripts/licenses.py "dist/stage/$LICENSES" aarch64-apple-darwin x86_64-apple-darwin
    mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
    lipo -create -output "$APP/Contents/MacOS/duck-sheets" \
      "$T/aarch64-apple-darwin/release/duck-sheets" "$T/x86_64-apple-darwin/release/duck-sheets"
    strip -x "$APP/Contents/MacOS/duck-sheets"
    cp "$ASSETS/icon.icns" "$APP/Contents/Resources/duck-sheets.icns"
    cp LICENSE "dist/stage/$LICENSES" "$APP/Contents/Resources/"
    cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Duck Sheets</string>
  <key>CFBundleDisplayName</key><string>Duck Sheets</string>
  <key>CFBundleIdentifier</key><string>dev.duck-office.sheets</string>
  <key>CFBundleIconFile</key><string>duck-sheets</string>
  <key>CFBundleVersion</key><string>$NUMBER</string>
  <key>CFBundleShortVersionString</key><string>$NUMBER</string>
  <key>CFBundleGetInfoString</key><string>Duck Sheets $VERSION</string>
  <key>CFBundleExecutable</key><string>duck-sheets</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleDevelopmentRegion</key><string>pt_BR</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <key>NSHumanReadableCopyright</key><string>Licença MIT · duck-office</string>
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
    plutil -lint -s "$APP/Contents/Info.plist"
    # O binário sai do linker com assinatura ad-hoc só dele; o bundle precisa
    # da sua, senão o macOS acusa "app danificado" ao abrir o que foi baixado.
    codesign --force -s - "$APP"
    codesign --verify --strict "$APP"
    # O que o programa baixa para se atualizar.
    ditto -c -k --keepParent "$APP" "dist/$NAME-atualizacao.zip"
    "$PY" -m dmgbuild -s scripts/package/dmg.py -D app="$APP" -D art="$ASSETS/instalador" \
      -D icon="$ASSETS/icon.icns" "Duck Sheets" "dist/$NAME.dmg"
    ;;
  *)
    echo "sistema não suportado: $OS" >&2
    exit 1
    ;;
esac
rm -rf dist/stage
ls -l dist
