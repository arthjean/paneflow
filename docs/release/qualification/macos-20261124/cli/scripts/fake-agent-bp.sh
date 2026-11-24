#!/bin/bash
osc() { printf '\033]7501;%s\033\\' "$1"; }
printf "\033[?2004h"; osc "state=idle"
echo "FAKE_AGENT_READY"
while IFS= read -r line; do
  case "$line" in
    ask*) osc "state=blocked:kind=permission:msg=$(printf 'Allow rm?' | base64)"; echo "ASKING";;
    slow*) osc "state=working"; echo "working on: $line"; sleep 8; echo "DONE: $line"; osc "state=done";;
    noop*) echo "ignored: $line";;
    mcp\ *) osc "state=working"; out=${MCP_OUT:-$HOME/pfq-cli-fake/mcp-from-pane.out}; echo "== $(date +%T) $line" >> "$out"; python3 -I $HOME/pfq-cli-fake/mcpclient.py "${line#mcp }" >> "$out" 2>&1; echo "MCP_CALL_DONE"; osc "state=done";;
    run\ *) osc "state=working"; bash -c "${line#run }"; echo "RUN_DONE"; osc "state=done";;
    *) osc "state=working"; echo "working on: $line"; sleep 2; echo "DONE: $line"; osc "state=done";;
  esac
done
