#!/usr/bin/env bash
# Generate every Paneflow icon asset from platform-specific master PNGs.
#
# Inputs (in assets/icons/master/):
#   paneflow-icon-1024.png              required transparent master for Linux + Windows
#   paneflow-icon-macos-1024.png        required plated macOS artwork
#   paneflow-icon-1024-simplified.png   optional transparent master for sizes <=64
#   paneflow-icon-template-1024.png     optional macOS menubar Template image (black silhouette + alpha)
#
# Outputs:
#   assets/icons/paneflow-{16,24,32,48,64,128,256,512}.png   hicolor sizes for cargo-deb / cargo-generate-rpm
#   assets/PaneFlow.icns                                     consumed by scripts/bundle-macos.sh
#   assets/PaneFlow.ico                                      canonical multi-res Windows .ico (build output)
#   packaging/wix/paneflow.ico                               mirror of assets/PaneFlow.ico; the .ico cargo-wix's main.wxs actually reads
#   src-app/assets/icons/paneflow.png                        runtime-embedded GPUI window icon (rust-embed)
#   assets/icons/paneflowTemplate{,@2x}.png                  macOS menubar templates (only if template master exists)
#
# Idempotent and deterministic. Run after editing a master, then commit the regenerated outputs.
#
# Backward compatible: when no master PNG is present at the required path the script logs a
# warning and exits 0. This lets the CI integration land before the masters do and keeps the
# committed (Apr 2026 baseline) icons in place until a master is dropped in.
set -euo pipefail

# Serialise ImageMagick's coder-module loading. The intermittent SIGABRT
# documented on `run_magick` below is a thread race in IM7's module
# registry during first-load of a coder/delegate: two worker threads
# initialise the same module concurrently and abort. Pinning IM to a
# single thread makes module init deterministic and serial. It only
# affects parallelism inside one invocation (icon resizes are tiny, so
# the wall-clock cost is nil) and never changes a single output pixel.
export MAGICK_THREAD_LIMIT=1
export OMP_NUM_THREADS=1

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"

# On Windows CI this runs under Git Bash, where pwd yields an MSYS path
# (/d/a/paneflow/...). The preinstalled ImageMagick is the NATIVE magick.exe,
# which cannot open such paths ("No such file or directory"). Convert to a
# mixed Windows path (D:/a/paneflow/...) that both magick.exe and Git Bash
# accept. cygpath exists only under Git Bash/Cygwin, so this is a no-op on
# Linux/macOS.
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

# Resolve a master by stem: accept .png (preferred), .jpg, or .jpeg so that
# raw Nano Banana / Midjourney / DALL-E exports (which default to JPG) can
# be dropped in without manual conversion. ImageMagick reads either format
# transparently and writes PNG on the output side.
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
MASTER_SIMPLE="$(resolve_master   "paneflow-icon-1024-simplified" || true)"
MASTER_TEMPLATE="$(resolve_master "paneflow-icon-template-1024"   || true)"

# --- Graceful no-op when no master is present ----------------------------
# Apr 2026 baseline shipped committed PNGs directly without a master pipeline.
# This guard lets the CI integration land before the new chrome master does.
if [ -z "$MASTER" ]; then
    warn "no master found at $MASTER_DIR/paneflow-icon-1024.{png,jpg,jpeg}"
    warn "keeping existing committed icons. To regenerate, drop a 1024x1024 master in that directory and re-run."
    exit 0
fi

# A dedicated macOS master prevents the portable transparent mark from leaking
# into the Apple bundle when the release pipeline regenerates every platform.
[ -n "$MASTER_MACOS" ] || die "missing macOS master at $MASTER_DIR/paneflow-icon-macos-1024.{png,jpg,jpeg}"

# --- Resolve ImageMagick before writing any output -----------------------
# build-icons.sh always assembles a multi-resolution Windows ICO, so sips
# alone can never complete the full pipeline. Resolve and validate the tool
# once up front to avoid partial outputs and to reject Windows' unrelated
# System32/convert.exe binary.
IM_BIN=""
if command -v magick >/dev/null 2>&1; then
    IM_BIN="magick"
