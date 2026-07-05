//! `send` / `key` - write-side CLI verbs (US-006; orchestration-v2
//! US-003/US-004/US-005).
//!
//! Wraps `surface.send_text` / `surface.send_keystroke`. The human-in-loop
//! invariant is enforced server side: `send_text` writes text with no trailing
//! carriage return, and rejects raw CR/LF unless the target has active bracketed
//! paste, so the user/agent presses Enter themselves - UNLESS `--submit` is
//! passed explicitly (US-005), the only sanctioned submission path.
//! `key` refuses submitting keystrokes (`enter`, `ctrl-m`, …) server-side.
//!
//! Both methods are gated behind `PANEFLOW_IPC_SCRIPTING=1` on the RUNNING
//! instance (the gate is read from Paneflow's own process env, not the
//! CLI's), so when it is off the server returns `-32601` and we translate
//! that into an actionable hint rather than a bare JSON-RPC code.

use paneflow_ipc_client::IpcTransport;
use serde_json::json;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::selector::{resolve_all, resolve_target};
use super::{CliError, EXIT_OK, EXIT_RUNTIME};

pub(super) const SUBMIT_START_TIMEOUT: Duration = Duration::from_millis(3000);
const SUBMIT_START_POLL: Duration = Duration::from_millis(60);

/// `paneflow send <target> <text> [--broadcast] [--submit] [--paste]`.
pub fn send(
    client: &impl IpcTransport,
    target: &str,
    text: &str,
    broadcast: bool,
    submit: bool,
    paste: bool,
    report_file: Option<&str>,
) -> Result<i32, CliError> {
    if broadcast && report_file.is_some() {
        return Err(CliError::runtime(
            "send --report-file cannot be combined with --broadcast; use one report file per target",
        ));
    }
    let report = report_file.map(report_contract).transpose()?;
    let text = match &report {
        Some(report) => prompt_with_report_contract(text, report),
        None => text.to_string(),
    };
    if broadcast {
        return send_broadcast(client, target, &text, submit, paste);
    }
    let surface_id = resolve_target(client, target)?;
    match send_to(client, surface_id, &text, submit, paste) {
        Ok(mut result) => {
            if let Some(report) = report {
                result["report_file"] = json!(report.path);
                result["report_sentinel"] = json!(report.sentinel);
            }
            super::print_json(&result)?;
            Ok(EXIT_OK)
        }
        Err(e) => Err(e),
    }
}

struct ReportContract {
    path: String,
    sentinel: String,
}

fn report_contract(path: &str) -> Result<ReportContract, CliError> {
    if path.trim().is_empty() {
        return Err(CliError::runtime(
            "send --report-file requires a non-empty path",
        ));
    }
    let path = PathBuf::from(path);
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|e| CliError::runtime(format!("cannot resolve current directory: {e}")))?
            .join(path)
    };
    let path = absolute.display().to_string();
    Ok(ReportContract {
        sentinel: format!("REPORT_DONE {path}"),
        path,
    })
}

fn prompt_with_report_contract(text: &str, report: &ReportContract) -> String {
    let mut prompt = String::with_capacity(text.len() + report.path.len() * 2 + 256);
    prompt.push_str(text);
    if !text.ends_with('\n') {
        prompt.push('\n');
    }
    prompt.push_str(
        "\nPaneflow report protocol:\n\
         - Write the complete final answer/report to this exact UTF-8 text file, overwriting it if it exists:\n",
    );
    prompt.push_str(&report.path);
    prompt.push_str(
        "\n- When the file is fully written and closed, print exactly this single line to the terminal:\n",
    );
    prompt.push_str(&report.sentinel);
    prompt.push_str("\n- Do not rely on terminal scrollback as the report channel.\n");
    prompt
}

