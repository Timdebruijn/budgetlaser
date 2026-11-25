#!/bin/zsh
# Signs and notarizes the macOS archive of a published release, then replaces it.
# The release workflow builds it unsigned; run this on a Mac with the Developer ID:
#   CODESIGN_IDENTITY="Developer ID Application: …" NOTARY_PROFILE=<notarytool profile> scripts/sign_macos.sh v0.4.1
# NO_UPLOAD=1 stops before the upload. A bare binary cannot be stapled, so Gatekeeper
# checks the notarization online the first time it runs.
set -euo pipefail
TAG=${1:?usage: scripts/sign_macos.sh <tag>}
IDENTITY=${CODESIGN_IDENTITY:?set CODESIGN_IDENTITY to your Developer ID Application identity}
PROFILE=${NOTARY_PROFILE:?set NOTARY_PROFILE to the notarytool keychain profile name}
REPO=Timdebruijn/budgetlaser
ARCHIVE=budgetlaser-macos.tar.gz
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

gh release download "$TAG" --repo "$REPO" --pattern "$ARCHIVE" --dir "$WORK"
tar xzf "$WORK/$ARCHIVE" -C "$WORK"
BIN="$WORK/budgetlaser-macos/budgetlaser"
codesign --force --sign "$IDENTITY" --options runtime --timestamp "$BIN"
codesign --verify --strict --verbose=2 "$BIN"
ditto -c -k "$BIN" "$WORK/notarize.zip"
# notarytool exits 0 on a rejected submission too, so check the status it prints.
OUT=$(xcrun notarytool submit "$WORK/notarize.zip" --keychain-profile "$PROFILE" --wait)
print -r -- "$OUT"
[[ "$OUT" == *"status: Accepted"* ]] || { echo "notarization not accepted"; exit 1; }
tar czf "$WORK/out.tar.gz" -C "$WORK" budgetlaser-macos
if [[ -n "${NO_UPLOAD:-}" ]]; then
    cp "$WORK/out.tar.gz" "./$ARCHIVE"
    echo "signed and notarized: ./$ARCHIVE (not uploaded)"
    exit 0
fi
mv "$WORK/out.tar.gz" "$WORK/$ARCHIVE"
gh release upload "$TAG" --repo "$REPO" --clobber "$WORK/$ARCHIVE"
echo "signed, notarized and uploaded: $ARCHIVE to $TAG"
