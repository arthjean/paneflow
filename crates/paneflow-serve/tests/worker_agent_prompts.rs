#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::time::Duration;

use paneflow_ipc_client::host_control::HostControl;
use serde_json::{Value, json};

const PATIENCE: Duration = Duration::from_secs(30);

fn call(home: &Path, method: &str, params: Value) -> Value {
    HostControl::connect(
        &paneflow_home::serve_endpoint_path(home),
        "agent-prompts-test",
    )
    .unwrap()
    .request_with_deadline(method, params, Duration::from_secs(60))
    .unwrap()
}

fn open(home: &Path, config: &str) -> paneflow_serve::worker::RunningWorker {
    std::fs::write(home.join("paneflow.json"), config).unwrap();
    paneflow_serve::open(home).unwrap()
}

fn window(home: &Path) -> HostControl {
    let mut follower =
        HostControl::connect(&paneflow_home::serve_endpoint_path(home), "window").unwrap();
    let header = follower
        .request("prompt.follow", json!({"kinds": ["input", "pick"]}))
        .unwrap();
    assert_eq!(header["type"], "following");
    follower
}

fn next_requested(follower: &mut HostControl) -> Value {
    let deadline = std::time::Instant::now() + PATIENCE;
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "no prompt was requested"
        );
        let line = follower.read_stream_line(PATIENCE).unwrap().unwrap();
        let frame: Value = serde_json::from_str(&line).unwrap();
        if frame["type"] == "requested" {
            return frame;
        }
    }
}

fn resolve_later(home: &Path, params: Value) -> std::thread::JoinHandle<Value> {
    let home = home.to_path_buf();
    std::thread::spawn(move || call(&home, "agent_prompt.resolve", params))
}

fn reply(home: &Path, id: &Value, answer: Value) {
    call(home, "prompt.reply", json!({"id": id, "answer": answer}));
}

fn questions() -> Value {
    json!([
        {"question": "Which color?", "header": "Color", "multiSelect": false,
         "options": [{"label": "Red", "description": "Warm"}, {"label": "Blue", "description": "Cold"}]},
        {"question": "Which sizes?", "header": "Size", "multiSelect": true,
         "options": [{"label": "S"}, {"label": "M"}, {"label": "L"}]}
    ])
}

fn ask(agent: &str) -> Value {
    json!({
        "event": "ask_user_question",
        "agent": agent,
        "payload": {"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion", "tool_input": {"questions": questions()}}
    })
}

fn permission(tool: &str, input: Value) -> Value {
    json!({
        "event": "permission_request",
        "agent": "claude",
        "payload": {"hook_event_name": "PermissionRequest", "tool_name": tool, "tool_input": input}
    })
}

