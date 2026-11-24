#!/bin/bash
H=$HOME/.paneflow-dev-qual-cli/bin/paneflow-ai-hook
CLAUDE_PID=$(ps -o ppid= -p $PPID | tr -d ' ')
export PANEFLOW_AI_TOOL=claude PANEFLOW_AI_PID=$CLAUDE_PID
out=$HOME/pfq-cli-fake/fire-hooks.out
{
echo "claude pid=$CLAUDE_PID"
fire() { echo "$2" | $H "$1"; echo "$1 exit=$?"; sleep 0.4; }
fire SessionStart '{"session_id":"11111111-2222-4333-8444-555555555555","hook_event_name":"SessionStart","source":"startup","cwd":"/Users/m1/pfq-cli-b"}'
fire UserPromptSubmit '{"session_id":"11111111-2222-4333-8444-555555555555","hook_event_name":"UserPromptSubmit","prompt":"qualify the scripting docs"}'
fire PreToolUse '{"session_id":"11111111-2222-4333-8444-555555555555","hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"ls"}}'
fire Notification '{"session_id":"11111111-2222-4333-8444-555555555555","hook_event_name":"Notification","message":"Claude needs your permission to use Bash"}'
fire Stop '{"session_id":"11111111-2222-4333-8444-555555555555","hook_event_name":"Stop","last_assistant_message":"Qualification finished: all good"}'
echo "== no-session case"; (unset PANEFLOW_SESSION_ID; echo '{}' | $H Stop; echo "outside pane exit=$?")
echo "== DONE"
} > "$out" 2>&1
