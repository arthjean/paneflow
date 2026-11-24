#!/bin/bash
PF=/Users/m1/dev/paneflow/target/debug/paneflow
out=${OUT:-$HOME/pfq-cli-fake/inpane-delegate.out}
{
echo "== split (no command)"; r=$($PF api call surface.split --args "{\"direction\":\"horizontal\",\"surface_id\":${SPLIT_FROM:-3081}}" 2>&1); echo "$r"
sid=$(echo "$r" | python3 -c 'import sys,json; print(json.load(sys.stdin)["surface_id"])' 2>/dev/null)
sleep 1
child=$($PF api call surface.status --args "{\"surface_id\":$sid}" 2>/dev/null | python3 -c 'import sys,json; print(json.load(sys.stdin)["session"])')
echo "child surface=$sid session=$child"
echo "== delegate wider (instance)"; $PF api call grant.delegate --args "{\"session\":\"$child\",\"rights\":[\"write\"],\"scope\":\"instance\"}" 2>&1
echo "== delegate spawn (not held)"; $PF api call grant.delegate --args "{\"session\":\"$child\",\"rights\":[\"spawn\"]}" 2>&1
echo "== delegate to non-spawned pane (producer 490fa6c8)"; $PF api call grant.delegate --args '{"session":"490fa6c8-8d31-458e-b39a-967f979a2b52","rights":["write"]}' 2>&1
echo "== delegate ok"; d=$($PF api call grant.delegate --args "{\"session\":\"$child\",\"rights\":[\"write\"],\"duration_secs\":300}" 2>&1); echo "$d"
gid=$(echo "$d" | python3 -c 'import sys,json; print(json.load(sys.stdin).get("id",""))' 2>/dev/null)
echo "== grants ls from pane"; $PF grants ls 2>&1
echo "== revoke delegated $gid"; $PF grants revoke "$gid" 2>&1; echo "[exit=$?]"
echo "== grants ls after"; $PF grants ls 2>&1
echo "CHILD_SID=$sid"
echo "== DONE"
} > "$out" 2>&1