elif command -v convert >/dev/null 2>&1 \
    && convert -version 2>&1 | grep -qi "ImageMagick"; then
    IM_BIN="convert"
else
    die "need ImageMagick 6 or 7 to regenerate the complete icon set"
fi

# Lanczos is the best general-purpose resampling filter for icon downscaling.
#
# Platform geometry is intentionally split:
#
#   Linux + Windows
#     Keep the app mark transparent. Windows recommends transparent product
#     icons and does not impose a universal outer tile. KDE's 48 px colorful
#     grid reserves 4 px per side, so an 83.33% body is a neutral hicolor
#     fallback that also lands inside GNOME's 128 px app-icon canvas.
#
#   macOS
#     Legacy .icns bundles still need a plated fallback. Keep the traditional
#     824/1024 body and rounded mask isolated to that output. Current Apple
#     system masking is not applied to the Windows or Linux assets.
PORTABLE_BODY_PCT=8333
MACOS_BODY_PCT=8047
MACOS_MASK_RADIUS_PCT=2237

# Run a `magick` (or `convert`) invocation with up to 3 attempts.
# ImageMagick 7.1.2-23 (the current Homebrew bottle on macos-14-arm64,
# and what ships preinstalled on windows-2022) has an intermittent
# SIGABRT (exit 134) during coder-module loading -- the same script, on
# the same runner image, with the same master PNG, will succeed one run
# and crash the next. The Linux apt copy on ubuntu-22.04 is older and
# doesn't hit this, but a cheap retry is worth the safety on every leg.
#
# The first arg picks the IM binary (`magick` for IM7, `convert` for
# IM6); remaining args are passed verbatim. Caller is responsible for
# the if/elif branch; this helper only adds the retry. `if run_magick`
# is set-e-safe because failure inside an `if` test is suppressed.
run_magick() {
    local bin="$1"; shift
    local attempt=0
    # 6 attempts (was 3): the IM7 coder-loader SIGABRT can recur across
    # consecutive identical invocations, so a 3-attempt budget is too
    # tight -- v0.3.6's first tag build exhausted it on the masked
    # `paneflow-512.png` step and failed the whole release. The
    # `MAGICK_THREAD_LIMIT=1` export above is the primary mitigation;
    # the wider budget + escalating backoff is the belt to its braces.
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
        # Escalating backoff (1s, 2s, 3s, ...) gives any transient
        # module-loader / temp-file contention more room between tries
        # than a flat 1s without ballooning total wall-clock.
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
    # 3-element pipeline in a single invocation (fast, no temp files):
    #   1. resize while preserving or creating alpha;
    #   2. draw the legacy macOS rounded mask at the body size;
    #   3. intersect alpha, then center the plated body on the icon canvas.
    run_magick "$IM_BIN" \
        \( "$src" -filter Lanczos -resize "${body}x${body}" -alpha On \) \
        \( -size "${body}x${body}" xc:none -fill white \
            -draw "roundrectangle 0,0 ${edge},${edge} ${radius},${radius}" \) \
        -compose DstIn -composite \
        +repage -compose Over -background none -gravity center \
        -extent "${size}x${size}" \
        -strip "PNG32:$dst"
}

# Source picker: small sizes (<=64) prefer the simplified master to avoid muddy
# chrome reflections at low resolution. Fall back to the full master if no
# simplified version exists -- the small icons will look softer than ideal but
# the release flow keeps working.
src_for_size() {
    local size="$1"
    if [ "$size" -le 64 ] && [ -f "$MASTER_SIMPLE" ]; then
        printf '%s' "$MASTER_SIMPLE"
    else
        printf '%s' "$MASTER"
    fi
}

# --- Linux hicolor PNGs + rust-embed runtime icon ------------------------
mkdir -p "$OUT_ICONS_DIR"
for size in 16 24 32 48 64 128 256 512; do
    src="$(src_for_size "$size")"
    dst="$OUT_ICONS_DIR/paneflow-${size}.png"
    log "  $dst  <- $(basename "$src")"
    resize_with_inset_png "$src" "$dst" "$size" "$PORTABLE_BODY_PCT"
