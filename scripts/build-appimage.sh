#!/usr/bin/env bash
# Builds target/appimage/linux-office-installer-suite-x86_64_v3.AppImage (and its .sha256).
#
# Used by CI and for local builds. The AppDir bundles no libraries: the binary only links
# core system libraries (glibc, fontconfig, freetype, xz …), which every desktop has.
# appimagetool and the static type2 runtime are pinned and checked by SHA-256, so the
# AppImage needs no libfuse2 on the user's system.
set -euo pipefail

BIN=linux-office-installer-suite
NAME=$BIN-x86_64_v3.AppImage

APPIMAGETOOL_URL=https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage
APPIMAGETOOL_SHA256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
RUNTIME_URL=https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64
RUNTIME_SHA256=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
OUT=$ROOT/target/appimage
TOOLS=$ROOT/target/appimage-tools
APPDIR=$OUT/AppDir

# Download $1 to $3 unless a copy with SHA-256 $2 is already there.
fetch() {
    local url=$1 sha=$2 dest=$3
    if [[ -f $dest ]] && echo "$sha  $dest" | sha256sum -c --quiet - 2>/dev/null; then
        return
    fi
    curl -fsSL --retry 3 -o "$dest.part" "$url"
    echo "$sha  $dest.part" | sha256sum -c --quiet -
    chmod +x "$dest.part"
    mv "$dest.part" "$dest"
}

cd "$ROOT"
cargo build --release --locked

rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" "$APPDIR/usr/share/icons" "$TOOLS"
install -m 755 "target/release/$BIN" "$APPDIR/usr/bin/$BIN"
cp -r assets/icons/hicolor "$APPDIR/usr/share/icons/"
cp "assets/icons/hicolor/256x256/apps/$BIN.png" "$APPDIR/$BIN.png"
ln -s "usr/bin/$BIN" "$APPDIR/AppRun"

cat > "$APPDIR/$BIN.desktop" <<EOF
[Desktop Entry]
Type=Application
Version=1.4
Name=Linux Office Installer Suite
GenericName=Office Installer
Comment=Install Microsoft 365 or Office 2024 with Wine
Exec=$BIN
Icon=$BIN
Terminal=false
StartupNotify=true
StartupWMClass=$BIN
Categories=Utility;
Keywords=Office;Wine;Microsoft;Installer;
EOF
desktop-file-validate "$APPDIR/$BIN.desktop"
cp "$APPDIR/$BIN.desktop" "$APPDIR/usr/share/applications/"

fetch "$APPIMAGETOOL_URL" "$APPIMAGETOOL_SHA256" "$TOOLS/appimagetool"
fetch "$RUNTIME_URL" "$RUNTIME_SHA256" "$TOOLS/runtime-x86_64"

# Extract-and-run: CI runners (and some desktops) have no FUSE for mounting appimagetool.
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 "$TOOLS/appimagetool" --no-appstream \
    --runtime-file "$TOOLS/runtime-x86_64" "$APPDIR" "$OUT/$NAME"
(cd "$OUT" && sha256sum "$NAME" > "$NAME.sha256")
echo "$OUT/$NAME"
