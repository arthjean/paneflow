#!/bin/bash
PF=/Users/m1/dev/paneflow/target/debug/paneflow
out=$HOME/pfq-cli-fake/inpane-delegate2.out
child=$1
{
echo "== whoami"; $PF api call system.whoami
echo "== delegate no duration to $child"; $PF api call grant.delegate --args "{\"session\":\"$child\",\"rights\":[\"write\"]}" 2>&1
echo "== delegate duration 0"; $PF api call grant.delegate --args "{\"session\":\"$child\",\"rights\":[\"write\"],\"duration_secs\":0}" 2>&1
echo "== DONE"
} > "$out" 2>&1
