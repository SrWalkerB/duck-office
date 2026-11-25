#!/usr/bin/env bash
# Pacotes do Linux, a partir de dist/stage (chamado por scripts/package.sh):
#   .tar.gz, .deb, .rpm e .AppImage
set -euo pipefail

VERSION=$1
NUMBER=$2
STAGE=$3
ARCH=$(uname -m)
ASSETS=apps/duck-sheets/assets
NAME="duck-sheets-$VERSION-linux-x64"
DATE=$(date -u +%Y-%m-%d)
# 1.0.0-beta.1 vem antes de 1.0.0 para o dpkg e o rpm.
SORTED=${VERSION/-/\~}
SUMMARY="Editor de planilhas rápido, compatível com o Excel"

if [[ "$ARCH" != x86_64 ]]; then
  echo "só há pacotes para x86_64 (aqui: $ARCH)" >&2
  exit 1
fi

# --- os arquivos como ficam instalados --------------------------------------
ROOT="$STAGE/raiz"
mkdir -p "$ROOT/usr/bin" "$ROOT/usr/share/applications" "$ROOT/usr/share/metainfo" \
  "$ROOT/usr/share/doc/duck-sheets" "$ROOT/usr/share/icons/hicolor"/{64x64,256x256,scalable}/apps
install -m 755 "$STAGE/duck-sheets" "$ROOT/usr/bin/duck-sheets"
strip "$ROOT/usr/bin/duck-sheets"
install -m 644 "$ASSETS/icon-64.png" "$ROOT/usr/share/icons/hicolor/64x64/apps/duck-sheets.png"
install -m 644 "$ASSETS/icon-256.png" "$ROOT/usr/share/icons/hicolor/256x256/apps/duck-sheets.png"
install -m 644 "$ASSETS/icon.svg" "$ROOT/usr/share/icons/hicolor/scalable/apps/duck-sheets.svg"
install -m 644 "$STAGE/LICENSE" "$ROOT/usr/share/doc/duck-sheets/copyright"
install -m 644 "$STAGE/LICENCAS-DE-TERCEIROS.txt" "$ROOT/usr/share/doc/duck-sheets/"
cat > "$ROOT/usr/share/applications/duck-sheets.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Duck Sheets
GenericName=Spreadsheet
GenericName[pt_BR]=Planilha
Comment=Fast, Excel-compatible spreadsheets
Comment[pt_BR]=$SUMMARY
Exec=duck-sheets %f
Icon=duck-sheets
StartupWMClass=duck-sheets
Terminal=false
Categories=Office;Spreadsheet;
Keywords=xlsx;xlsm;xlsb;csv;excel;planilha;
MimeType=application/vnd.openxmlformats-officedocument.spreadsheetml.sheet;application/vnd.ms-excel.sheet.macroEnabled.12;application/vnd.ms-excel.sheet.binary.macroEnabled.12;text/csv;
EOF
cat > "$ROOT/usr/share/metainfo/duck-sheets.metainfo.xml" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<component type="desktop-application">
  <id>duck-sheets</id>
  <launchable type="desktop-id">duck-sheets.desktop</launchable>
  <name>Duck Sheets</name>
  <summary>Fast, Excel-compatible spreadsheets</summary>
  <summary xml:lang="pt_BR">$SUMMARY</summary>
  <metadata_license>CC0-1.0</metadata_license>
  <project_license>MIT</project_license>
  <developer id="duck-office"><name>duck-office</name></developer>
  <url type="homepage">https://github.com/SrWalkerB/duck-office</url>
  <description>
    <p>Opens, calculates and saves huge Excel workbooks without freezing.</p>
    <p xml:lang="pt_BR">Abre, calcula e salva planilhas do Excel gigantes sem travar.</p>
  </description>
  <content_rating type="oars-1.1"/>
  <releases><release version="$SORTED" date="$DATE"/></releases>
