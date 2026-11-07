#!/bin/sh
# Remove the current user's OmaMail install:
#   ~/.local/bin/omamail
#   ~/.local/share/applications/omamail.desktop
#   ~/.local/share/icons/hicolor/.../omamail.png
#   ~/.local/share/icons/hicolor/.../omamail.svg
#
# Mail, tokens, and config are left in place. Pass --purge to remove those too:
#   ~/.config/omamail
#   ~/.local/share/omamail
# The keyring entry is named "omamail" and is not deleted here.
#
# Run from anywhere:
#   ./scripts/uninstall.sh
#   ./scripts/uninstall.sh --purge

set -eu

BIN_DIR=${OMAMAIL_BIN_DIR:-"$HOME/.local/bin"}
APP_DIR="$HOME/.local/share/applications"
ICON_DIR="$HOME/.local/share/icons/hicolor/512x512/apps"
SVG_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"
PURGE=0

if [ "${1:-}" = "--purge" ]; then
  PURGE=1
elif [ -n "${1:-}" ]; then
  printf '%s\n' "usage: $0 [--purge]" >&2
  exit 2
fi

say() {
  printf '%s\n' "$*"
}

remove() {
  if [ -e "$1" ] || [ -L "$1" ]; then
    rm -f "$1"
    say "removed $1"
  fi
}

remove "$BIN_DIR/omamail"
remove "$APP_DIR/omamail.desktop"
remove "$ICON_DIR/omamail.png"
remove "$SVG_DIR/omamail.svg"
touch "$HOME/.local/share/icons/hicolor" 2>/dev/null || true

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$APP_DIR" >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q -t "$HOME/.local/share/icons/hicolor" >/dev/null 2>&1 || true
fi

if [ "$PURGE" -eq 1 ]; then
  if [ -d "$HOME/.config/omamail" ]; then
    rm -rf "$HOME/.config/omamail"
    say "removed $HOME/.config/omamail"
  fi
  if [ -d "$HOME/.local/share/omamail" ]; then
    rm -rf "$HOME/.local/share/omamail"
    say "removed $HOME/.local/share/omamail"
  fi
  say "keyring items for service omamail were left alone"
else
  say "left mail and config in place. Use --purge to remove them too."
fi

say "uninstalled"
