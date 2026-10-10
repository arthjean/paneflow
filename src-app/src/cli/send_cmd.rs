use paneflow_ipc_client::IpcTransport;
use paneflow_ipc_client::host_control::session_id_from;
use serde_json::{Value, json};
use std::path::PathBuf;

use super::selector::{resolve_all, resolve_target};
use super::worker_state::{agent_runtime, with_host_foreground};
use super::{CliError, EXIT_OK, EXIT_RUNTIME};
use paneflow_agent_config::delivery::{DeliveryRefusal, TurnStart};

pub(super) use paneflow_agent_config::delivery::SUBMIT_START_TIMEOUT;
const CALLER_SESSION_ENV: &str = "PANEFLOW_SESSION_ID";

#[derive(Debug, Clone, Copy, Default)]
pub struct SendOptions {
    pub broadcast: bool,
    pub submit: bool,
    pub paste: bool,
    pub force: bool,
    pub scope_all: bool,
}

pub fn send(
    client: &impl IpcTransport,
    target: &str,
    text: &str,
    options: SendOptions,
    report_file: Option<&str>,
) -> Result<i32, CliError> {
    if options.broadcast && report_file.is_some() {
        return Err(CliError::runtime(
            "send --report-file cannot be combined with --broadcast; use one report file per target",
        ));
    }
    let report = report_file.map(report_contract).transpose()?;
    let text = match &report {
        Some(report) => prompt_with_report_contract(text, report),
        None => text.to_string(),
    };
    if options.broadcast {
        return send_broadcast(client, target, &text, options);
    }
    let surface_id = resolve_target(client, target)?;
    let mut result = send_to(client, surface_id, &text, options)?;
    if let Some(report) = report {
        result["report_file"] = json!(report.path);
        result["report_sentinel"] = json!(report.sentinel);
    }
    super::print_json(&result)?;
    if start_unconfirmed(&result) {
        eprintln!(
            "paneflow: the prompt was delivered to pane {surface_id}, but no turn start was confirmed within {}ms ({})",
            SUBMIT_START_TIMEOUT.as_millis(),
            result["reason"].as_str().unwrap_or_default()
        );
        return Ok(EXIT_RUNTIME);
    }
    Ok(EXIT_OK)
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

pub(super) fn caller_session() -> Option<String> {
    session_id_from(std::env::var(CALLER_SESSION_ENV).ok().as_deref())
}

pub(super) fn add_write_scope(params: &mut Value, caller: Option<String>, scope_all: bool) {
    if let Some(caller) = caller {
        params["scope_session"] = json!(caller);
    }
    if scope_all {
        params["scope"] = json!("all");
    }
}

pub(super) fn delivery_refusal(surface_id: u64, status: &Value) -> Option<String> {
    match paneflow_agent_config::delivery::delivery_refusal(status)? {
        DeliveryRefusal::Blocked { reason } => Some(format!(
            "surface {surface_id} is waiting for a decision ({reason}): answer in the pane or rerun with --force"
        )),
        DeliveryRefusal::LeftForeground { runtime, holder } => Some(format!(
            "surface {surface_id} no longer runs {runtime} in the foreground ({holder} has it): rerun with --force to write anyway"
        )),
    }
}

fn send_to(
    client: &impl IpcTransport,
    surface_id: u64,
    text: &str,
    options: SendOptions,
) -> Result<Value, CliError> {
    let mut before = status_of(client, surface_id);
    if !options.force
        && let Some(status) = before.as_mut()
        && agent_runtime(status).is_some()
    {
        with_host_foreground(status);
    }
    if !options.force
        && let Some(refusal) = before
            .as_ref()
            .and_then(|status| delivery_refusal(surface_id, status))
    {
        return Err(CliError::target(refusal));
    }
    let mut params = json!({ "surface_id": surface_id, "text": text, "submit": options.submit });
    if options.paste {
        params["paste"] = json!(true);
    }
    if options.force {
        params["force"] = json!(true);
    }
    add_write_scope(&mut params, caller_session(), options.scope_all);
    match client.call("surface.send_text", params) {
        Ok(result) => {
            let mut result = super::reject_legacy_error(result)?;
            if should_wait_for_submit_start(&result) {
                confirm_turn_start(client, surface_id, before.as_ref()).annotate(&mut result);
            }
            Ok(result)
        }
        Err(e) if is_send_text_disabled_error(&e) => Err(CliError::runtime(format!(
            "send is disabled on the running Paneflow instance; relaunch it with \
             PANEFLOW_IPC_SCRIPTING=1 to enable text injection (server said: {e})"
        ))),
        Err(e) => Err(CliError::runtime(e)),
    }
}

pub(super) fn status_of(client: &impl IpcTransport, surface_id: u64) -> Option<Value> {
    client
        .call("surface.status", json!({ "surface_id": surface_id }))
        .ok()
        .filter(|status| status.get("error").is_none())
}

pub(super) fn start_unconfirmed(result: &Value) -> bool {
    result.get("started") == Some(&Value::Bool(false))
}

pub(super) fn confirm_turn_start(
    client: &impl IpcTransport,
    surface_id: u64,
    before: Option<&Value>,
) -> TurnStart {
    paneflow_agent_config::delivery::confirm_turn_start(
        || status_of(client, surface_id),
        before,
        SUBMIT_START_TIMEOUT,
        paneflow_agent_config::delivery::SUBMIT_START_POLL,
    )
}

pub(super) fn should_wait_for_submit_start(result: &Value) -> bool {
    if !result["submitted"].as_bool().unwrap_or(false) {
        return false;
    }
    result["agent_target"].as_bool().unwrap_or(false)
        || result["submit_mode"].as_str() == Some("deferred_paste_cr")
}

fn send_broadcast(
    client: &impl IpcTransport,
    target: &str,
    text: &str,
    options: SendOptions,
) -> Result<i32, CliError> {
    let ids = resolve_all(client, target)?;
    let mut sent: Vec<u64> = Vec::new();
    let mut failed: Vec<Value> = Vec::new();
    for id in ids {
        match send_to(client, id, text, options) {
            Ok(result) if start_unconfirmed(&result) => failed.push(json!({
                "surface_id": id,
                "delivered": true,
                "started": false,
                "error": result["reason"],
            })),
            Ok(_) => sent.push(id),
            Err(e) if e.message.contains("PANEFLOW_IPC_SCRIPTING") && sent.is_empty() => {
                return Err(e);
            }
            Err(e) => failed.push(json!({ "surface_id": id, "error": e.message })),
        }
    }
    let all_ok = failed.is_empty();
    super::print_json(&json!({ "sent": sent, "failed": failed, "submitted": options.submit }))?;
    Ok(if all_ok { EXIT_OK } else { EXIT_RUNTIME })
}

pub fn key(
    client: &impl IpcTransport,
    target: &str,
    keystroke: &str,
    scope_all: bool,
) -> Result<i32, CliError> {
    let surface_id = resolve_target(client, target)?;
    let mut params = json!({ "surface_id": surface_id, "keystroke": keystroke });
    add_write_scope(&mut params, caller_session(), scope_all);
    match client.call("surface.send_keystroke", params) {
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
    use paneflow_agent_config::delivery::reports_turns;
    use std::cell::RefCell;

    struct ScriptedTransport {
        calls: RefCell<Vec<(String, Value)>>,
        replies: RefCell<Vec<Result<Value, String>>>,
        statuses: RefCell<Vec<Value>>,
    }
    impl ScriptedTransport {
        fn new(replies: Vec<Result<Value, String>>) -> Self {
            Self::with_statuses(replies, Vec::new())
        }

        fn with_statuses(replies: Vec<Result<Value, String>>, statuses: Vec<Value>) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                replies: RefCell::new(replies),
                statuses: RefCell::new(statuses),
            }
        }

        fn writes(&self) -> usize {
            self.calls
                .borrow()
                .iter()
                .filter(|(method, _)| method == "surface.send_text")
                .count()
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
                let mut statuses = self.statuses.borrow_mut();
                return Ok(match statuses.len() {
                    0 => json!({ "hooked": false, "output_generation": 0 }),
                    1 => statuses[0].clone(),
                    _ => statuses.remove(0),
                });
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

    fn opts(broadcast: bool, submit: bool, paste: bool) -> SendOptions {
        SendOptions {
            broadcast,
            submit,
            paste,
            ..SendOptions::default()
        }
    }

    fn agent_status(state: &str, state_seq: u64) -> Value {
        json!({
            "state": state,
            "state_seq": state_seq,
            "activity_source": "declared",
            "tool": "claude",
            "agent_runtime": "com.anthropic.claude-code",
            "foreground_runtime": "com.anthropic.claude-code",
            "output_generation": 40 + state_seq,
        })
    }

    fn agent_reply() -> Result<Value, String> {
        Ok(json!({
            "sent": true,
            "submitted": true,
            "agent_target": true,
            "paste": true,
            "submit_mode": "deferred_paste_cr"
        }))
    }

    #[test]
    fn send_passes_submit_flag_through() {
        let fake = ScriptedTransport::new(vec![Ok(json!({ "sent": true, "submitted": true }))]);
        assert_eq!(
            send(&fake, "shard-api", "run", opts(false, true, false), None).expect("ok"),
            EXIT_OK
        );
        let calls = fake.calls.borrow();
        let send_call = calls
            .iter()
            .find(|(method, _)| method == "surface.send_text")
            .expect("send_text call");
        assert_eq!(send_call.1["submit"], true);
        assert_eq!(send_call.1["surface_id"], 12);
        assert!(send_call.1.get("paste").is_none());
        assert!(send_call.1.get("force").is_none());
    }

    #[test]
    fn send_default_is_not_submitting() {
        let fake = ScriptedTransport::new(vec![Ok(json!({ "sent": true }))]);
        send(&fake, "shard-api", "run", opts(false, false, false), None).expect("ok");
        assert_eq!(fake.calls.borrow()[0].1["submit"], false);
    }

    #[test]
    fn paste_flag_is_forwarded_only_when_set() {
        let fake = ScriptedTransport::new(vec![Ok(json!({ "sent": true, "paste": true }))]);
        send(&fake, "shard-api", "hi", opts(false, true, true), None).expect("ok");
        let calls = fake.calls.borrow();
        let send_call = calls
            .iter()
            .find(|(method, _)| method == "surface.send_text")
            .expect("send_text call");
        assert_eq!(send_call.1["paste"], true);
        assert_eq!(send_call.1["submit"], true);
    }

    #[test]
    fn a_blocked_agent_receives_no_byte_and_fails_as_a_target_error() {
        let mut blocked = agent_status("waiting_for_input", 3);
        blocked["message"] = json!("Allow Bash(rm -rf build)?");
        let fake = ScriptedTransport::with_statuses(vec![agent_reply()], vec![blocked]);
        let err = send(&fake, "shard-api", "go on", opts(false, true, false), None)
            .expect_err("a blocked agent refuses the prompt");
        assert_eq!(err.code, crate::cli::EXIT_TARGET);
        assert!(
            err.message.contains("surface 12 is waiting for a decision"),
            "{}",
            err.message
        );
        assert!(err.message.contains("Allow Bash(rm -rf build)?"));
        assert!(err.message.contains("--force"));
        assert_eq!(fake.writes(), 0, "no byte reaches a blocked agent");
    }

    #[test]
    fn an_agent_that_left_the_foreground_refuses_the_prompt() {
        let mut departed = agent_status("finished", 4);
        departed["foreground_runtime"] = Value::Null;
        let fake = ScriptedTransport::with_statuses(vec![], vec![departed]);
        let err = send(&fake, "shard-api", "hello", opts(false, false, false), None)
            .expect_err("the agent no longer holds the pane");
        assert_eq!(err.code, crate::cli::EXIT_TARGET);
        assert!(
            err.message.contains("no longer runs Claude Code"),
            "{}",
            err.message
        );
        assert!(err.message.contains("a shell or another program"));
        assert_eq!(fake.writes(), 0);

        let mut editor = agent_status("finished", 4);
        editor["foreground_runtime"] = json!("com.example.not-in-catalog");
        assert!(
            delivery_refusal(12, &editor)
                .is_some_and(|refusal| refusal.contains("com.example.not-in-catalog"))
        );
    }

    #[test]
    fn force_bypasses_both_checks_and_says_so_to_the_server() {
        let blocked = agent_status("waiting_for_input", 3);
        let fake =
            ScriptedTransport::with_statuses(vec![Ok(json!({ "sent": true }))], vec![blocked]);
        let options = SendOptions {
            force: true,
            ..opts(false, false, false)
        };
        assert_eq!(
            send(&fake, "shard-api", "y", options, None).expect("forced"),
            EXIT_OK
        );
        let calls = fake.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].1["force"], true,
            "the server logs the forced write"
        );
    }

    #[test]
    fn a_plain_shell_is_never_held_by_the_agent_checks() {
        let shell =
            json!({ "state": "idle", "state_seq": 0, "hooked": false, "foreground_runtime": null });
        assert_eq!(delivery_refusal(12, &shell), None);
        let fake = ScriptedTransport::with_statuses(vec![], vec![shell]);
        send(&fake, "shard-api", "ls", opts(false, false, false), None).expect("shell write");
        assert_eq!(fake.writes(), 1);
    }

    #[test]
    fn a_turn_starts_only_on_a_state_transition() {
        let fake = ScriptedTransport::with_statuses(
            vec![agent_reply()],
            vec![
                agent_status("finished", 4),
                agent_status("finished", 4),
                agent_status("thinking", 5),
            ],
        );
        let result = send_to(&fake, 12, "hi", opts(false, true, false)).expect("started");
        assert_eq!(result["delivered"], true);
        assert_eq!(result["started"], true);
        assert_eq!(result["reason"], "state_transition");
        assert_eq!(result["state"], "working");
    }

    #[test]
    fn the_paste_echo_alone_never_confirms_a_start() {
        let mut echoed = agent_status("finished", 4);
        echoed["output_generation"] = json!(9_999);
        let fake = ScriptedTransport::with_statuses(
            vec![agent_reply()],
            vec![agent_status("finished", 4), echoed],
        );
        let result = send_to(&fake, 12, "hi", opts(false, true, false)).expect("delivered");
        assert_eq!(result["delivered"], true);
        assert_eq!(result["started"], false);
        assert_eq!(result["reason"], "no_state_transition");
        assert_eq!(result["state"], "idle");
    }

    #[test]
    fn an_unconfirmed_start_prints_the_report_and_exits_like_before() {
        let fake = ScriptedTransport::with_statuses(
            vec![agent_reply()],
            vec![agent_status("finished", 4)],
        );
        let code = send(&fake, "shard-api", "hi", opts(false, true, false), None)
            .expect("the delivery is reported, not raised");
        assert_eq!(code, EXIT_RUNTIME);
    }

    #[test]
    fn a_runtime_that_declares_nothing_reports_no_signal() {
        let quiet = json!({
            "state": "idle",
            "state_seq": 2,
            "agent_runtime": "com.sourcegraph.amp",
            "foreground_runtime": "com.sourcegraph.amp",
        });
        assert!(agent_runtime(&quiet).is_some() && !reports_turns(&quiet));
        let fake = ScriptedTransport::with_statuses(vec![agent_reply()], vec![quiet]);
        let result = send_to(&fake, 12, "hi", opts(false, true, false)).expect("delivered");
        assert_eq!(result["delivered"], true);
        assert_eq!(result["started"], Value::Null, "{result}");
        assert_eq!(result["reason"], "no_signal");
        assert_eq!(result["state"], "idle");
    }

    #[test]
    fn an_agent_that_blocks_right_after_the_submit_still_started() {
        let fake = ScriptedTransport::with_statuses(
            vec![agent_reply()],
            vec![
                agent_status("finished", 4),
                agent_status("waiting_for_input", 5),
            ],
        );
        let result = send_to(&fake, 12, "hi", opts(false, true, false)).expect("started");
        assert_eq!(result["started"], true);
        assert_eq!(result["state"], "blocked");
    }

    #[test]
    fn inline_submit_without_agent_hint_does_not_wait_for_start() {
        let fake = ScriptedTransport::new(vec![Ok(json!({
            "sent": true,
            "submitted": true,
            "agent_target": false,
            "submit_mode": "inline_cr"
        }))]);

        let result =
            send_to(&fake, 12, "hi", opts(false, true, false)).expect("inline shell submit is ok");
        assert!(result.get("started").is_none());
    }

    #[test]
    fn the_write_carries_the_calling_pane_and_the_widened_scope() {
        let mut params = json!({});
        add_write_scope(&mut params, Some("0a9e5266".to_string()), false);
        assert_eq!(params["scope_session"], "0a9e5266");
        assert!(params.get("scope").is_none());
        let mut widened = json!({});
        add_write_scope(&mut widened, None, true);
        assert!(widened.get("scope_session").is_none());
        assert_eq!(widened["scope"], "all");
    }

    #[test]
    fn send_multi_match_without_broadcast_is_target_error() {
        let fake = ScriptedTransport::new(vec![]);
        let err =
            send(&fake, "shard", "x", opts(false, false, false), None).expect_err("ambiguous");
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
            send(&fake, "shard", "x", opts(true, false, false), None).expect("ok"),
            EXIT_OK
        );
        let calls = fake.calls.borrow();
        let ids: Vec<&Value> = calls.iter().map(|(_, p)| &p["surface_id"]).collect();
        assert_eq!(ids, vec![&json!(12), &json!(18)]);
    }

    #[test]
    fn broadcast_partial_failure_serves_the_rest_and_exits_nonzero() {
        let fake = ScriptedTransport::new(vec![
            Ok(json!({ "error": "Surface not found" })),
            Ok(json!({ "sent": true })),
        ]);
        let code =
            send(&fake, "shard", "x", opts(true, false, false), None).expect("report, not abort");
        assert_eq!(code, EXIT_RUNTIME);
        assert_eq!(fake.calls.borrow().len(), 2, "second pane still served");
    }

    #[test]
    fn broadcast_no_match_is_target_error() {
        let fake = ScriptedTransport::new(vec![]);
        let err = send(&fake, "zzz", "x", opts(true, false, false), None).expect_err("no match");
        assert_eq!(err.code, crate::cli::EXIT_TARGET);
        assert!(fake.calls.borrow().is_empty(), "no partial send");
    }

    #[test]
    fn broadcast_gate_off_aborts_with_actionable_hint() {
        let fake = ScriptedTransport::new(vec![Err(
            "server error -32601: surface.send_text disabled".to_string(),
        )]);
        let err = send(&fake, "shard", "x", opts(true, false, false), None).expect_err("gate off");
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
                opts(false, false, false),
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
            opts(true, false, false),
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
        let err = key(&fake, "shard-api", "escape", false).expect_err("gate off");
        assert!(err.message.contains("PANEFLOW_IPC_SCRIPTING"));

        let fake = ScriptedTransport::new(vec![Ok(json!({ "sent": true }))]);
        assert_eq!(
            key(&fake, "shard-api", "escape", true).expect("ok"),
            EXIT_OK
        );
        let calls = fake.calls.borrow();
        assert_eq!(calls[0].0, "surface.send_keystroke");
        assert_eq!(calls[0].1["keystroke"], "escape");
        assert_eq!(calls[0].1["scope"], "all");
    }

    #[test]
    fn key_enter_refusal_is_nonzero_exit() {
        let fake = ScriptedTransport::new(vec![Ok(
            json!({ "error": "keystroke 'enter' would submit (CR/LF); use surface.send_text with submit=true (`paneflow send --submit`) instead" }),
        )]);
        let err = key(&fake, "shard-api", "enter", false).expect_err("refused");
        assert_eq!(err.code, EXIT_RUNTIME);
        assert!(err.message.contains("send --submit"), "hint present");
    }

    #[test]
    fn submit_forwards_a_full_64_kib_payload_intact() {
        let payload = "x".repeat(64 * 1024);
        let fake = ScriptedTransport::new(vec![Ok(json!({
            "sent": true, "length": payload.len(), "submitted": true
        }))]);
        assert_eq!(
            send(&fake, "shard-api", &payload, opts(false, true, false), None).expect("ok"),
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
