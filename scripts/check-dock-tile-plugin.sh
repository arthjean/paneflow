#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd -P)"

TRIPLE="${1:-aarch64-apple-darwin}"
PROBE_ID="io.github.arthurdev44.paneflow.dock-tile-probe"
WORK="$(mktemp -d -t paneflow-dock-tile.XXXXXX)"
trap 'defaults delete "$PROBE_ID" >/dev/null 2>&1 || true; rm -rf "$WORK"' EXIT

APP="$WORK/Probe.app"
mkdir -p "$APP/Contents/PlugIns"
cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>$PROBE_ID</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleName</key>
    <string>Probe</string>
</dict>
</plist>
EOF

bash "$SCRIPT_DIR/build-dock-tile-plugin.sh" \
    --target "$TRIPLE" \
    --version 0.0.0 \
    --out "$APP/Contents/PlugIns"

PLUGIN="$APP/Contents/PlugIns/PaneflowDockTile.docktileplugin"
codesign --force --sign - "$PLUGIN"
codesign --verify --strict "$PLUGIN"

cargo run --release --locked \
    -p paneflow-dock-tile --example probe \
    --target "$TRIPLE" \
    --manifest-path "$REPO_ROOT/Cargo.toml" \
    -- "$PLUGIN"
