#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"

NAME="PaneflowDockTile"
MIN_MACOS="13.0"
TRIPLE="aarch64-apple-darwin"
VERSION=""
OUT=""

usage() {
    cat >&2 <<EOF
Usage: $0 --version <ver> --out <PlugIns folder> [--target {aarch64-apple-darwin|x86_64-apple-darwin}]

Builds $NAME.docktileplugin, the Dock tile plug-in that keeps the chosen app
icon in the Dock while Paneflow is closed, into the given folder.
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
        --out)
            [ "$#" -ge 2 ] || die "--out requires an argument"
            OUT="$2"
            shift 2
            ;;
        --target)
            [ "$#" -ge 2 ] || die "--target requires an argument"
            TRIPLE="$2"
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
[ -n "$OUT" ]     || { usage; die "--out is required"; }

case "$TRIPLE" in
    aarch64-apple-darwin) ARCH="arm64" ;;
    x86_64-apple-darwin)  ARCH="x86_64" ;;
    *) die "--target must be aarch64-apple-darwin or x86_64-apple-darwin (got '$TRIPLE')" ;;
esac

[ "$(uname -s)" = "Darwin" ] || die "the plug-in links against AppKit, so it builds on macOS only"

TARGET_ROOT="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
LIB="$TARGET_ROOT/$TRIPLE/release/libpaneflow_dock_tile.a"
PLUGIN="$OUT/$NAME.docktileplugin"
ICONS="$REPO_ROOT/src-app/assets/icons/app"

MACOSX_DEPLOYMENT_TARGET="$MIN_MACOS" cargo build \
    --release --locked \
    -p paneflow-dock-tile --features bundle \
    --target "$TRIPLE" \
    --manifest-path "$REPO_ROOT/Cargo.toml"
[ -f "$LIB" ] || die "static library not found at $LIB"

rm -rf "$PLUGIN"
mkdir -p "$PLUGIN/Contents/MacOS" "$PLUGIN/Contents/Resources"

clang -bundle \
    -arch "$ARCH" \
    -mmacosx-version-min="$MIN_MACOS" \
    -Wl,-force_load,"$LIB" \
    -Wl,-dead_strip \
    -framework AppKit \
    -framework Foundation \
    -framework CoreFoundation \
    -lobjc \
    -o "$PLUGIN/Contents/MacOS/$NAME"

count=0
for png in "$ICONS"/*-macos-512.png; do
    [ -f "$png" ] || continue
    icon="$(basename "$png" -macos-512.png)"
    install -m 0644 "$png" "$PLUGIN/Contents/Resources/$icon.png"
    count=$((count + 1))
done
[ "$count" -gt 0 ] || die "no *-macos-512.png icon under $ICONS"

sed -e "s/@VERSION@/$VERSION/g" "$REPO_ROOT/assets/$NAME-Info.plist" > "$PLUGIN/Contents/Info.plist"
chmod 0644 "$PLUGIN/Contents/Info.plist"
plutil -lint "$PLUGIN/Contents/Info.plist" >/dev/null

file "$PLUGIN/Contents/MacOS/$NAME" | grep -q "bundle" \
    || die "$PLUGIN/Contents/MacOS/$NAME is not a Mach-O bundle"

echo "Built plug-in: $PLUGIN ($ARCH, v$VERSION, $count icons, $(du -sk "$PLUGIN" | cut -f1) KiB)"
