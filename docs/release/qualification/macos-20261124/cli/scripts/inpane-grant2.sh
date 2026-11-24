#!/bin/bash
PF=/Users/m1/dev/paneflow/target/debug/paneflow
out=$HOME/pfq-cli-fake/inpane-grant2.out
{
echo "== whoami"; $PF api call system.whoami 2>&1
echo "== caps"; $PF api call system.capabilities 2>&1 | head -3
echo "== grants ls (from pane)"; $PF grants ls 2>&1; echo "[exit=$?]"
echo "== send producer (same ws)"; $PF send producer "echo GRANT_WRITE_OK" 2>&1 | head -2; echo "[exit=${PIPESTATUS[0]}]"
echo "== key producer ctrl-u"; $PF key producer ctrl-u 2>&1; echo "[exit=$?]"
echo "== send 5137 (other ws) default scope"; $PF send 5137 "echo nope" 2>&1 | head -2; echo "[exit=${PIPESTATUS[0]}]"
echo "== send 5137 --scope all"; $PF send 5137 "echo nope" --scope all 2>&1 | head -2; echo "[exit=${PIPESTATUS[0]}]"
echo "== split with command (spawn right missing)"; $PF api call surface.split --args '{"direction":"vertical","surface_id":3081,"command":"echo spawned"}' 2>&1 | head -3; echo "[exit=${PIPESTATUS[0]}]"
echo "== grants revoke someone else's"; $PF grants revoke g-1795539482054-1 2>&1; echo "[exit=$?]"
echo "== grant.request layout workspace"; $PF api call grant.request --args '{"rights":["layout"],"scope":"workspace","duration_secs":60}' 2>&1; echo "[exit=$?]"
echo "== DONE"
} > "$out" 2>&1
