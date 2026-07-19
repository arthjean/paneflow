#!/usr/bin/env bash
# Generate assets/PaneFlow.icns from a prepared PNG iconset.
#
# US-014. The `.icns` file is consumed by `scripts/bundle-macos.sh`
# (US-013), which copies it into `Contents/Resources/` of the .app bundle.
#
# build-icons.sh passes a prepared plated source directory through
# PANEFLOW_ICNS_SOURCE_DIR. Requiring it prevents a direct invocation from
# accidentally rebuilding the Apple bundle from Linux's transparent assets.
#
# Tool selection cascades at runtime (first available wins):
#   1. iconutil        (macOS, Apple-blessed, best quality)
#   2. png2icns        (Linux, from libicns)
#   3. icnsutil        (Linux, Python package)
#   4. python3 + inline packer   (guaranteed fallback - stdlib only)
#
# AC3 of the PRD lists `png2icns` or `icnsutil` as the Linux fallback.
# The Python stdlib fallback is added last because (a) Python3 is
# near-universal on dev machines and CI runners, and (b) the ICNS wire
# format is simple enough to emit directly in ~30 lines of Python (file
# header + typed sub-image chunks). ImageMagick was considered but its
# ICNS coder isn't compiled into standard packages.
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

# --- Validate sources -----------------------------------------------------
# The Apple iconset spec needs 16, 32, 64, 128, 256, 512, and 1024 px. The
# five baseline sizes are required here; 64 and 1024 are derived only when the
# caller did not prepare them explicitly.
for size in 16 32 128 256 512; do
    src="$SRC_DIR/paneflow-$size.png"
    [ -f "$src" ] || die "missing source PNG: $src"
done

# --- Resize helper --------------------------------------------------------
# Prefer `sips` on macOS (native, no extra deps) and `magick` elsewhere.
# Lanczos is the best general-purpose resampling filter for both up- and
# downscaling of RGBA icons.
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

# --- Build iconset staging dir --------------------------------------------
STAGING="$(mktemp -d)"
trap 'rm -rf "$STAGING"' EXIT
ICONSET="$STAGING/PaneFlow.iconset"
mkdir -p "$ICONSET"

# Prefer explicit sources prepared by build-icons.sh. Keep a derivation
# fallback for callers that provide a valid iconset without 64/1024 sources.
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

# Apple iconset filename convention (iconutil(1)):
#   icon_<base>[@2x].png, where base ∈ {16x16, 32x32, 128x128, 256x256, 512x512}
#   and the logical pixel count is base, with @2x doubling it.
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

# --- Pack .icns -----------------------------------------------------------
if command -v iconutil >/dev/null 2>&1; then
    echo "Packing via iconutil (macOS)..."
    iconutil -c icns "$ICONSET" -o "$OUT"
elif command -v png2icns >/dev/null 2>&1; then
    echo "Packing via png2icns (libicns)..."
    # png2icns takes the largest sizes (512, 256, 128, 32, 16) and auto-picks
    # which ones to embed. Feed the staging copies to be explicit.
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
    # The ICNS wire format is trivial: 8-byte file header (b'icns' + u32
    # total length, both big-endian) followed by typed sub-image chunks
    # (4-byte OSType + u32 chunk length including header + payload).
    # Apple's per-resolution OSTypes for PNG-encoded sub-images:
    #   icp4=16, ic11=32 (16@2x), icp5=32, ic12=64 (32@2x),
    #   ic07=128, ic13=256 (128@2x), ic08=256, ic14=512 (256@2x),
    #   ic09=512, ic10=1024 (512@2x).
    # The chunks are fed in Apple's canonical order (baseline then @2x for
    # each base size) so Finder's size cache lookups stay fast.
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

# --- Verify ---------------------------------------------------------------
[ -s "$OUT" ] || die "produced empty $OUT"
# ICNS magic header is the ASCII bytes "icns" at offset 0.
if ! head -c 4 "$OUT" | grep -q icns; then
    die "$OUT is not a valid ICNS file (missing 'icns' magic header)"
fi

echo "Generated: $OUT ($(wc -c < "$OUT") bytes)"
