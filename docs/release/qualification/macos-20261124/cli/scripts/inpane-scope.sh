#!/bin/bash
PF=/Users/m1/dev/paneflow/target/debug/paneflow
out=${1:-$HOME/pfq-cli-fake/inpane-scope.out}
{
echo "== env"; env | grep -E '^PANEFLOW_(SESSION_ID|SURFACE_ID|WORKSPACE_ID|WORKSPACE_UUID)='
echo "== ls"; $PF ls 2>&1 | grep -E 'surface_id|workspace|scope'
for t in $TARGETS; do echo "== send $t"; $PF send $t "echo from-pane-$PANEFLOW_SURFACE_ID" 2>&1 | head -3; echo "[exit=${PIPESTATUS[0]}]"; done
for t in $TARGETS; do echo "== send $t --scope all"; $PF send $t "echo from-pane-all-$PANEFLOW_SURFACE_ID" --scope all 2>&1 | head -3; echo "[exit=${PIPESTATUS[0]}]"; done
echo "== DONE"
} > "$out" 2>&1
