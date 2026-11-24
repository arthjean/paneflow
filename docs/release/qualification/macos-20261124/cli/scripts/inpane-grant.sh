#!/bin/bash
PF=/Users/m1/dev/paneflow/target/debug/paneflow
out=${OUT:-$HOME/pfq-cli-fake/inpane-grant.out}
{
echo "== $(date +%T) send before grant"; $PF send ${TARGET:-1026} "echo before-grant" 2>&1; echo "[exit=$?]"
echo "== $(date +%T) grant.request $ARGS"; $PF api call grant.request --args "$ARGS" 2>&1; echo "[exit=$?]"
echo "== $(date +%T) grants ls"; $PF grants ls 2>&1; echo "[exit=$?]"
echo "== $(date +%T) send after grant"; $PF send ${TARGET:-1026} "echo after-grant" 2>&1; echo "[exit=$?]"
echo "== $(date +%T) send cross-ws --scope all"; $PF send 5137 "echo xws" --scope all 2>&1; echo "[exit=$?]"
echo "== DONE"
} > "$out" 2>&1
