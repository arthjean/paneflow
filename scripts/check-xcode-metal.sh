#!/usr/bin/env bash
set -euo pipefail

CLT_PATH="$(xcode-select -p 2>/dev/null || true)"
if [ -z "$CLT_PATH" ] || [ ! -d "$CLT_PATH" ]; then
  echo "::error::Xcode Command Line Tools not found (xcode-select -p returned '$CLT_PATH')."
  exit 1
fi

SDK_PATH="$(xcrun --show-sdk-path 2>/dev/null || true)"
if [ -z "$SDK_PATH" ] || [ ! -d "$SDK_PATH" ]; then
  echo "::error::macOS SDK not found (xcrun --show-sdk-path returned '$SDK_PATH')."
  exit 1
fi

if ! xcrun --find metal >/dev/null 2>&1; then
  echo "::error::Metal compiler not available - gpui_macos compiles .metal shaders and will fail without it."
  exit 1
fi

METAL_HEADERS="$SDK_PATH/System/Library/Frameworks/Metal.framework/Headers"
if [ ! -d "$METAL_HEADERS" ]; then
  echo "::error::Metal framework headers not found at $METAL_HEADERS."
  exit 1
fi
echo "Xcode CLT: $CLT_PATH | SDK: $SDK_PATH | Metal: $(xcrun --find metal)"