/// One `surface.send_text` round for a resolved surface. Maps the legacy
/// `{"error": …}` result shape (empty text / >64 KiB / surface gone) to a
/// non-zero `CliError` (US-006 AC3) and the `-32601` gate-off reply to an
/// actionable hint.
fn send_to(
    client: &impl IpcTransport,
    surface_id: u64,
    text: &str,
    submit: bool,
    paste: bool,
) -> Result<serde_json::Value, CliError> {
    let before = if submit {
        status_snapshot(client, surface_id)
    } else {
        None
    };
    let mut params = json!({ "surface_id": surface_id, "text": text, "submit": submit });
    // EP-001 US-002: only forward `paste` when the user explicitly passed
    // `--paste`. Absent, the server auto-decides (agent pane -> bracketed paste
    // + deferred submit, bare shell -> verbatim); sending an explicit `false`
    // here would instead PIN the verbatim path and defeat the auto-detection.
    if paste {
        params["paste"] = json!(true);
    }
    match client.call("surface.send_text", params) {
        Ok(result) => {
            let mut result = super::reject_legacy_error(result)?;
            if should_wait_for_submit_start(&result) {
                match wait_for_submit_start(client, surface_id, before.as_ref()) {
                    SubmitStart::Confirmed(reason) => {
                        result["started"] = json!(true);
                        result["start_reason"] = json!(reason);
                    }
                    SubmitStart::Unconfirmed(reason) => {
                        result["started"] = json!(false);
                        result["start_reason"] = json!(reason);
                        return Err(CliError::runtime(format!(
                            "submit was written to agent pane {surface_id}, but no turn start was confirmed within {}ms ({reason})",
                            SUBMIT_START_TIMEOUT.as_millis()
                        )));
                    }
                }
            }
            Ok(result)
        }
        // The scripting gate is off on the running instance.
        Err(e) if is_send_text_disabled_error(&e) => Err(CliError::runtime(format!(
            "send is disabled on the running Paneflow instance; relaunch it with \
             PANEFLOW_IPC_SCRIPTING=1 to enable text injection (server said: {e})"
        ))),
        Err(e) => Err(CliError::runtime(e)),
    }
}