done

# Runtime-embedded GPUI window icon -- rust-embed picks this up at compile
# time for the title-bar / about pane uses. 128px is enough today.
mkdir -p "$(dirname "$OUT_RUNTIME_ICON")"
cp "$OUT_ICONS_DIR/paneflow-128.png" "$OUT_RUNTIME_ICON"

# One shared temporary root keeps native ImageMagick paths valid under Git
# Bash and gives the EXIT trap a single target for both platform pipelines.
TMP_ASSETS="$(mktemp -d)"
if command -v cygpath >/dev/null 2>&1; then
    TMP_ASSETS="$(cygpath -m "$TMP_ASSETS")"
fi
trap 'rm -rf "$TMP_ASSETS"' EXIT

# --- macOS .icns ---------------------------------------------------------
# Generate a dedicated plated iconset, then delegate packing to the existing
# iconutil/png2icns/icnsutil/python3 fallback chain. The hicolor PNGs above
# remain transparent and never leak into the macOS bundle.
#
# Skip on Windows Git Bash: generate-icns.sh requires python3 or one of the
# native packers, and the .icns is only ever consumed by the macOS leg --
# whose CI runs natively on macos-14 with iconutil built into Xcode CLT.
# A failed Windows-side regeneration would be wasted noise.
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

# --- Windows .ico (multi-resolution) -------------------------------------
log "  $OUT_ICO"
TMP_ICO="$TMP_ASSETS/ico"
mkdir -p "$TMP_ICO"
for size in 16 24 32 48 64 128 256; do
    src="$(src_for_size "$size")"
    resize_with_inset_png "$src" "$TMP_ICO/${size}.png" "$size" "$PORTABLE_BODY_PCT"
done

# .ico is a multi-image container. ImageMagick assembles it natively and
# automatically PNG-compresses the 256px frame inside the .ico envelope (the
# rest stay BMP) for Vista+ ProgramsAndFeatures compatibility.
run_magick "$IM_BIN" "$TMP_ICO"/{16,24,32,48,64,128,256}.png "$OUT_ICO"

# Mirror the multi-res .ico into the WiX packaging dir. cargo-wix's
# main.wxs sources the installer / Start-Menu shortcut / Add-or-Remove-
# Programs icon from packaging/wix/paneflow.ico -- NOT assets/PaneFlow.ico.
# Without this copy a new logo refreshes the runtime, Linux and macOS
# icons but leaves the Windows installer icon frozen at the previous
# artwork (the split-brain that shipped the old logo in the v0.5.0 MSI).
# A plain cp keeps the two byte-identical.
mkdir -p "$(dirname "$OUT_WIX_ICO")"
cp "$OUT_ICO" "$OUT_WIX_ICO"
log "  $OUT_WIX_ICO  (mirror of $OUT_ICO for cargo-wix)"

# --- macOS menubar Template PNGs (optional) ------------------------------
# AppKit auto-tints images whose filename ends in `Template.png` /
# `Template@2x.png`. The template master MUST be a black silhouette on alpha
# (no chrome render, no color). We only emit these if a template master is
# placed -- the existing release flow does not consume them yet.
if [ -f "$MASTER_TEMPLATE" ]; then
    log "  $OUT_ICONS_DIR/paneflowTemplate.png + @2x"
    resize_png "$MASTER_TEMPLATE" "$OUT_ICONS_DIR/paneflowTemplate.png"    22
    resize_png "$MASTER_TEMPLATE" "$OUT_ICONS_DIR/paneflowTemplate@2x.png" 44
fi

log ""
log "portable icons regenerated from $(basename "$MASTER")"
log "macOS icon source: $(basename "$MASTER_MACOS")"
[ -f "$MASTER_SIMPLE" ]   || log  "no simplified master -- sizes <=64 use the transparent portable master"
[ -f "$MASTER_TEMPLATE" ] || log  "no template master  -- skipping menubar Template PNGs"
