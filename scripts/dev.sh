#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
profile=()
profile_dir=debug
tag=""
while [ $# -gt 0 ]; do
    case "$1" in
        --release)
            profile=(--release)
            profile_dir=release
            shift
            ;;
        --tag)
            if [ $# -lt 2 ]; then
                echo "dev.sh: --tag needs a name" >&2
                exit 2
            fi
            tag=$(printf '%s' "$2" | tr '[:upper:]' '[:lower:]' | sed -E 's/[^a-z0-9]+/-/g; s/^-+//; s/-+$//')
            if [ -z "$tag" ]; then
                echo "dev.sh: --tag '$2' has no letter or digit" >&2
                exit 2
            fi
            shift 2
            ;;
        *)
            break
            ;;
    esac
done
if [ -n "$tag" ]; then
    export PANEFLOW_HOME="$HOME/.paneflow-dev-$tag"
    echo "dev.sh: tag '$tag' runs on its own state home $PANEFLOW_HOME" >&2
fi
cargo build -p paneflow-app -p paneflow-host --locked ${profile[@]+"${profile[@]}"}
exec "target/$profile_dir/paneflow" "$@"
