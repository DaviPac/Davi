#!/bin/sh
# Package an already-built `target/release/davi` for Linux:
#   dist/davi-<tag>-linux-x64.tar.gz   binary + install.sh (any distro)
#   dist/davi-<tag>-linux-x64.deb      Debian/Ubuntu (`sudo apt install ./…deb`)
#   dist/Davi-<tag>-x86_64.AppImage    single portable file
#
# Usage: packaging/linux/package.sh <tag>   (needs cargo-deb; downloads
# appimagetool unless APPIMAGETOOL points to it)
set -eu

TAG="$1"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HERE="$ROOT/packaging/linux"
BIN="$ROOT/target/release/davi"
DIST="$ROOT/dist"
NAME="davi-$TAG-linux-x64"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

[ -x "$BIN" ] || { echo "missing $BIN: run cargo build --release first" >&2; exit 1; }
mkdir -p "$DIST"

# -- tarball -------------------------------------------------------------------
mkdir "$WORK/$NAME"
cp "$BIN" "$HERE/install.sh" "$HERE/dev.davi.Davi.desktop" "$ROOT/README.md" "$WORK/$NAME/"
cp "$ROOT/assets/icon.png" "$WORK/$NAME/dev.davi.Davi.png"
cp -r "$ROOT/examples/sample-collection" "$WORK/$NAME/sample-collection"
tar -C "$WORK" -czf "$DIST/$NAME.tar.gz" "$NAME"

# -- .deb ----------------------------------------------------------------------
# Version from the tag (v1.2.3 -> 1.2.3); dev builds keep Cargo's version.
case "$TAG" in
    v[0-9]*) DEB_VERSION="--deb-version=${TAG#v}" ;;
    *) DEB_VERSION="" ;;
esac
(cd "$ROOT" && cargo deb -p davi-ui --no-build $DEB_VERSION -o "$DIST/$NAME.deb")

# -- AppImage ------------------------------------------------------------------
APPDIR="$WORK/Davi.AppDir"
install -Dm755 "$BIN" "$APPDIR/usr/bin/davi"
install -Dm644 "$HERE/dev.davi.Davi.desktop" "$APPDIR/usr/share/applications/dev.davi.Davi.desktop"
install -Dm644 "$HERE/dev.davi.Davi.metainfo.xml" "$APPDIR/usr/share/metainfo/dev.davi.Davi.appdata.xml"
install -Dm644 "$ROOT/assets/icon.png" "$APPDIR/usr/share/icons/hicolor/512x512/apps/dev.davi.Davi.png"
cp "$HERE/dev.davi.Davi.desktop" "$APPDIR/"
cp "$ROOT/assets/icon.png" "$APPDIR/dev.davi.Davi.png"
cat > "$APPDIR/AppRun" <<'APPRUN'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
exec "$HERE/usr/bin/davi" "$@"
APPRUN
chmod +x "$APPDIR/AppRun"

TOOL="${APPIMAGETOOL:-$WORK/appimagetool}"
if [ ! -x "$TOOL" ]; then
    curl -fsSL -o "$TOOL" \
        https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
    chmod +x "$TOOL"
fi
# Extract-and-run: CI runners have no FUSE.
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$TOOL" --no-appstream "$APPDIR" "$DIST/Davi-$TAG-x86_64.AppImage"

ls -lh "$DIST"