pub(super) enum SubmitStart {
    Confirmed(&'static str),
    Unconfirmed(&'static str),
}

pub(super) struct StatusSnapshot {
    state: String,
    output_generation: Option<u64>,
}

pub(super) fn status_snapshot(
    client: &impl IpcTransport,
    surface_id: u64,
) -> Option<StatusSnapshot> {
    let v = client
        .call("surface.status", json!({ "surface_id": surface_id }))
        .ok()?;
    Some(StatusSnapshot {
        state: v
            .get("state")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("idle")
            .to_string(),
        output_generation: v
            .get("output_generation")
            .and_then(serde_json::Value::as_u64),
    })
}

pub(super) fn wait_for_submit_start(
    client: &impl IpcTransport,
    surface_id: u64,
    before: Option<&StatusSnapshot>,
) -> SubmitStart {
    let deadline = Instant::now() + SUBMIT_START_TIMEOUT;
    loop {
        if let Ok(status) = client.call("surface.status", json!({ "surface_id": surface_id })) {
            let state = status
                .get("state")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("idle");
            let hooked = status
                .get("hooked")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let state_changed = before.is_none_or(|b| b.state != state);
            if hooked
                && state_changed
                && matches!(state, "thinking" | "waiting_for_input" | "finished")
            {
                return SubmitStart::Confirmed("hook_state_changed");
            }
            let generation_changed =
                before
                    .and_then(|b| b.output_generation)
                    .is_some_and(|baseline| {
                        status
                            .get("output_generation")
                            .and_then(serde_json::Value::as_u64)
                            .is_some_and(|generation| generation > baseline)
                    });
            if generation_changed {
                return SubmitStart::Confirmed("output_generation_changed");
            }
        }
        if Instant::now() >= deadline {
            return SubmitStart::Unconfirmed("no_hook_state_or_output_confirmation");
        }
        std::thread::sleep(SUBMIT_START_POLL);
    }
}

pub(super) fn should_wait_for_submit_start(result: &serde_json::Value) -> bool {
    if !result["submitted"].as_bool().unwrap_or(false) {
        return false;
    }
    result["agent_target"].as_bool().unwrap_or(false)
        || result["submit_mode"].as_str() == Some("deferred_paste_cr")
}

/// `send --broadcast`: every pane matching the selector gets the text. A pane
/// failing mid-loop (closed between resolve and send) does not abort the
/// rest; the report lists both sets and the exit is non-zero when anything
/// failed (US-003). The gate-off error DOES abort: every send would fail the
/// same way, so the actionable hint surfaces immediately instead of N times.
fn send_broadcast(
    client: &impl IpcTransport,
    target: &str,
    text: &str,
    submit: bool,
    paste: bool,
) -> Result<i32, CliError> {
    let ids = resolve_all(client, target)?;
    let mut sent: Vec<u64> = Vec::new();
    let mut failed: Vec<serde_json::Value> = Vec::new();
    for id in ids {
        match send_to(client, id, text, submit, paste) {
            Ok(_) => sent.push(id),
            // Gate off: abort with the hint; nothing was partially injected
            // (the very first call already failed the same way).
            Err(e) if e.message.contains("PANEFLOW_IPC_SCRIPTING") && sent.is_empty() => {
                return Err(e);
            }
            Err(e) => failed.push(json!({ "surface_id": id, "error": e.message })),
        }
    }
    let all_ok = failed.is_empty();
    super::print_json(&json!({ "sent": sent, "failed": failed, "submitted": submit }))?;
    Ok(if all_ok { EXIT_OK } else { EXIT_RUNTIME })
}

/// `paneflow key <target> <keystroke>`. Wraps `surface.send_keystroke`; the
/// server refuses CR/LF-resolving keystrokes (`enter`, `ctrl-m`, `ctrl-j`) so
/// submission stays exclusive to `send --submit` (US-004/US-005).
pub fn key(client: &impl IpcTransport, target: &str, keystroke: &str) -> Result<i32, CliError> {
    let surface_id = resolve_target(client, target)?;
    match client.call(
        "surface.send_keystroke",
        json!({ "surface_id": surface_id, "keystroke": keystroke }),
    ) {
        Ok(result) => {
            let result = super::reject_legacy_error(result)?;
            super::print_json(&result)?;
            Ok(EXIT_OK)
        }
        Err(e) if is_send_keystroke_disabled_error(&e) => Err(CliError::runtime(format!(
            "key is disabled on the running Paneflow instance; relaunch it with \
             PANEFLOW_IPC_SCRIPTING=1 to enable keystroke injection (server said: {e})"
        ))),
        Err(e) => Err(CliError::runtime(e)),
    }
}

pub(super) fn is_send_text_disabled_error(error: &str) -> bool {
    method_disabled_error(error, "surface.send_text")
}

fn is_send_keystroke_disabled_error(error: &str) -> bool {
    method_disabled_error(error, "surface.send_keystroke")
}

fn method_disabled_error(error: &str, method: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("-32601") && lower.contains(method) && lower.contains("disabled")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::cell::RefCell;

    /// Method-routed fake: fixed `surface.list`, scripted per-call replies for
    /// the write methods (popped front-to-back), and a (method, params) log.
    struct ScriptedTransport {
        calls: RefCell<Vec<(String, Value)>>,
        replies: RefCell<Vec<Result<Value, String>>>,
    }
    impl ScriptedTransport {
        fn new(replies: Vec<Result<Value, String>>) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                replies: RefCell::new(replies),
            }
        }
    }
    impl IpcTransport for ScriptedTransport {
        fn call(&self, method: &str, params: Value) -> Result<Value, String> {
            if method == "surface.list" {
                return Ok(json!({ "surfaces": [
                    { "surface_id": 12, "name": "shard-api" },
                    { "surface_id": 18, "name": "shard-ui" },
                ]}));
            }
            if method == "surface.status" {
                self.calls
                    .borrow_mut()
                    .push((method.to_string(), params.clone()));
                let mut replies = self.replies.borrow_mut();
                let scripted_status = replies.first().is_some_and(|r| {
                    r.as_ref().is_ok_and(|v| {
                        v.get("state").is_some()
                            || v.get("hooked").is_some()
                            || v.get("output_generation").is_some()
                    })
                });
                if scripted_status {
                    return replies.remove(0);
                }
                return Ok(json!({
                    "state": "idle",
                    "hooked": false,
                    "output_generation": 0
                }));
            }
            self.calls
                .borrow_mut()
                .push((method.to_string(), params.clone()));
            let mut replies = self.replies.borrow_mut();
            if replies.is_empty() {
                return Ok(json!({ "sent": true }));
            }
            replies.remove(0)
        }
    }

