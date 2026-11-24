#!/bin/bash
out=${1:-$HOME/pfq-cli-fake/inpane-raw.out}
W=$PANEFLOW_SOCKET_PATH; H=$PANEFLOW_HOST_ENDPOINT; V=${PANEFLOW_SOCKET_PATH/paneflow-ipc-/paneflow-serve-}
M=system.whoami,layout.launch,layout.sweep,approval.follow,approval.decide,session.runtime.bind,session.workspace,layout.attach,peer.classify
{
python3 -I ~/pfq-cli-fake/rawrpc2.py window "$W" "$M"
python3 -I ~/pfq-cli-fake/rawrpc2.py worker "$V" "$M"
python3 -I ~/pfq-cli-fake/rawrpc2.py host "$H" "$M"
echo "== DONE"
} > "$out" 2>&1
