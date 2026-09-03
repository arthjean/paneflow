#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"

ARCH="${ARCH:-x86_64}"
TARGET_TRIPLE="${TARGET:-}"

if [ "$#" -ge 1 ]; then
    VERSION="$1"
else
    VERSION="$(awk -F'"' '/^version = / { print $2; exit }' "$REPO_ROOT/Cargo.toml")"
fi
if [ -z "${VERSION:-}" ]; then
    echo "error: could not determine version (pass as arg or set in Cargo.toml)" >&2
    exit 1
fi

if [ -n "$TARGET_TRIPLE" ]; then
    BIN="$REPO_ROOT/target/$TARGET_TRIPLE/release/paneflow"
else
    BIN="$REPO_ROOT/target/release/paneflow"
fi

if [ ! -x "$BIN" ]; then
    echo "error: release binary not found at $BIN" >&2
    echo "hint:  run 'cargo build --release${TARGET_TRIPLE:+ --target $TARGET_TRIPLE} -p paneflow-app' first" >&2
    exit 1
fi

BUNDLE_DIR="$REPO_ROOT/target/bundle"
APP="$BUNDLE_DIR/paneflow.app"
TARBALL="$BUNDLE_DIR/paneflow-${VERSION}-${ARCH}.tar.gz"

rm -rf "$APP"
mkdir -p "$APP/bin" \
         "$APP/share/applications" \
         "$APP/share/metainfo"

install -m 755 "$BIN" "$APP/bin/paneflow"
install -m 644 "$REPO_ROOT/assets/paneflow.desktop" "$APP/share/applications/paneflow.desktop"
install -m 644 "$REPO_ROOT/assets/io.github.arthurdev44.paneflow.metainfo.xml" \
               "$APP/share/metainfo/io.github.arthurdev44.paneflow.metainfo.xml"

for size in 16 32 48 128 256 512; do
    dest="$APP/share/icons/hicolor/${size}x${size}/apps"
    mkdir -p "$dest"
    install -m 644 "$REPO_ROOT/assets/icons/paneflow-${size}.png" "$dest/paneflow.png"
done

install -m 644 "$REPO_ROOT/LICENSE"   "$APP/LICENSE"
install -m 644 "$REPO_ROOT/README.md" "$APP/README.md"
install -m 644 "$REPO_ROOT/native/libghostty/THIRD_PARTY_NOTICES.md" \
               "$APP/THIRD_PARTY_NOTICES.md"
install -m 755 "$SCRIPT_DIR/tarball-install.sh" "$APP/install.sh"

MTIME="${SOURCE_DATE_EPOCH:-$(date +%s)}"
( cd "$BUNDLE_DIR" \
  && tar \
       --sort=name \
       --owner=0 --group=0 --numeric-owner \
       --mtime="@$MTIME" \
       -cf - paneflow.app \
     | gzip -n -9 > "$TARBALL" )

echo "$TARBALL"