    #[test]
    fn send_passes_submit_flag_through() {
        let fake = ScriptedTransport::new(vec![Ok(json!({ "sent": true, "submitted": true }))]);
        assert_eq!(
            send(&fake, "shard-api", "run", false, true, false, None).expect("ok"),
            EXIT_OK
        );
        let calls = fake.calls.borrow();
        let send_call = calls
            .iter()
            .find(|(method, _)| method == "surface.send_text")
            .expect("send_text call");
        assert_eq!(send_call.1["submit"], true);
        assert_eq!(send_call.1["surface_id"], 12);
        // EP-001 US-002: no `--paste` -> the key is omitted so the server
        // auto-decides agent-vs-shell rather than being pinned to verbatim.
        assert!(send_call.1.get("paste").is_none());
    }

    #[test]
    fn send_default_is_not_submitting() {
        let fake = ScriptedTransport::new(vec![Ok(json!({ "sent": true }))]);
        send(&fake, "shard-api", "run", false, false, false, None).expect("ok");
        assert_eq!(fake.calls.borrow()[0].1["submit"], false);
    }

    #[test]
    fn paste_flag_is_forwarded_only_when_set() {
        // EP-001 US-002 AC2: `--paste` -> the param rides through to the server.
        let fake = ScriptedTransport::new(vec![Ok(json!({ "sent": true, "paste": true }))]);
        send(&fake, "shard-api", "hi", false, true, true, None).expect("ok");
        let calls = fake.calls.borrow();
        let send_call = calls
            .iter()
            .find(|(method, _)| method == "surface.send_text")
            .expect("send_text call");
        assert_eq!(send_call.1["paste"], true);
        assert_eq!(send_call.1["submit"], true);
    }

    #[test]
    fn submit_to_agent_waits_for_hook_state_start() {
        let fake = ScriptedTransport::new(vec![
            Ok(json!({ "state": "idle", "hooked": true, "output_generation": 1 })),
            Ok(json!({
                "sent": true,
                "submitted": true,
                "agent_target": true,
                "paste": true
            })),
            Ok(json!({ "state": "thinking", "hooked": true, "output_generation": 2 })),
        ]);
        assert_eq!(
            send(&fake, "shard-api", "hi", false, true, false, None).expect("ok"),
            EXIT_OK
        );
        let calls = fake.calls.borrow();
        assert!(
            calls.iter().any(|(method, _)| method == "surface.status"),
            "submit start verification probes status"
        );
    }

    #[test]
    fn submit_to_agent_accepts_output_generation_start_confirmation() {
        let fake = ScriptedTransport::new(vec![
            Ok(json!({ "state": "idle", "hooked": false, "output_generation": 41 })),
            Ok(json!({
                "sent": true,
                "submitted": true,
                "agent_target": true,
                "paste": true
            })),
            Ok(json!({ "state": "idle", "hooked": false, "output_generation": 42 })),
        ]);
        let result = send_to(&fake, 12, "hi", true, false).expect("output confirms start");
        assert_eq!(result["started"], true);
        assert_eq!(result["start_reason"], "output_generation_changed");
    }

