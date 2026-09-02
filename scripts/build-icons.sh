#!/usr/bin/env bash
set -euo pipefail

export MAGICK_THREAD_LIMIT=1
export OMP_NUM_THREADS=1

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"

if command -v cygpath >/dev/null 2>&1; then
    REPO_ROOT="$(cygpath -m "$REPO_ROOT")"
fi

MASTER_DIR="$REPO_ROOT/assets/icons/master"
OUT_ICONS_DIR="$REPO_ROOT/assets/icons"
OUT_ICNS="$REPO_ROOT/assets/PaneFlow.icns"
OUT_ICO="$REPO_ROOT/assets/PaneFlow.ico"
OUT_WIX_ICO="$REPO_ROOT/packaging/wix/paneflow.ico"
OUT_RUNTIME_ICON="$REPO_ROOT/src-app/assets/icons/paneflow.png"

log()  { printf '%s\n' "$*" >&2; }
warn() { log "warning: $*"; }
die()  { log "error: $*"; exit 1; }

resolve_master() {
    local stem="$1" path
    for ext in png jpg jpeg; do
        path="$MASTER_DIR/${stem}.${ext}"
        if [ -f "$path" ]; then
            printf '%s' "$path"
            return 0
        fi
    done
    return 1
}

MASTER="$(resolve_master "paneflow-icon-1024"             || true)"
MASTER_MACOS="$(resolve_master "paneflow-icon-macos-1024" || true)"
MASTER_LINUX="$(resolve_master    "paneflow-icon-linux-1024"      || true)"
MASTER_SIMPLE="$(resolve_master   "paneflow-icon-1024-simplified" || true)"
MASTER_TEMPLATE="$(resolve_master "paneflow-icon-template-1024"   || true)"

if [ -z "$MASTER" ]; then
    warn "no master found at $MASTER_DIR/paneflow-icon-1024.{png,jpg,jpeg}"
    warn "keeping existing committed icons. To regenerate, drop a 1024x1024 master in that directory and re-run."
    exit 0
fi

[ -n "$MASTER_MACOS" ] || die "missing macOS master at $MASTER_DIR/paneflow-icon-macos-1024.{png,jpg,jpeg}"

IM_BIN=""
if command -v magick >/dev/null 2>&1; then
    IM_BIN="magick"
elif command -v convert >/dev/null 2>&1 \
    && convert -version 2>&1 | grep -qi "ImageMagick"; then
    IM_BIN="convert"
else
    die "need ImageMagick 6 or 7 to regenerate the complete icon set"
fi

PORTABLE_BODY_PCT=8333
LINUX_BODY_PCT=8750
MACOS_BODY_PCT=8047
MACOS_MASK_RADIUS_PCT=2237

run_magick() {
    local bin="$1"; shift
    local attempt=0
    local max=6
    while : ; do
        if "$bin" "$@"; then
            return 0
        fi
        attempt=$((attempt + 1))
        if [ "$attempt" -ge "$max" ]; then
            warn "$bin failed after $max attempts"
            return 1
        fi
        warn "$bin transient failure (attempt $attempt/$max); retrying in ${attempt}s"
        sleep "$attempt"
    done
}

resize_png() {
    local src="$1" dst="$2" size="$3"
    run_magick "$IM_BIN" "$src" -filter Lanczos -resize "${size}x${size}" -strip "$dst"
}

resize_with_inset_png() {
    local src="$1" dst="$2" size="$3" body_pct="$4"
    local body=$(( size * body_pct / 10000 ))
    [ "$body" -lt 1 ] && body=1
    run_magick "$IM_BIN" \
        \( "$src" -filter Lanczos -resize "${body}x${body}" -alpha On \) \
        +repage -compose Over -background none -gravity center \
        -extent "${size}x${size}" \
        -strip "PNG32:$dst"
}

resize_macos_png() {
    local src="$1" dst="$2" size="$3"
    local body=$(( size * MACOS_BODY_PCT / 10000 ))
    [ "$body" -lt 1 ] && body=1
    local radius=$(( body * MACOS_MASK_RADIUS_PCT / 10000 ))
    local edge=$(( body - 1 ))
    run_magick "$IM_BIN" \
        \( "$src" -filter Lanczos -resize "${body}x${body}" -alpha On \) \
        \( -size "${body}x${body}" xc:none -fill white \
            -draw "roundrectangle 0,0 ${edge},${edge} ${radius},${radius}" \) \
        -compose DstIn -composite \
        +repage -compose Over -background none -gravity center \
        -extent "${size}x${size}" \
        -strip "PNG32:$dst"
}

