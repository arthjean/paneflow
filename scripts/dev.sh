#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
profile=()
profile_dir=debug
if [ "${1:-}" = "--release" ]; then
    profile=(--release)
    profile_dir=release
    shift
fi
cargo build -p paneflow-app -p paneflow-host --locked ${profile[@]+"${profile[@]}"}
exec "target/$profile_dir/paneflow" "$@"
