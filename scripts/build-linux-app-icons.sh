#!/usr/bin/env bash
set -euo pipefail

export MAGICK_THREAD_LIMIT=1
export OMP_NUM_THREADS=1

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"
MASTER_DIR="$REPO_ROOT/assets/icons/master"
OUT_DIR="$REPO_ROOT/src-app/assets/icons/app"

SIZE=256
LINUX_BODY_PCT=8750
BODY=$(( SIZE * LINUX_BODY_PCT / 10000 ))

SKINS=(
    aubergine paper midnight-matte polar melon honeydew berry rainbow
    solarwave neon phosphor holo amethyst blueprint chip nostalgia
)
DRAWINGS=(
    "black:bars/paneflow-icon-bars-black.svg"
    "white:bars/paneflow-icon-bars-white.svg"
    "black-solid:solid/paneflow-icon-solid-black.svg"
    "white-solid:solid/paneflow-icon-solid-white.svg"
)

die() { printf 'error: %s\n' "$*" >&2; exit 1; }

command -v magick >/dev/null 2>&1 || die "need ImageMagick 7 (magick)"
magick -list format 2>/dev/null | grep -q "RSVG" || die "need the librsvg delegate of ImageMagick (magick -list format | grep RSVG)"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

emit() {
    local src="$1" slug="$2"
    local dst="$OUT_DIR/${slug}-linux-${SIZE}.png"
    magick \( "$src" -filter Lanczos -resize "${BODY}x${BODY}" -alpha On \) \
        +repage -compose Over -background none -gravity center \
        -extent "${SIZE}x${SIZE}" \
        -strip -define png:exclude-chunks=date,time "PNG32:$dst"
    printf '%s\n' "${dst#"$REPO_ROOT/"}"
}

for skin in "${SKINS[@]}"; do
    master="$MASTER_DIR/skins/${skin}-1024.png"
    [ -f "$master" ] || die "missing master $master"
    emit "$master" "$skin"
done

for entry in "${DRAWINGS[@]}"; do
    slug="${entry%%:*}"
    svg="$MASTER_DIR/svg/${entry#*:}"
    [ -f "$svg" ] || die "missing drawing $svg"
    render="$WORK/${slug}-1024.png"
    magick -background none "RSVG:$svg" -strip "PNG32:$render"
    emit "$render" "$slug"
done