src_for_size() {
    local size="$1"
    if [ "$size" -le 64 ] && [ -f "$MASTER_SIMPLE" ]; then
        printf '%s' "$MASTER_SIMPLE"
    else
        printf '%s' "$MASTER"
    fi
}

mkdir -p "$OUT_ICONS_DIR"
for size in 16 24 32 48 64 128 256 512; do
    dst="$OUT_ICONS_DIR/paneflow-${size}.png"
    if [ -n "$MASTER_LINUX" ]; then
        log "  $dst  <- $(basename "$MASTER_LINUX")  (full-bleed, keyline applied)"
        resize_with_inset_png "$MASTER_LINUX" "$dst" "$size" "$LINUX_BODY_PCT"
    else
        src="$(src_for_size "$size")"
        log "  $dst  <- $(basename "$src")"
        resize_with_inset_png "$src" "$dst" "$size" "$PORTABLE_BODY_PCT"
    fi
done

mkdir -p "$(dirname "$OUT_RUNTIME_ICON")"
if [ -n "$MASTER_LINUX" ]; then
    log "  $OUT_RUNTIME_ICON  <- $(basename "$(src_for_size 128)")  (portable, not the Linux master)"
    resize_with_inset_png "$(src_for_size 128)" "$OUT_RUNTIME_ICON" 128 "$PORTABLE_BODY_PCT"
else
    cp "$OUT_ICONS_DIR/paneflow-128.png" "$OUT_RUNTIME_ICON"
fi

TMP_ASSETS="$(mktemp -d)"
if command -v cygpath >/dev/null 2>&1; then
    TMP_ASSETS="$(cygpath -m "$TMP_ASSETS")"
fi
trap 'rm -rf "$TMP_ASSETS"' EXIT

case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*)
        warn "skipping .icns regeneration on Windows (keeps the committed copy; macOS leg regenerates its own)"
        ;;
    *)
        TMP_MACOS="$TMP_ASSETS/macos"
        mkdir -p "$TMP_MACOS"
        for size in 16 32 64 128 256 512 1024; do
            resize_macos_png "$MASTER_MACOS" "$TMP_MACOS/paneflow-${size}.png" "$size"
        done
        log "  $OUT_ICNS  (via generate-icns.sh)"
        PANEFLOW_ICNS_SOURCE_DIR="$TMP_MACOS" bash "$SCRIPT_DIR/generate-icns.sh" >&2
        ;;
esac

log "  $OUT_ICO"
TMP_ICO="$TMP_ASSETS/ico"
mkdir -p "$TMP_ICO"
for size in 16 24 32 48 64 128 256; do
    src="$(src_for_size "$size")"
    resize_with_inset_png "$src" "$TMP_ICO/${size}.png" "$size" "$PORTABLE_BODY_PCT"
done

run_magick "$IM_BIN" "$TMP_ICO"/{16,24,32,48,64,128,256}.png "$OUT_ICO"

mkdir -p "$(dirname "$OUT_WIX_ICO")"
cp "$OUT_ICO" "$OUT_WIX_ICO"
log "  $OUT_WIX_ICO  (mirror of $OUT_ICO for cargo-wix)"

if [ -f "$MASTER_TEMPLATE" ]; then
    log "  $OUT_ICONS_DIR/paneflowTemplate.png + @2x"
    resize_png "$MASTER_TEMPLATE" "$OUT_ICONS_DIR/paneflowTemplate.png"    22
    resize_png "$MASTER_TEMPLATE" "$OUT_ICONS_DIR/paneflowTemplate@2x.png" 44
fi

log ""
log "portable icons regenerated from $(basename "$MASTER")"
log "macOS icon source: $(basename "$MASTER_MACOS")"
if [ -n "$MASTER_LINUX" ]; then
    log "Linux hicolor source: $(basename "$MASTER_LINUX") (full-bleed + keyline, Linux only -- .ico/.icns/runtime icon untouched)"
else
    log "no Linux master  -- hicolor sizes use the transparent portable master"
fi
[ -f "$MASTER_SIMPLE" ]   || log  "no simplified master -- sizes <=64 use the transparent portable master"
[ -f "$MASTER_TEMPLATE" ] || log  "no template master  -- skipping menubar Template PNGs"
