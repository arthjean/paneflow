#!/bin/sh
[ -n "${PANEFLOW_SESSION_ID:-}" ] || exit 0
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec "$SCRIPT_DIR/paneflow-ai-hook" "$@"