    #[test]
    fn deferred_paste_submit_without_agent_hint_still_waits_for_start() {
        let fake = ScriptedTransport::new(vec![
            Ok(json!({ "state": "idle", "hooked": false, "output_generation": 10 })),
            Ok(json!({
                "sent": true,
                "submitted": true,
                "agent_target": false,
                "paste": true,
                "submit_mode": "deferred_paste_cr",
                "terminal_bracketed_paste": true
            })),
            Ok(json!({ "state": "idle", "hooked": false, "output_generation": 11 })),
        ]);

        let result = send_to(&fake, 12, "hi", true, false).expect("output confirms start");
        assert_eq!(result["started"], true);
        assert_eq!(result["start_reason"], "output_generation_changed");
    }

    #[test]
    fn inline_submit_without_agent_hint_does_not_wait_for_start() {
        let fake = ScriptedTransport::new(vec![Ok(json!({
            "sent": true,
            "submitted": true,
            "agent_target": false,
            "submit_mode": "inline_cr"
        }))]);

        let result = send_to(&fake, 12, "hi", true, false).expect("inline shell submit is ok");
        assert!(result.get("started").is_none());
        assert_eq!(
            fake.calls
                .borrow()
                .iter()
                .filter(|(method, _)| method == "surface.status")
                .count(),
            1,
            "only the pre-submit snapshot should run"
        );
    }

    #[test]
    fn send_multi_match_without_broadcast_is_target_error() {
        // "shard" prefixes both panes: single-target send must refuse, not
        // pick one silently (US-003 keeps the existing single semantics).
        let fake = ScriptedTransport::new(vec![]);
        let err = send(&fake, "shard", "x", false, false, false, None).expect_err("ambiguous");
        assert_eq!(err.code, crate::cli::EXIT_TARGET);
        assert!(fake.calls.borrow().is_empty());
    }

    #[test]
    fn broadcast_hits_every_match() {
        let fake = ScriptedTransport::new(vec![
            Ok(json!({ "sent": true })),
            Ok(json!({ "sent": true })),
        ]);
        assert_eq!(
            send(&fake, "shard", "x", true, false, false, None).expect("ok"),
            EXIT_OK
        );
        let calls = fake.calls.borrow();
        let ids: Vec<&Value> = calls.iter().map(|(_, p)| &p["surface_id"]).collect();
        assert_eq!(ids, vec![&json!(12), &json!(18)]);
    }

    #[test]
    fn broadcast_partial_failure_serves_the_rest_and_exits_nonzero() {
        // First pane vanished mid-loop (legacy error shape): the second pane
        // must still be served and the exit must be non-zero (US-003 AC4).
        let fake = ScriptedTransport::new(vec![
            Ok(json!({ "error": "Surface not found" })),
            Ok(json!({ "sent": true })),
        ]);
        let code = send(&fake, "shard", "x", true, false, false, None).expect("report, not abort");
        assert_eq!(code, EXIT_RUNTIME);
        assert_eq!(fake.calls.borrow().len(), 2, "second pane still served");
    }

    #[test]
    fn broadcast_no_match_is_target_error() {
        let fake = ScriptedTransport::new(vec![]);
        let err = send(&fake, "zzz", "x", true, false, false, None).expect_err("no match");
        assert_eq!(err.code, crate::cli::EXIT_TARGET);
        assert!(fake.calls.borrow().is_empty(), "no partial send");
    }

    #[test]
    fn broadcast_gate_off_aborts_with_actionable_hint() {
        let fake = ScriptedTransport::new(vec![Err(
            "server error -32601: surface.send_text disabled".to_string(),
        )]);
        let err = send(&fake, "shard", "x", true, false, false, None).expect_err("gate off");
        assert_eq!(err.code, EXIT_RUNTIME);
        assert!(err.message.contains("PANEFLOW_IPC_SCRIPTING"));
        assert_eq!(fake.calls.borrow().len(), 1, "aborted after first reply");
    }

    #[test]
    fn gate_hint_requires_the_specific_disabled_method() {
        assert!(is_send_text_disabled_error(
            "server error -32601: surface.send_text disabled"
        ));
        assert!(!is_send_text_disabled_error(
            "server error -32601: Method not found"
        ));
        assert!(!is_send_text_disabled_error(
            "server error -32601: surface.send_keystroke disabled"
        ));
    }