fn titles(frame: &Value) -> Vec<String> {
    frame["spec"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["title"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn with_agent_prompts_absent_or_native_nothing_is_asked_and_the_reply_is_declined() {
    for config in ["{}", r#"{"agent_prompts": "native"}"#] {
        let home = tempfile::tempdir().unwrap();
        let worker = open(home.path(), config);
        let mut follower = window(home.path());
        assert_eq!(
            call(home.path(), "agent_prompt.resolve", ask("claude")),
            json!({"status": "declined", "reason": "native"})
        );
        assert_eq!(
            call(
                home.path(),
                "agent_prompt.resolve",
                permission("Bash", json!({"command": "ls"}))
            ),
            json!({"status": "declined", "reason": "native"})
        );
        assert!(
            follower
                .read_stream_line(Duration::from_millis(200))
                .ok()
                .flatten()
                .is_none_or(|line| !line.contains("requested")),
            "no prompt reaches the window"
        );
        worker.stop();
    }
}

#[test]
fn every_question_is_asked_in_order_and_the_answers_become_the_hook_output() {
    let home = tempfile::tempdir().unwrap();
    let worker = open(home.path(), r#"{"agent_prompts": "paneflow"}"#);
    let mut follower = window(home.path());
    let resolving = resolve_later(home.path(), ask("claude"));
    let first = next_requested(&mut follower);
    assert_eq!(first["spec"]["kind"], "pick");
    assert_eq!(first["spec"]["title"], "Which color?");
    assert_eq!(first["spec"]["allow_other"], true);
    assert!(first["spec"].get("multiple").is_none());
    assert_eq!(titles(&first), ["Red", "Blue", "Answer in the terminal"]);
    assert_eq!(first["spec"]["items"][0]["subtitle"], "Warm");
    assert_eq!(first["origin"]["kind"], "agent");
    assert_eq!(first["origin"]["agent"], "Claude Code");
    reply(home.path(), &first["id"], json!("1"));
    let second = next_requested(&mut follower);
    assert_eq!(second["spec"]["title"], "Which sizes?");
    assert_eq!(second["spec"]["multiple"], true);
    assert!(
        second["spec"].get("allow_other").is_none(),
        "the broker takes typed text only for a single choice"
    );
    reply(home.path(), &second["id"], json!(["0", "2"]));
    assert_eq!(
        resolving.join().unwrap(),
        json!({"status": "answered", "output": {"hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "updatedInput": {
                "questions": questions(),
                "answers": {"Which color?": "Blue", "Which sizes?": "S, L"}
            }
        }}})
    );
    worker.stop();
}

#[test]
fn answering_in_the_terminal_or_cancelling_declines_and_asks_nothing_more() {
    let home = tempfile::tempdir().unwrap();
    let worker = open(home.path(), r#"{"agent_prompts": "paneflow"}"#);
    let mut follower = window(home.path());
    let resolving = resolve_later(home.path(), ask("claude"));
    let first = next_requested(&mut follower);
    reply(home.path(), &first["id"], json!("terminal"));
    assert_eq!(
        resolving.join().unwrap(),
        json!({"status": "declined", "reason": "terminal"})
    );
    let resolving = resolve_later(home.path(), ask("claude"));
    let first = next_requested(&mut follower);
    call(
        home.path(),
        "prompt.reply",
        json!({"id": first["id"], "cancel": true}),
    );
    assert_eq!(
        resolving.join().unwrap(),
        json!({"status": "declined", "reason": "cancelled"})
    );
    assert_eq!(
        call(home.path(), "agent_prompt.resolve", ask("codex")),
        json!({"status": "declined", "reason": "unsupported"}),
        "a Codex question is never intercepted"
    );
    worker.stop();
}

#[test]
fn with_no_window_the_reply_is_declined_at_once() {
    let home = tempfile::tempdir().unwrap();
    let worker = open(home.path(), r#"{"agent_prompts": "paneflow"}"#);
    assert_eq!(
        call(home.path(), "agent_prompt.resolve", ask("claude")),
        json!({"status": "declined", "reason": "no_window"})
    );
    worker.stop();
}

#[test]
fn a_permission_request_offers_allow_deny_or_the_terminal_titled_with_the_tool() {
    let home = tempfile::tempdir().unwrap();
    let worker = open(home.path(), r#"{"agent_prompts": "paneflow"}"#);
    let mut follower = window(home.path());
    let long = format!("echo {}\u{202e}", "x".repeat(600));
    for (choice, expected) in [
        (
            "allow",
            json!({"status": "answered", "output": {"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": {"behavior": "allow"}}}}),
        ),
        (
            "deny",
            json!({"status": "answered", "output": {"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": {"behavior": "deny", "message": "The user denied this in Paneflow"}}}}),
        ),
        (
            "terminal",
            json!({"status": "declined", "reason": "terminal"}),
        ),
    ] {
        let resolving = resolve_later(
            home.path(),
            permission("Bash", json!({"command": long, "description": "print"})),
        );
        let shown = next_requested(&mut follower);
        let title = shown["spec"]["title"].as_str().unwrap();
        let (tool, summary) = title.split_once('\n').unwrap();
        assert_eq!(tool, "Bash");
        assert_eq!(summary.chars().count(), 400);
        assert_eq!(
            titles(&shown),
            ["Allow once", "Deny", "Decide in the terminal"]
        );
        assert!(shown["spec"].get("allow_other").is_none());
        reply(home.path(), &shown["id"], json!(choice));
        assert_eq!(resolving.join().unwrap(), expected, "{choice}");
    }
    assert_eq!(
        call(
            home.path(),
            "agent_prompt.resolve",
            permission("AskUserQuestion", json!({"questions": questions()}))
        ),
        json!({"status": "declined", "reason": "unsupported"}),
        "the permission Claude Code raises after a declined question writes nothing"
    );
    worker.stop();
}