</component>
EOF
chmod 644 "$ROOT"/usr/share/{applications,metainfo}/*
SIZE=$(du -sk "$ROOT" | cut -f1)

# --- .tar.gz -----------------------------------------------------------------
mkdir -p "$STAGE/$NAME"
cp "$ROOT/usr/bin/duck-sheets" "$STAGE/LICENSE" "$STAGE/LICENCAS-DE-TERCEIROS.txt" "$STAGE/$NAME/"
cp "$ROOT/usr/share/applications/duck-sheets.desktop" "$STAGE/$NAME/"
cp "$ASSETS/icon-256.png" "$STAGE/$NAME/duck-sheets.png"
tar -C "$STAGE" --owner=0 --group=0 -czf "dist/$NAME.tar.gz" "$NAME"

# --- .deb --------------------------------------------------------------------
DEB="$STAGE/deb"
cp -r "$ROOT" "$DEB"
mkdir -p "$DEB/DEBIAN"
cat > "$DEB/DEBIAN/control" <<EOF
Package: duck-sheets
Version: $SORTED
Architecture: amd64
Maintainer: duck-office <noreply@users.noreply.github.com>
Installed-Size: $SIZE
Depends: libc6 (>= 2.35), libgcc-s1, libxkbcommon0, libxkbcommon-x11-0
Recommends: libvulkan1, mesa-vulkan-drivers, xdg-desktop-portal
Section: editors
Priority: optional
Homepage: https://github.com/SrWalkerB/duck-office
Description: $SUMMARY
 Abre, calcula, edita e salva planilhas .xlsx, .xlsm, .xlsb e .csv
 gigantes sem travar, com gráficos, tabelas dinâmicas e macros.
EOF
dpkg-deb --build --root-owner-group -Zxz "$DEB" "dist/$NAME.deb"

# --- .rpm --------------------------------------------------------------------
RPM="$STAGE/rpm"
mkdir -p "$RPM"/{BUILD,RPMS,SOURCES,SPECS,SRPMS}
cat > "$RPM/SPECS/duck-sheets.spec" <<EOF
Name: duck-sheets
Version: $SORTED
Release: 1
Summary: $SUMMARY
License: MIT
URL: https://github.com/SrWalkerB/duck-office
BuildArch: x86_64
AutoReqProv: no
Requires: glibc >= 2.35, libgcc, libxkbcommon, libxkbcommon-x11
Recommends: vulkan-loader, mesa-vulkan-drivers, xdg-desktop-portal
%global debug_package %{nil}
%global __strip /bin/true
%global _build_id_links none
%description
Abre, calcula, edita e salva planilhas .xlsx, .xlsm, .xlsb e .csv gigantes
sem travar, com gráficos, tabelas dinâmicas e macros.
%install
cp -a $(realpath "$ROOT")/. %{buildroot}/
%files
/usr/bin/duck-sheets
/usr/share/applications/duck-sheets.desktop
/usr/share/metainfo/duck-sheets.metainfo.xml
/usr/share/icons/hicolor/64x64/apps/duck-sheets.png
/usr/share/icons/hicolor/256x256/apps/duck-sheets.png
/usr/share/icons/hicolor/scalable/apps/duck-sheets.svg
%doc /usr/share/doc/duck-sheets
EOF
rpmbuild -bb --quiet --define "_topdir $(realpath "$RPM")" "$RPM/SPECS/duck-sheets.spec"
cp "$RPM"/RPMS/x86_64/*.rpm "dist/$NAME.rpm"

# --- .AppImage ---------------------------------------------------------------
# Ferramentas com versão e conteúdo fixos.
fetch() { # url sha256 destino
  curl -fsSL --retry 3 -o "$3" "$1"
  echo "$2  $3" | sha256sum -c --quiet
}
TOOLS="${APPIMAGE_TOOLS:-$STAGE/ferramentas}"
mkdir -p "$TOOLS"
fetch https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage \
  ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0 "$TOOLS/appimagetool"
fetch https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64 \
  2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d "$TOOLS/runtime"
chmod +x "$TOOLS/appimagetool"
DIR="$STAGE/duck-sheets.AppDir"
cp -r "$ROOT" "$DIR"
ln -s usr/bin/duck-sheets "$DIR/AppRun"
cp "$ROOT/usr/share/applications/duck-sheets.desktop" "$ASSETS/icon-256.png" "$DIR/"
mv "$DIR/icon-256.png" "$DIR/duck-sheets.png"
ln -s duck-sheets.png "$DIR/.DirIcon"
ARCH=x86_64 "$TOOLS/appimagetool" --appimage-extract-and-run --no-appstream \
  --runtime-file "$TOOLS/runtime" "$DIR" "dist/$NAME.AppImage"
