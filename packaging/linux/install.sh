#!/bin/sh
# Install Davi for the current user (no root needed):
#   ./install.sh            install into ~/.local
#   ./install.sh --uninstall
# Set PREFIX to install elsewhere, e.g. `sudo PREFIX=/usr/local ./install.sh`.
set -eu

PREFIX="${PREFIX:-$HOME/.local}"
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="$PREFIX/bin/davi"
DESKTOP="$PREFIX/share/applications/dev.davi.Davi.desktop"
ICON="$PREFIX/share/icons/hicolor/512x512/apps/dev.davi.Davi.png"

refresh() {
    command -v update-desktop-database >/dev/null 2>&1 &&
        update-desktop-database -q "$PREFIX/share/applications" || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 &&
        gtk-update-icon-cache -q -t "$PREFIX/share/icons/hicolor" || true
}

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$BIN" "$DESKTOP" "$ICON"
    refresh
    echo "Davi uninstalled from $PREFIX"
    exit 0
fi

install -Dm755 "$HERE/davi" "$BIN"
install -Dm644 "$HERE/dev.davi.Davi.desktop" "$DESKTOP"
install -Dm644 "$HERE/dev.davi.Davi.png" "$ICON"
refresh

echo "Davi installed to $BIN"
case ":$PATH:" in
    *":$PREFIX/bin:"*) ;;
    *) echo "Note: $PREFIX/bin is not on your PATH; launch Davi from the app menu or add it." ;;
esac
