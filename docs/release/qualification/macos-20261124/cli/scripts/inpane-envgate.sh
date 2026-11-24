#!/bin/bash
PF=/Users/m1/dev/paneflow/target/debug/paneflow
out=${OUT:-$HOME/pfq-cli-fake/inpane-envgate.out}
{ echo "== $(date +%T) caps"; $PF api call system.capabilities 2>&1 | head -3
  echo "== send 1026 from pane"; $PF send 1026 "echo pane-envgate" 2>&1 | head -2; echo "[exit=${PIPESTATUS[0]}]"
  $PF key 1026 ctrl-u >/dev/null 2>&1; echo "== DONE"; } > "$out" 2>&1
