#!/usr/bin/env bash
# Build the PaneFlow AppImage via linuxdeploy.
#
# Produces:
#   target/appimage/paneflow-<version>-x86_64.AppImage
#   target/appimage/paneflow-<version>-x86_64.AppImage.zsync
#
# Environment:
#   TARGET          - optional rust target triple (picks target/$TARGET/release/paneflow)
#   PANEFLOW_BIN    - optional path to prebuilt binary (overrides auto-detection)
#   LINUXDEPLOY     - optional path to a linuxdeploy binary (else downloaded)
#   SOURCE_DATE_EPOCH - reproducible timestamps if set
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"

# ARCH selects which linuxdeploy + appimagetool release we download, which
# UPDATE_INFORMATION zsync glob we embed, and the canonical output filename
# suffix. Defaults to x86_64 so the script still works unmodified on a dev
# host; CI passes `ARCH=aarch64` on the ARM matrix leg (US-019).
ARCH="${ARCH:-x86_64}"
case "$ARCH" in
    x86_64|aarch64) ;;
    *) echo "error: unsupported ARCH='$ARCH' (expected x86_64 or aarch64)" >&2; exit 1 ;;
esac
LINUXDEPLOY_VERSION="1-alpha-20251107-1"
APPIMAGETOOL_VERSION="1.9.1"
LINUXDEPLOY_URL="https://github.com/linuxdeploy/linuxdeploy/releases/download/${LINUXDEPLOY_VERSION}/linuxdeploy-${ARCH}.AppImage"
APPIMAGETOOL_URL="https://github.com/AppImage/appimagetool/releases/download/${APPIMAGETOOL_VERSION}/appimagetool-${ARCH}.AppImage"
case "$ARCH" in
    x86_64)
        LINUXDEPLOY_SHA256="c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d"
        APPIMAGETOOL_SHA256="ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0"
        ;;
    aarch64)
        LINUXDEPLOY_SHA256="620095110d693282b8ebeb244a95b5e911cf8f65f76c88b4b47d16ae6346fcff"
        APPIMAGETOOL_SHA256="f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158"
        ;;
esac

verify_sha256() {
    file="$1"
    expected="$2"
    echo "${expected}  ${file}" | sha256sum -c - >/dev/null
}

download_verified_tool() {
    dst="$1"
    url="$2"
    expected="$3"
    label="$4"

    if [ -x "$dst" ]; then
        if verify_sha256 "$dst" "$expected"; then
            return 0
        fi
        echo "warning: cached ${label} failed SHA-256 verification; re-downloading" >&2
        rm -f "$dst"
    fi

    tmp="${dst}.tmp.$$"
    rm -f "$tmp"
    curl --fail --location --silent --show-error -o "$tmp" "$url"
    verify_sha256 "$tmp" "$expected"
    mv "$tmp" "$dst"
    chmod +x "$dst"
}

# --- version -------------------------------------------------------------
if [ "$#" -ge 1 ]; then
    VERSION="$1"
else
    VERSION="$(awk -F'"' '/^version = / { print $2; exit }' "$REPO_ROOT/Cargo.toml")"
fi
if [ -z "${VERSION:-}" ]; then
    echo "error: could not determine version" >&2
    exit 1
fi

# --- locate release binary ----------------------------------------------
BIN="${PANEFLOW_BIN:-}"
if [ -z "$BIN" ]; then
    if [ -n "${TARGET:-}" ]; then
        BIN="$REPO_ROOT/target/$TARGET/release/paneflow"
    elif [ -x "$REPO_ROOT/target/release/paneflow" ]; then
        BIN="$REPO_ROOT/target/release/paneflow"
    elif [ -x "$REPO_ROOT/target/x86_64-unknown-linux-gnu/release/paneflow" ]; then
        BIN="$REPO_ROOT/target/x86_64-unknown-linux-gnu/release/paneflow"
    fi
fi
if [ ! -x "$BIN" ]; then
    echo "error: release binary not found (set PANEFLOW_BIN or run 'cargo build --release -p paneflow-app')" >&2
    exit 1
fi

# --- linuxdeploy --------------------------------------------------------
LD_BIN="${LINUXDEPLOY:-}"
if [ -z "$LD_BIN" ]; then
    TOOLS_DIR="$REPO_ROOT/target/tools"
    LD_BIN="$TOOLS_DIR/linuxdeploy-${ARCH}.AppImage"
    mkdir -p "$TOOLS_DIR"
    if [ ! -x "$LD_BIN" ]; then
        echo "info: downloading linuxdeploy..." >&2
    fi
    download_verified_tool "$LD_BIN" "$LINUXDEPLOY_URL" "$LINUXDEPLOY_SHA256" "linuxdeploy"
