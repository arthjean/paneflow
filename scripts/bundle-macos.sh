#!/usr/bin/env bash
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

[ -f "$BIN" ]              || die "release binary not found at $BIN (did you run 'cargo build --release --target $TRIPLE -p paneflow-app'?)"
[ -f "$INFO_PLIST_SRC" ]   || die "Info.plist template not found at $INFO_PLIST_SRC"
[ -f "$ICNS_SRC" ]         || die "PaneFlow.icns not found at $ICNS_SRC (scripts/build-icons.sh generates it from the macOS icon master)"

APP="$REPO_ROOT/dist/PaneFlow.app"
CONTENTS="$APP/Contents"
MACOS_DIR="$CONTENTS/MacOS"
RESOURCES_DIR="$CONTENTS/Resources"

rm -rf "$APP"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"

install -m 0755 "$BIN" "$MACOS_DIR/paneflow"
install -m 0644 "$ICNS_SRC" "$RESOURCES_DIR/PaneFlow.icns"

sed -e "s/@VERSION@/$VERSION/g" "$INFO_PLIST_SRC" > "$CONTENTS/Info.plist"
chmod 0644 "$CONTENTS/Info.plist"

echo "Built bundle: $APP ($ARCH, v$VERSION)"
