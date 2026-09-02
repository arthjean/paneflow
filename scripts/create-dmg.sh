#!/usr/bin/env bash
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

[ -n "$VERSION" ] || { usage; die "--version is required"; }
[ -n "$ARCH" ]    || { usage; die "--arch is required"; }
case "$ARCH" in
    aarch64|x86_64) ;;
    *) die "--arch must be 'aarch64' or 'x86_64' (got '$ARCH')" ;;
esac
[ -d "$APP" ] || die "bundle not found: $APP"

command -v hdiutil >/dev/null 2>&1 || die "hdiutil not found (this script only runs on macOS)"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"

VOLNAME="PaneFlow"
FINAL_DMG="$REPO_ROOT/dist/paneflow-${VERSION}-${ARCH}-apple-darwin.dmg"

STAGING="$(mktemp -d)"
trap 'rm -rf "$STAGING"' EXIT

cp -R "$APP" "$STAGING/"
BUNDLE_NAME="$(basename "$APP")"

ln -s /Applications "$STAGING/Applications"

mkdir -p "$(dirname "$FINAL_DMG")"

echo "Creating $FINAL_DMG (source: $(du -sh "$STAGING" | awk '{print $1}'))..."
hdiutil create \
    -volname "$VOLNAME" \
    -srcfolder "$STAGING" \
    -ov \
    -format UDZO \
    "$FINAL_DMG" >/dev/null

hdiutil verify "$FINAL_DMG" >/dev/null

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
