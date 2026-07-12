#!/usr/bin/env bash
# QuotaBar terminal installer/updater for macOS (Apple silicon).
# Requires: gh CLI authenticated with access to the repo.
# Usage: gh api -H "Accept: application/vnd.github.raw" repos/arteeeezy/quotabar/contents/scripts/install-mac.sh | bash
set -euo pipefail

REPO="arteeeezy/quotabar"
TMP="$(mktemp -d)"
MNT="$TMP/mnt"
trap 'hdiutil detach "$MNT" -quiet 2>/dev/null || true; rm -rf "$TMP"' EXIT

echo "==> Downloading latest QuotaBar release from $REPO ..."
gh release download --repo "$REPO" --pattern '*.dmg' --dir "$TMP"
DMG="$(find "$TMP" -name '*.dmg' | head -1)"
[ -n "$DMG" ] || { echo "No .dmg asset found in the latest release." >&2; exit 1; }

echo "==> Installing $(basename "$DMG") ..."
mkdir -p "$MNT"
hdiutil attach "$DMG" -nobrowse -quiet -mountpoint "$MNT"
pkill -x quotabar 2>/dev/null || true
rm -rf /Applications/QuotaBar.app
ditto "$MNT/QuotaBar.app" /Applications/QuotaBar.app
hdiutil detach "$MNT" -quiet
xattr -dr com.apple.quarantine /Applications/QuotaBar.app 2>/dev/null || true

open /Applications/QuotaBar.app
echo "==> QuotaBar installed and launched — check the menu bar (top right)."
