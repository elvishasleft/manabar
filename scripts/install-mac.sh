#!/usr/bin/env bash
# ManaBar terminal installer/updater for macOS (Apple silicon).
# Requires: gh CLI authenticated with access to the repo.
# Usage: gh api -H "Accept: application/vnd.github.raw" repos/arteeeezy/manabar/contents/scripts/install-mac.sh | bash
set -euo pipefail

REPO="arteeeezy/manabar"
TMP="$(mktemp -d)"
MNT="$TMP/mnt"
trap 'hdiutil detach "$MNT" -quiet 2>/dev/null || true; rm -rf "$TMP"' EXIT

echo "==> Downloading latest ManaBar release from $REPO ..."
gh release download --repo "$REPO" --pattern '*.dmg' --dir "$TMP"
DMG="$(find "$TMP" -name '*.dmg' | head -1)"
[ -n "$DMG" ] || { echo "No .dmg asset found in the latest release." >&2; exit 1; }

echo "==> Installing $(basename "$DMG") ..."
mkdir -p "$MNT"
hdiutil attach "$DMG" -nobrowse -quiet -mountpoint "$MNT"
pkill -x manabar 2>/dev/null || true
rm -rf /Applications/ManaBar.app
ditto "$MNT/ManaBar.app" /Applications/ManaBar.app
hdiutil detach "$MNT" -quiet
xattr -dr com.apple.quarantine /Applications/ManaBar.app 2>/dev/null || true

open /Applications/ManaBar.app
echo "==> ManaBar installed and launched — check the menu bar (top right)."