    #[test]
    fn report_file_adds_file_contract_to_sent_prompt() {
        let fake = ScriptedTransport::new(vec![Ok(json!({ "sent": true }))]);
        assert_eq!(
            send(
                &fake,
                "shard-api",
                "audit the system",
                false,
                false,
                false,
                Some("reports/out.md"),
            )
            .expect("ok"),
            EXIT_OK
        );
        let calls = fake.calls.borrow();
        let send_call = calls
            .iter()
            .find(|(method, _)| method == "surface.send_text")
            .expect("send_text call");
        let text = send_call.1["text"].as_str().unwrap();
        assert!(text.contains("Paneflow report protocol"));
        assert!(text.contains("REPORT_DONE"));
        assert!(text.contains("reports"));
    }

    #[test]
    fn report_file_refuses_broadcast_collision() {
        let fake = ScriptedTransport::new(vec![]);
        let err = send(
            &fake,
            "shard",
            "audit",
            true,
            false,
            false,
            Some("reports/out.md"),
        )
        .expect_err("one report file cannot serve multiple panes");
        assert_eq!(err.code, EXIT_RUNTIME);
        assert!(err.message.contains("--broadcast"));
        assert!(fake.calls.borrow().is_empty());
    }

    #[test]
    fn key_translates_gate_off_and_passes_keystroke() {
        let fake = ScriptedTransport::new(vec![Err(
            "server error -32601: surface.send_keystroke disabled".to_string(),
        )]);
        let err = key(&fake, "shard-api", "escape").expect_err("gate off");
        assert!(err.message.contains("PANEFLOW_IPC_SCRIPTING"));

        let fake = ScriptedTransport::new(vec![Ok(json!({ "sent": true }))]);
        assert_eq!(key(&fake, "shard-api", "escape").expect("ok"), EXIT_OK);
        let calls = fake.calls.borrow();
        assert_eq!(calls[0].0, "surface.send_keystroke");
        assert_eq!(calls[0].1["keystroke"], "escape");
    }

    #[test]
    fn key_enter_refusal_is_nonzero_exit() {
        // The server refuses submitting keystrokes with a legacy error shape
        // (TerminalView::send_keystroke -> {"error": …}); the CLI must exit
        // non-zero and surface the `send --submit` hint (US-004 AC3).
        let fake = ScriptedTransport::new(vec![Ok(
            json!({ "error": "keystroke 'enter' would submit (CR/LF); use surface.send_text with submit=true (`paneflow send --submit`) instead" }),
        )]);
        let err = key(&fake, "shard-api", "enter").expect_err("refused");
        assert_eq!(err.code, EXIT_RUNTIME);
        assert!(err.message.contains("send --submit"), "hint present");
    }

    #[test]
    fn submit_forwards_a_full_64_kib_payload_intact() {
        // US-005 AC6: the one explicitly-mandated stress case - a 64 KiB text
        // submitted in a single round. The server enforces the 64 KiB ceiling
        // and appends the `\r` after the last byte (ipc_handler.rs:1344-1363);
        // the CLI's job, pinned here, is to forward the max-size payload WHOLE
        // (no client-side chunking/truncation) with `submit:true`, so the lone
        // server-side CR lands after the complete text rather than mid-stream.
        let payload = "x".repeat(64 * 1024);
        let fake = ScriptedTransport::new(vec![Ok(json!({
            "sent": true, "length": payload.len(), "submitted": true
        }))]);
        assert_eq!(
            send(&fake, "shard-api", &payload, false, true, false, None).expect("ok"),
            EXIT_OK
        );
        let calls = fake.calls.borrow();
        let send_call = calls
            .iter()
            .find(|(method, _)| method == "surface.send_text")
            .expect("send_text call");
        assert_eq!(send_call.1["submit"], true);
        assert_eq!(
            send_call.1["text"].as_str().map(str::len),
            Some(64 * 1024),
            "the 64 KiB payload must reach the server intact, not chunked"
        );
    }
}
