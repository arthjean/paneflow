#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"

SRC_DIR="${PANEFLOW_ICNS_SOURCE_DIR:-}"
OUT="$REPO_ROOT/assets/PaneFlow.icns"

die() {
    echo "error: $*" >&2
    exit 1
}

[ -n "$SRC_DIR" ] || die "PANEFLOW_ICNS_SOURCE_DIR is required; run scripts/build-icons.sh"

for size in 16 32 128 256 512; do
    src="$SRC_DIR/paneflow-$size.png"
    [ -f "$src" ] || die "missing source PNG: $src"
done

resize_png() {
    local src="$1" dst="$2" size="$3"
    if command -v sips >/dev/null 2>&1; then
        sips -Z "$size" "$src" --out "$dst" >/dev/null
    elif command -v magick >/dev/null 2>&1; then
        magick "$src" -filter Lanczos -resize "${size}x${size}" "$dst"
    elif command -v convert >/dev/null 2>&1 \
        && convert -version 2>&1 | grep -qi "ImageMagick"; then
        convert "$src" -filter Lanczos -resize "${size}x${size}" "$dst"
    else
        die "need sips (macOS) or magick/convert (ImageMagick) to resize PNGs"
    fi
}

STAGING="$(mktemp -d)"
trap 'rm -rf "$STAGING"' EXIT
ICONSET="$STAGING/PaneFlow.iconset"
mkdir -p "$ICONSET"

SRC_64="$SRC_DIR/paneflow-64.png"
if [ ! -f "$SRC_64" ]; then
    SRC_64="$STAGING/paneflow-64.png"
    resize_png "$SRC_DIR/paneflow-128.png" "$SRC_64" 64
fi
SRC_1024="$SRC_DIR/paneflow-1024.png"
if [ ! -f "$SRC_1024" ]; then
    SRC_1024="$STAGING/paneflow-1024.png"
    resize_png "$SRC_DIR/paneflow-512.png" "$SRC_1024" 1024
fi

cp "$SRC_DIR/paneflow-16.png"           "$ICONSET/icon_16x16.png"
cp "$SRC_DIR/paneflow-32.png"           "$ICONSET/icon_16x16@2x.png"
cp "$SRC_DIR/paneflow-32.png"           "$ICONSET/icon_32x32.png"
cp "$SRC_64"                            "$ICONSET/icon_32x32@2x.png"
cp "$SRC_DIR/paneflow-128.png"          "$ICONSET/icon_128x128.png"
cp "$SRC_DIR/paneflow-256.png"          "$ICONSET/icon_128x128@2x.png"
cp "$SRC_DIR/paneflow-256.png"          "$ICONSET/icon_256x256.png"
cp "$SRC_DIR/paneflow-512.png"          "$ICONSET/icon_256x256@2x.png"
cp "$SRC_DIR/paneflow-512.png"          "$ICONSET/icon_512x512.png"
cp "$SRC_1024"                          "$ICONSET/icon_512x512@2x.png"

if command -v iconutil >/dev/null 2>&1; then
    echo "Packing via iconutil (macOS)..."
    iconutil -c icns "$ICONSET" -o "$OUT"
elif command -v png2icns >/dev/null 2>&1; then
    echo "Packing via png2icns (libicns)..."
    png2icns "$OUT" \
        "$ICONSET/icon_512x512@2x.png" \
        "$ICONSET/icon_512x512.png" \
        "$ICONSET/icon_256x256.png" \
        "$ICONSET/icon_128x128.png" \
        "$ICONSET/icon_32x32.png" \
        "$ICONSET/icon_16x16.png"
elif command -v icnsutil >/dev/null 2>&1; then
    echo "Packing via icnsutil..."
    icnsutil compose "$OUT" \
        "$ICONSET/icon_512x512@2x.png" \
        "$ICONSET/icon_512x512.png" \
        "$ICONSET/icon_256x256.png" \
        "$ICONSET/icon_128x128.png" \
        "$ICONSET/icon_32x32.png" \
        "$ICONSET/icon_16x16.png"
elif command -v python3 >/dev/null 2>&1; then
    echo "Packing via python3 inline packer (stdlib-only fallback)..."
    ICONSET="$ICONSET" OUT="$OUT" python3 - <<'PY'
import os, struct
iconset = os.environ["ICONSET"]
out = os.environ["OUT"]
mapping = [
    (b"icp4", "16x16"),
    (b"ic11", "16x16@2x"),
    (b"icp5", "32x32"),
    (b"ic12", "32x32@2x"),
    (b"ic07", "128x128"),
    (b"ic13", "128x128@2x"),
    (b"ic08", "256x256"),
    (b"ic14", "256x256@2x"),
    (b"ic09", "512x512"),
    (b"ic10", "512x512@2x"),
]
body = bytearray()
for ostype, name in mapping:
    with open(os.path.join(iconset, f"icon_{name}.png"), "rb") as f:
        data = f.read()
    body += ostype + struct.pack(">I", len(data) + 8) + data
total = b"icns" + struct.pack(">I", len(body) + 8) + bytes(body)
with open(out, "wb") as f:
    f.write(total)
PY
else
    die "no .icns packer found - install one of: iconutil (macOS built-in), \
png2icns (libicns package), icnsutil (pip install icnsutil), or python3"
fi

[ -s "$OUT" ] || die "produced empty $OUT"
if ! head -c 4 "$OUT" | grep -q icns; then
    die "$OUT is not a valid ICNS file (missing 'icns' magic header)"
fi

echo "Generated: $OUT ($(wc -c < "$OUT") bytes)"