fi

# --- stage AppDir -------------------------------------------------------
OUT_DIR="$REPO_ROOT/target/appimage"
APPDIR="$OUT_DIR/PaneFlow.AppDir"
rm -rf "$APPDIR" "$OUT_DIR"/*.AppImage "$OUT_DIR"/*.AppImage.zsync
mkdir -p "$APPDIR/usr/share/metainfo"

# Pre-stage AppStream metainfo so linuxdeploy picks it up
install -m 644 "$REPO_ROOT/assets/io.github.arthurdev44.paneflow.metainfo.xml" \
               "$APPDIR/usr/share/metainfo/io.github.arthurdev44.paneflow.metainfo.xml"

# --- invoke linuxdeploy -------------------------------------------------
# NOTE: UPDATE_INFORMATION must be set in the environment BEFORE linuxdeploy
# runs so the string is embedded at the fixed ELF offset recognised by
# AppImageUpdate / appimageupdatetool.
export UPDATE_INFORMATION="gh-releases-zsync|ArthurDEV44|paneflow|latest|paneflow-*-${ARCH}.AppImage.zsync"

# Force linuxdeploy to use extract-and-run mode so FUSE 2 isn't required on
# build hosts that don't have libfuse2 (Fedora 43, Ubuntu 24.04, etc.).
export APPIMAGE_EXTRACT_AND_RUN=1

# Disable linuxdeploy's bundled `strip` - it's too old to recognise modern
# SHT_RELR (.relr.dyn) sections emitted by newer toolchains (binutils 2.38+,
# glibc 2.36+). The release binary is already stripped by `strip = true` in
# [profile.release], so there's nothing to gain by stripping again.
export NO_STRIP=1

# linuxdeploy bundles a patchelf that can mis-write RUNPATH on very-modern
# PIE binaries carrying SHT_RELR (.relr.dyn) sections - emitted by binutils
# 2.38+ / glibc 2.36+. The result: binary segfaults pre-main. We detect a
# host patchelf >=0.18 and post-patch the binary after linuxdeploy runs.
HOST_PATCHELF="$(command -v patchelf || true)"

cd "$OUT_DIR"

# Pass 1: populate AppDir and let linuxdeploy patch deps. Skip --output so
# we can re-patch the main binary before packing.
"$LD_BIN" \
    --appdir "$APPDIR" \
    --executable "$BIN" \
    --desktop-file "$REPO_ROOT/assets/paneflow.desktop" \
    --icon-file "$REPO_ROOT/assets/icons/paneflow-256.png" \
    --icon-filename paneflow \
    --custom-apprun "$REPO_ROOT/packaging/AppRun"

# Heal any binaries/libraries corrupted by linuxdeploy's bundled patchelf
# on modern SHT_RELR ELF files. On hosts where the toolchain does not emit
# .relr.dyn (the Ubuntu 22.04 CI runner) this is a harmless no-op.
#
# Strategy:
#   1. For each .so in usr/lib/ (not a symlink), look up a pristine copy in
#      the system's dynamic linker cache and overwrite the (corrupted) one.
#   2. Run host patchelf (>=0.18) to re-apply the $ORIGIN RPATH linuxdeploy
#      used - cleanly this time.
#   3. Re-apply $ORIGIN/../lib RPATH to the main binary.
if [ -n "$HOST_PATCHELF" ]; then
    PATCHELF_VER="$("$HOST_PATCHELF" --version 2>/dev/null | awk '{print $2}')"
    PATCHELF_MAJOR="${PATCHELF_VER%%.*}"
    PATCHELF_MINOR="${PATCHELF_VER#*.}"; PATCHELF_MINOR="${PATCHELF_MINOR%%.*}"
    if [ -n "$PATCHELF_VER" ] \
       && { [ "$PATCHELF_MAJOR" -gt 0 ] 2>/dev/null \
            || [ "$PATCHELF_MINOR" -ge 18 ] 2>/dev/null; }; then
        echo "info: healing AppDir with patchelf $PATCHELF_VER" >&2

        # Replace bundled libs with pristine system copies. We intentionally
        # do NOT run patchelf on them - modern patchelf (even 0.18) can
        # corrupt Fedora 43 libs on modern toolchains. Our AppRun sets
        # LD_LIBRARY_PATH=$HERE/usr/lib so the libs don't need an $ORIGIN
        # RPATH to find each other.
        #
        # `ldconfig -p` tags entries by ABI: `(libc6,x86-64)` on x86_64
        # hosts and `(libc6,AArch64)` on aarch64 hosts. Matching on the
        # wrong tag leaves every bundled lib un-healed - on an ARM
        # runner the old x86-64 filter silently picked zero libs and
        # any linuxdeploy-corrupted .so shipped broken. Filter by the
        # tag that matches our current $ARCH.
        case "$ARCH" in
            x86_64)  LDCONFIG_TAG='(libc6,x86-64)' ;;
            aarch64) LDCONFIG_TAG='(libc6,AArch64)' ;;
        esac
        LDCONFIG_CACHE="$(ldconfig -p 2>/dev/null || true)"
        for lib in "$APPDIR"/usr/lib/*.so*; do
            [ -L "$lib" ] && continue
            [ -f "$lib" ] || continue
            name="$(basename "$lib")"
            src="$(printf '%s\n' "$LDCONFIG_CACHE" \
                    | awk -v n="$name" -v tag="$LDCONFIG_TAG" \
                        '$1==n && index($0, tag) {print $NF; found=1} found{exit}' \
                    || true)"
            if [ -n "$src" ] && [ -f "$src" ]; then
                cp -f "$src" "$lib"
            fi
        done

        # Re-patch only the main binary's RUNPATH (linuxdeploy's bundled
        # patchelf can corrupt it too). The binary is a Rust PIE, not a
        # Fedora system lib, so host patchelf handles it cleanly.
        "$HOST_PATCHELF" --set-rpath '$ORIGIN/../lib' "$APPDIR/usr/bin/paneflow" 2>/dev/null || true
    fi
fi

# Pass 2: pack the AppDir into an AppImage. We call appimagetool directly
# instead of `linuxdeploy --output appimage` because the linuxdeploy output
# step re-walks the AppDir and re-invokes its (old, broken) bundled patchelf
# on every ELF - undoing the healing above. appimagetool just mksquashfs's
# the AppDir and prepends the AppImage runtime, leaving contents untouched.
AT_BIN="${APPIMAGETOOL:-}"
if [ -z "$AT_BIN" ]; then
    AT_BIN="$REPO_ROOT/target/tools/appimagetool-${ARCH}.AppImage"
    mkdir -p "$(dirname "$AT_BIN")"
    if [ ! -x "$AT_BIN" ]; then
        echo "info: downloading appimagetool..." >&2
    fi
    download_verified_tool "$AT_BIN" "$APPIMAGETOOL_URL" "$APPIMAGETOOL_SHA256" "appimagetool"
fi

# appimagetool reads UPDATE_INFORMATION via -u flag (env var is ignored).
"$AT_BIN" --updateinformation "$UPDATE_INFORMATION" "$APPDIR"

# --- regression guard: no bundled GPU drivers ---------------------------
BAD=$(find "$APPDIR/usr/lib" \
          \( -name 'libvulkan_*.so*' -o -name 'nvidia_icd.json' \) 2>/dev/null || true)
if [ -n "$BAD" ]; then
    echo "error: forbidden GPU files inside AppDir:" >&2
    echo "$BAD" >&2
    exit 1
fi

# --- rename output to canonical pattern ---------------------------------
# linuxdeploy names the output after the desktop file's Name= field, so
# something like `PaneFlow-x86_64.AppImage`. Rename to the release-asset
# canonical form.
PRODUCED=$(ls -1 "$OUT_DIR"/*.AppImage 2>/dev/null | head -n1 || true)
if [ -z "$PRODUCED" ]; then
    echo "error: linuxdeploy did not produce an AppImage" >&2
    exit 1
fi

APPIMAGE="$OUT_DIR/paneflow-${VERSION}-${ARCH}.AppImage"
ZSYNC="$APPIMAGE.zsync"

mv "$PRODUCED" "$APPIMAGE"
if [ -f "$PRODUCED.zsync" ]; then
    mv "$PRODUCED.zsync" "$ZSYNC"
fi

# --- size guard: < 80 MB ------------------------------------------------
SIZE=$(stat -c%s "$APPIMAGE")
MAX=$((80 * 1024 * 1024))
if [ "$SIZE" -ge "$MAX" ]; then
    echo "error: AppImage exceeds 80 MB budget ($SIZE bytes)" >&2
    exit 1
fi

echo "$APPIMAGE"
echo "$ZSYNC"
