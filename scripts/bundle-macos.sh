#!/usr/bin/env bash
# Assemble a macOS .app bundle from a cargo-built release binary.
#
# Output layout (US-013):
#   dist/PaneFlow.app/Contents/
#     MacOS/paneflow                  (executable, chmod 755)
#     Info.plist                      (from assets/Info.plist, @VERSION@ substituted)
#     Resources/PaneFlow.icns         (from assets/PaneFlow.icns, produced by US-014)
#
# Usage:
#   scripts/bundle-macos.sh --version 0.2.0 --arch aarch64
#   scripts/bundle-macos.sh --version 0.2.0 --arch x86_64 \
#       --target-dir target/x86_64-apple-darwin/release
#
# Arguments:
#   --version <string>       Version to stamp into Info.plist (required).
#   --arch <aarch64|x86_64>  Target architecture (required).
#   --target-dir <path>      Directory containing the built `paneflow` binary.
#                            Defaults to target/<triple>/release where
#                            <triple> is aarch64-apple-darwin or
#                            x86_64-apple-darwin depending on --arch.
#
# Signing, notarization, and .dmg creation are intentionally out of scope -
# see US-015 (codesign + notarytool) and US-016 (hdiutil .dmg).
#
# Portable enough to run on Linux for structural verification (the shell
# logic doesn't depend on Darwin-only tools); the resulting bundle is only
# *useful* on macOS.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"

VERSION=""
ARCH=""
TARGET_DIR=""

usage() {
    cat >&2 <<EOF
Usage: $0 --version <ver> --arch {aarch64|x86_64} [--target-dir <path>]
EOF
}

die() {
    echo "error: $*" >&2
    exit 1
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version)
            [ "$#" -ge 2 ] || die "--version requires an argument"
            VERSION="$2"
            shift 2
            ;;
        --arch)
            [ "$#" -ge 2 ] || die "--arch requires an argument"
            ARCH="$2"
            shift 2
            ;;
        --target-dir)
            [ "$#" -ge 2 ] || die "--target-dir requires an argument"
            TARGET_DIR="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            usage
            die "unknown argument: $1"
            ;;
    esac
done

# --- Validate required inputs ---------------------------------------------
[ -n "$VERSION" ] || { usage; die "--version is required"; }
[ -n "$ARCH" ]    || { usage; die "--arch is required"; }

case "$ARCH" in
    aarch64) TRIPLE="aarch64-apple-darwin" ;;
    x86_64)  TRIPLE="x86_64-apple-darwin"  ;;
    *)       die "--arch must be 'aarch64' or 'x86_64' (got '$ARCH')" ;;
esac

if [ -z "$TARGET_DIR" ]; then
    TARGET_DIR="$REPO_ROOT/target/$TRIPLE/release"
fi

BIN="$TARGET_DIR/paneflow"
INFO_PLIST_SRC="$REPO_ROOT/assets/Info.plist"
ICNS_SRC="$REPO_ROOT/assets/PaneFlow.icns"

# Fail fast and loud (AC6) - every missing input names the path that wasn't
# found, so a failing CI log tells you exactly what to check.
[ -f "$BIN" ]              || die "release binary not found at $BIN (did you run 'cargo build --release --target $TRIPLE -p paneflow-app'?)"
[ -f "$INFO_PLIST_SRC" ]   || die "Info.plist template not found at $INFO_PLIST_SRC"
[ -f "$ICNS_SRC" ]         || die "PaneFlow.icns not found at $ICNS_SRC (scripts/build-icons.sh generates it from the macOS icon master)"

# --- Assemble bundle ------------------------------------------------------
APP="$REPO_ROOT/dist/PaneFlow.app"
CONTENTS="$APP/Contents"
MACOS_DIR="$CONTENTS/MacOS"
RESOURCES_DIR="$CONTENTS/Resources"

rm -rf "$APP"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"

install -m 0755 "$BIN" "$MACOS_DIR/paneflow"
install -m 0644 "$ICNS_SRC" "$RESOURCES_DIR/PaneFlow.icns"

# Substitute @VERSION@ in the Info.plist template. `sed -e` keeps the
# command portable between BSD sed (macOS) and GNU sed (Linux CI).
sed -e "s/@VERSION@/$VERSION/g" "$INFO_PLIST_SRC" > "$CONTENTS/Info.plist"
chmod 0644 "$CONTENTS/Info.plist"

echo "Built bundle: $APP ($ARCH, v$VERSION)"
