#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"

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

if [ "$#" -ge 1 ]; then
    VERSION="$1"
else
    VERSION="$(awk -F'"' '/^version = / { print $2; exit }' "$REPO_ROOT/Cargo.toml")"
fi
if [ -z "${VERSION:-}" ]; then
    echo "error: could not determine version" >&2
    exit 1
fi

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

OUT_DIR="$REPO_ROOT/target/appimage"
APPDIR="$OUT_DIR/PaneFlow.AppDir"
rm -rf "$APPDIR" "$OUT_DIR"/*.AppImage "$OUT_DIR"/*.AppImage.zsync
mkdir -p "$APPDIR/usr/share/metainfo"
mkdir -p "$APPDIR/usr/share/doc/paneflow"

install -m 644 "$REPO_ROOT/assets/io.github.arthurdev44.paneflow.metainfo.xml" \
               "$APPDIR/usr/share/metainfo/io.github.arthurdev44.paneflow.metainfo.xml"
install -m 644 "$REPO_ROOT/native/libghostty/THIRD_PARTY_NOTICES.md" \
               "$APPDIR/usr/share/doc/paneflow/THIRD_PARTY_NOTICES.md"

export UPDATE_INFORMATION="gh-releases-zsync|arthjean|paneflow|latest|paneflow-*-${ARCH}.AppImage.zsync"

export APPIMAGE_EXTRACT_AND_RUN=1

export NO_STRIP=1

HOST_PATCHELF="$(command -v patchelf || true)"

cd "$OUT_DIR"

"$LD_BIN" \
    --appdir "$APPDIR" \
    --executable "$BIN" \
    --desktop-file "$REPO_ROOT/assets/paneflow.desktop" \
    --icon-file "$REPO_ROOT/assets/icons/paneflow-256.png" \
    --icon-filename paneflow \
    --custom-apprun "$REPO_ROOT/packaging/AppRun"

if [ -n "$HOST_PATCHELF" ]; then
    PATCHELF_VER="$("$HOST_PATCHELF" --version 2>/dev/null | awk '{print $2}')"
    PATCHELF_MAJOR="${PATCHELF_VER%%.*}"
    PATCHELF_MINOR="${PATCHELF_VER#*.}"; PATCHELF_MINOR="${PATCHELF_MINOR%%.*}"
    if [ -n "$PATCHELF_VER" ] \
       && { [ "$PATCHELF_MAJOR" -gt 0 ] 2>/dev/null \
            || [ "$PATCHELF_MINOR" -ge 18 ] 2>/dev/null; }; then
        echo "info: healing AppDir with patchelf $PATCHELF_VER" >&2

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

        "$HOST_PATCHELF" --set-rpath '$ORIGIN/../lib' "$APPDIR/usr/bin/paneflow" 2>/dev/null || true
    fi
fi

AT_BIN="${APPIMAGETOOL:-}"
if [ -z "$AT_BIN" ]; then
    AT_BIN="$REPO_ROOT/target/tools/appimagetool-${ARCH}.AppImage"
    mkdir -p "$(dirname "$AT_BIN")"
    if [ ! -x "$AT_BIN" ]; then
        echo "info: downloading appimagetool..." >&2
    fi
    download_verified_tool "$AT_BIN" "$APPIMAGETOOL_URL" "$APPIMAGETOOL_SHA256" "appimagetool"
fi

"$AT_BIN" --updateinformation "$UPDATE_INFORMATION" "$APPDIR"

BAD=$(find "$APPDIR/usr/lib" \
          \( -name 'libvulkan_*.so*' -o -name 'nvidia_icd.json' \) 2>/dev/null || true)
if [ -n "$BAD" ]; then
    echo "error: forbidden GPU files inside AppDir:" >&2
    echo "$BAD" >&2
    exit 1
fi

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

SIZE=$(stat -c%s "$APPIMAGE")
MAX=$((80 * 1024 * 1024))
if [ "$SIZE" -ge "$MAX" ]; then
    echo "error: AppImage exceeds 80 MB budget ($SIZE bytes)" >&2
    exit 1
fi

echo "$APPIMAGE"
echo "$ZSYNC"
