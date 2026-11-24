#!/bin/bash
PF=/Users/m1/dev/paneflow/target/debug/paneflow
out=$HOME/pfq-cli-fake/inpane-reserved.out
{
echo "== api list (names only, grep reserved)"; $PF api list 2>&1 | grep -E 'approval|peer.classify|session.runtime|session.workspace|host.shutdown|layout\.(launch|sweep|attach)'
for on in window worker host; do
 for m in approval.decide approval.follow session.runtime.bind session.workspace peer.classify layout.launch layout.attach layout.sweep; do
  echo "== $m --on $on"; $PF api call $m --on $on --args '{"pid":1,"surface_id":2053,"session":"x","id":"x","decision":"deny"}' 2>&1 | head -3; echo "[exit=${PIPESTATUS[0]}]"
 done
done
echo "== DONE"
} > "$out" 2>&1
