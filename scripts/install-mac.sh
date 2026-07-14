#!/usr/bin/env bash
# ManaBar terminal installer/updater for macOS (Apple silicon).
# Usage: curl -fsSL https://raw.githubusercontent.com/elvishasleft/manabar/main/scripts/install-mac.sh | bash
set -euo pipefail

REPO="elvishasleft/manabar"
TMP="$(mktemp -d)"
MNT="$TMP/mnt"
trap 'hdiutil detach "$MNT" -quiet 2>/dev/null || true; rm -rf "$TMP"' EXIT

echo "==> Resolving latest ManaBar release from $REPO ..."
DMG_URL="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
  | grep -o '"browser_download_url": *"[^"]*\.dmg"' | head -1 | cut -d'"' -f4)"
[ -n "$DMG_URL" ] || { echo "No .dmg asset found in the latest release." >&2; exit 1; }

echo "==> Downloading $(basename "$DMG_URL") ..."
curl -fL --progress-bar -o "$TMP/ManaBar.dmg" "$DMG_URL"

echo "==> Installing ..."
mkdir -p "$MNT"
hdiutil attach "$TMP/ManaBar.dmg" -nobrowse -quiet -mountpoint "$MNT"
pkill -x manabar 2>/dev/null || true
rm -rf /Applications/ManaBar.app
ditto "$MNT/ManaBar.app" /Applications/ManaBar.app
hdiutil detach "$MNT" -quiet
xattr -dr com.apple.quarantine /Applications/ManaBar.app 2>/dev/null || true

open /Applications/ManaBar.app
echo "==> ManaBar installed and launched — check the menu bar (top right)."
