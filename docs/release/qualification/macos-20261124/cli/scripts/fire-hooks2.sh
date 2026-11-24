#!/bin/bash
H=$HOME/.paneflow-dev-qual-cli/bin/paneflow-ai-hook
CLAUDE_PID=$(ps -o ppid= -p $PPID | tr -d ' ')
export PANEFLOW_AI_TOOL=claude PANEFLOW_AI_PID=$CLAUDE_PID PANEFLOW_HOOK_LOG=$HOME/pfq-cli-fake/hook.log
out=$HOME/pfq-cli-fake/fire-hooks2.out
{
fire() { echo "$2" | $H "$1"; echo "$1 exit=$?"; sleep 0.5; }
fire Notification '{"session_id":"11111111-2222-4333-8444-555555555555","hook_event_name":"Notification","notification_type":"permission_prompt","message":"Claude needs your permission to use Bash"}'
fire PermissionRequest '{"session_id":"11111111-2222-4333-8444-555555555555","hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{"command":"rm -rf build"}}'
fire Stop '{"session_id":"11111111-2222-4333-8444-555555555555","hook_event_name":"Stop","last_assistant_message":"second turn done"}'
fire SessionEnd '{"session_id":"11111111-2222-4333-8444-555555555555","hook_event_name":"SessionEnd","reason":"exit"}'
PANEFLOW_AI_EXIT_CODE=0 $H Exit < /dev/null; echo "Exit exit=$?"
echo "== DONE"
} > "$out" 2>&1
