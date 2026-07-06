#!/usr/bin/env bash
# Package a signed+notarized .app bundle into a drag-to-Applications .dmg.
#
# US-016. Run this on a macOS runner AFTER `scripts/sign-macos.sh` and
# `scripts/notarize-macos.sh` have produced a signed, stapled bundle at
# `dist/PaneFlow.app`.
#
# Output filename convention:
#   dist/paneflow-<version>-<arch>-apple-darwin.dmg
#
# The `-apple-darwin` suffix is required: `update_checker.rs::pick_asset`
# (US-008) matches `.dmg` assets with `AssetFormat::target_qualifier() ==
# "-apple-darwin"`, so any other filename shape would silently prevent
# in-app update prompts from finding the asset.
#
# Implementation: follows the Zed bundle-mac pattern
# (zed-industries/zed `script/bundle-mac:260` upstream) - a single
# `hdiutil create -srcfolder ... -format UDZO` invocation that auto-sizes
# the image from the source folder and produces the final zlib-compressed
# image in one pass. Earlier revisions of this script ran a UDRW staging
# image + osascript Finder layout + UDZO convert pipeline. On macos-14
# hosted runners that pipeline failed with `hdiutil: create failed - No
# space left on device` despite tens of GB free on the host volume:
# `hdiutil create -size <fixed>m` pre-allocates a virtual device whose
# ENOSPC is internal to the image, not the runner. Letting hdiutil
# auto-size from -srcfolder bypasses the failure mode entirely.
#
# Trade-off: no custom Finder window layout (icon positions, background
# image). The DMG still presents the standard /Applications symlink for
# drag-to-install. Cosmetic layout can be re-introduced later by bringing
# back a UDRW staging stage WITHOUT a fixed -size flag - but the unstyled
# DMG ships and notarizes today, which is the priority.
#
# Usage:
#   scripts/create-dmg.sh --version 0.2.0 --arch aarch64
#   scripts/create-dmg.sh --version 0.2.0 --arch x86_64 --app path/to/X.app
set -euo pipefail

VERSION=""
ARCH=""
APP="dist/PaneFlow.app"

usage() {
    cat >&2 <<EOF
Usage: $0 --version <ver> --arch {aarch64|x86_64} [--app <path>]
EOF
}

die() {
    echo "error: $*" >&2
    exit 1
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version)  [ "$#" -ge 2 ] || die "--version requires an argument"; VERSION="$2"; shift 2 ;;
        --arch)     [ "$#" -ge 2 ] || die "--arch requires an argument";    ARCH="$2";    shift 2 ;;
        --app)      [ "$#" -ge 2 ] || die "--app requires an argument";     APP="$2";     shift 2 ;;
        -h|--help)  usage; exit 0 ;;
        *)          usage; die "unknown argument: $1" ;;
    esac
done

# --- Validate inputs ------------------------------------------------------
[ -n "$VERSION" ] || { usage; die "--version is required"; }
[ -n "$ARCH" ]    || { usage; die "--arch is required"; }
case "$ARCH" in
    aarch64|x86_64) ;;
    *) die "--arch must be 'aarch64' or 'x86_64' (got '$ARCH')" ;;
esac
[ -d "$APP" ] || die "bundle not found: $APP"

# hdiutil is macOS-native and has no portable equivalent. Fail loudly on
# other OSes rather than producing a broken DMG.
command -v hdiutil >/dev/null 2>&1 || die "hdiutil not found (this script only runs on macOS)"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"

VOLNAME="PaneFlow"
FINAL_DMG="$REPO_ROOT/dist/paneflow-${VERSION}-${ARCH}-apple-darwin.dmg"

# --- Prepare staging dir -------------------------------------------------
# Every DMG run starts from a clean staging directory so stale symlinks or
# leftover `.Trash-*` files can't leak into the image.
STAGING="$(mktemp -d)"
trap 'rm -rf "$STAGING"' EXIT

# The enclosed app bundle - `cp -R` preserves the embedded code signature
# and extended attributes (notarization ticket). `ditto` would also work
# but `cp -R` keeps the dependency surface minimal.
cp -R "$APP" "$STAGING/"
BUNDLE_NAME="$(basename "$APP")"

# Drag target - a symlink to /Applications gives the familiar macOS
# "drag here to install" UX. `ln -s /Applications` creates an absolute
# symlink that resolves against /Applications when the DMG is mounted
# on a user's Mac (the symlink target is a string, not a resolved inode).
ln -s /Applications "$STAGING/Applications"

# --- Build the final compressed DMG --------------------------------------
# Single-pass: -srcfolder + -format UDZO produces the final zlib-compressed
# read-only image directly. -ov overwrites any leftover .dmg from a
# previous run (idempotent re-runs).
#
# Flags deliberately omitted vs. earlier revisions:
#   -size <N>m       - caused ENOSPC inside hdiutil's virtual device
#                      when the fixed allocation could not fit content +
#                      filesystem overhead. Auto-sizing avoids this.
#   -fs HFS+         - UDZO defaults to HFS+ for backwards compatibility;
#                      passing it explicitly was redundant.
#   -fsargs '-c …'   - pre-grew the HFS+ catalog/attributes/extents
#                      B-trees, competing with payload for fixed-size
#                      budget. Default newfs_hfs sizing is correct.
mkdir -p "$(dirname "$FINAL_DMG")"

echo "Creating $FINAL_DMG (source: $(du -sh "$STAGING" | awk '{print $1}'))..."
hdiutil create \
    -volname "$VOLNAME" \
    -srcfolder "$STAGING" \
    -ov \
    -format UDZO \
    "$FINAL_DMG" >/dev/null

# --- Verify -------------------------------------------------------------
# `hdiutil verify` checksums the compressed image - catches truncation.
hdiutil verify "$FINAL_DMG" >/dev/null

# AC3: codesign inside the DMG must still verify. Mount the final image
# read-only and run codesign against the embedded .app - any signature
# drift (e.g., from a buggy hdiutil that rewrote extended attributes)
# would surface here, not at Gatekeeper time on a user's Mac.
VERIFY_MOUNT="$(hdiutil attach -nobrowse -readonly -noautoopen "$FINAL_DMG")"
VERIFY_DEV="$(echo "$VERIFY_MOUNT" | awk 'NR==1 {print $1}')"
VERIFY_PT="/Volumes/$VOLNAME"
if ! codesign --verify --deep --strict "$VERIFY_PT/$BUNDLE_NAME"; then
    hdiutil detach "$VERIFY_DEV" -force 2>/dev/null || true
    die "codesign verification failed on enclosed bundle"
fi
if ! xcrun stapler validate "$VERIFY_PT/$BUNDLE_NAME"; then
    hdiutil detach "$VERIFY_DEV" -force 2>/dev/null || true
    die "stapled notarization ticket validation failed on enclosed bundle"
fi
if ! spctl --assess --type exec --verbose "$VERIFY_PT/$BUNDLE_NAME"; then
    hdiutil detach "$VERIFY_DEV" -force 2>/dev/null || true
    die "Gatekeeper assessment failed on enclosed bundle"
fi
hdiutil detach "$VERIFY_DEV" -quiet 2>/dev/null \
    || hdiutil detach "$VERIFY_DEV" -force 2>/dev/null \
    || true

echo "Created: $FINAL_DMG ($(du -h "$FINAL_DMG" | awk '{print $1}'))"
