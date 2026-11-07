#!/bin/sh
# Build OmaMail and install it for the current user:
#   ~/.local/bin/omamail
#   ~/.local/share/applications/omamail.desktop
#   ~/.local/share/icons/hicolor/.../omamail.png
#
# Run from anywhere:
#   ./scripts/install.sh
#
# A Desktop OAuth client is compiled in only if
# crates/omamail-app/oauth.local.toml exists before the build.
# Copy oauth.local.toml.example and fill it in first if you want that.

set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
BIN_DIR=${OMAMAIL_BIN_DIR:-"$HOME/.local/bin"}
APP_DIR="$HOME/.local/share/applications"
ICON_DIR="$HOME/.local/share/icons/hicolor/512x512/apps"
SVG_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"

say() {
  printf '%s\n' "$*"
}

need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    say "missing '$1'. Install it, then run this again."
    exit 1
  fi
}

need cargo
need npm

if [ ! -f "$ROOT/crates/omamail-app/oauth.local.toml" ]; then
  say "note: no crates/omamail-app/oauth.local.toml"
  say "      this build will not include a Google client. Sign-in will need"
  say "      OMAMAIL_GOOGLE_CLIENT_ID or ~/.config/omamail/oauth.toml."
fi

say "Building the UI…"
if [ ! -d "$ROOT/ui/node_modules" ]; then
  npm --prefix "$ROOT/ui" install
fi
npm --prefix "$ROOT/ui" run build

say "Building the release binary…"
cargo build --release --manifest-path "$ROOT/Cargo.toml" -p omamail

SRC="$ROOT/target/release/omamail"
if [ ! -x "$SRC" ]; then
  say "build finished but $SRC is missing"
  exit 1
fi

mkdir -p "$BIN_DIR" "$APP_DIR" "$ICON_DIR" "$SVG_DIR"
install -m 755 "$SRC" "$BIN_DIR/omamail"

if command -v rsvg-convert >/dev/null 2>&1; then
  rsvg-convert -w 512 -h 512 "$ROOT/src-tauri/icons/icon.svg" -o "$ICON_DIR/omamail.png"
else
  install -m 644 "$ROOT/src-tauri/icons/icon.png" "$ICON_DIR/omamail.png"
fi
install -m 644 "$ROOT/src-tauri/icons/icon.svg" "$SVG_DIR/omamail.svg"
touch "$HOME/.local/share/icons/hicolor" 2>/dev/null || true

cat >"$APP_DIR/omamail.desktop" <<ENTRY
[Desktop Entry]
Name=OmaMail
Comment=Gmail, the Omarchy way
Exec=$BIN_DIR/omamail
Icon=omamail
StartupWMClass=omamail
Type=Application
Categories=Office;Email;
Keywords=mail;email;gmail;
Terminal=false
ENTRY

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$APP_DIR" >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q -t "$HOME/.local/share/icons/hicolor" >/dev/null 2>&1 || true
fi

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    say "note: $BIN_DIR is not on PATH. The launcher still works; the shell will not"
    say "      find 'omamail' until that directory is on PATH."
    ;;
esac

if command -v busctl >/dev/null 2>&1 \
  && ! busctl --user list 2>/dev/null | grep -qi secret; then
  say "note: no Secret Service found. Sign-in needs gnome-keyring, KeePassXC, or kwallet."
fi

say ""
say "Installed: $BIN_DIR/omamail"
say "Launcher: OmaMail"
say "Uninstall: $ROOT/scripts/uninstall.sh"
