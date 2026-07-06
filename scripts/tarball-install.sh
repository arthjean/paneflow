#!/usr/bin/env bash
# PaneFlow user-local installer.
#
# Runs from inside an extracted `paneflow.app/` directory and installs to
# $HOME/.local/paneflow.app/, following the Zed distribution model.
#
# No sudo. No writes outside $HOME. Safe to re-run (atomic swap).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"

if [ ! -x "$SCRIPT_DIR/bin/paneflow" ]; then
    echo "error: $SCRIPT_DIR/bin/paneflow not found or not executable" >&2
    echo "hint:  run this script from the extracted paneflow.app/ directory." >&2
    exit 1
fi

LOCAL="$HOME/.local"
APP="$LOCAL/paneflow.app"
APP_OLD="$LOCAL/paneflow.app.old"
APP_NEW="$LOCAL/paneflow.app.new.$$"
BIN="$LOCAL/bin"
APPS="$LOCAL/share/applications"
ICONS="$LOCAL/share/icons/hicolor"

mkdir -p "$LOCAL" "$BIN" "$APPS"

# --- atomic swap ---------------------------------------------------------
# 1. Copy new tree into a sibling staging dir.
# 2. Validate the staged binary exists and is executable.
# 3. If a previous install exists, rename it to .app.old.
# 4. Rename staging to $APP.
# 5. On failure after step 3, restore .app.old to $APP.
# 6. On success, remove .app.old.
# ------------------------------------------------------------------------

# If a prior install died after moving live to .old, restore it before doing
# any new work. If both live and .old exist, refuse to guess which tree should
# win.
if [ -e "$APP_OLD" ]; then
    if [ ! -e "$APP" ]; then
        mv "$APP_OLD" "$APP"
        echo "Recovered previous install from $APP_OLD" >&2
    else
        echo "error: $APP_OLD already exists next to a live install." >&2
        echo "       Inspect it, then run: rm -rf '$APP_OLD'" >&2
        exit 1
    fi
fi

HAD_PREVIOUS=0

cleanup_on_failure() {
    status=$?
    if [ $status -ne 0 ]; then
        rm -rf "$APP_NEW" || true
        if [ "$HAD_PREVIOUS" -eq 1 ] && [ ! -e "$APP" ] && [ -e "$APP_OLD" ]; then
            mv "$APP_OLD" "$APP" || true
        fi
        echo "" >&2
        echo "error: install failed before promotion completed." >&2
        if [ "$HAD_PREVIOUS" -eq 1 ]; then
            echo "       Previous install was restored at: $APP" >&2
        fi
    fi
}
trap cleanup_on_failure EXIT

rm -rf "$APP_NEW"
mkdir -p "$APP_NEW"
cp -R "$SCRIPT_DIR"/. "$APP_NEW"/

if [ ! -x "$APP_NEW/bin/paneflow" ]; then
    echo "error: staged install is missing executable $APP_NEW/bin/paneflow" >&2
    exit 1
fi

if [ -e "$APP" ]; then
    mv "$APP" "$APP_OLD"
    HAD_PREVIOUS=1
fi

mv "$APP_NEW" "$APP"

# Staging succeeded - clear the failure trap and remove the old backup.
trap - EXIT
if [ "$HAD_PREVIOUS" -eq 1 ]; then
    rm -rf "$APP_OLD"
fi

# --- symlink, desktop entry, icons --------------------------------------
ln -sfn "$APP/bin/paneflow" "$BIN/paneflow"

DESKTOP_SRC="$APP/share/applications/paneflow.desktop"
if [ -f "$DESKTOP_SRC" ]; then
    sed "s|^Exec=.*|Exec=$BIN/paneflow|" "$DESKTOP_SRC" > "$APPS/paneflow.desktop"
fi

if [ -d "$APP/share/icons/hicolor" ]; then
    for size in 16 32 48 128 256 512; do
        src="$APP/share/icons/hicolor/${size}x${size}/apps/paneflow.png"
        if [ -f "$src" ]; then
            dest="$ICONS/${size}x${size}/apps"
            mkdir -p "$dest"
            cp -f "$src" "$dest/paneflow.png"
        fi
    done
fi

# --- best-effort cache refresh ------------------------------------------
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$ICONS" >/dev/null 2>&1 || true
fi
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$APPS" >/dev/null 2>&1 || true
fi

echo "PaneFlow installed to $APP"
echo "Symlink: $BIN/paneflow"

case ":$PATH:" in
    *":$BIN:"*) ;;
    *)
        echo ""
        echo "note: $BIN is not in your PATH."
        echo "      add this to your shell profile:"
        echo "        export PATH=\"\$HOME/.local/bin:\$PATH\""
        ;;
esac
