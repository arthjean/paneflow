#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_in_result
)]

mod pane;

use std::path::{Path, PathBuf};
use std::time::Duration;

use paneflow_ipc_client::host_control::HostControl;
use paneflow_serve::protocol::{ERR_INVALID_PARAMS, WORKER_PROTOCOL_VERSION};
use serde_json::{Value, json};

const CONFORMANCE: &str = include_str!("../../../protocol/prompt-conformance-v1.json");

const FRAME_WAIT: Duration = Duration::from_secs(15);
const REPLY_WAIT: Duration = Duration::from_secs(60);
const DISCONNECT_NOTICED: Duration = Duration::from_secs(5);

fn listed_cases() -> Vec<String> {
    let document: Value = serde_json::from_str(CONFORMANCE).expect("the conformance file parses");
    assert_eq!(document["protocol"], "paneflow-worker");
    assert_eq!(
        document["version"].as_u64(),
        Some(u64::from(WORKER_PROTOCOL_VERSION))
    );
    document["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .map(|case| case["id"].as_str().expect("an id").to_owned())
        .collect()
}

fn request(endpoint: &Path, method: &str, params: Value) -> Result<Value, String> {
    HostControl::connect(endpoint, "prompt-conformance")
        .expect("connects")
        .request_with_deadline(method, params, REPLY_WAIT)
}

fn ask(endpoint: &Path, params: Value) -> Value {
    request(endpoint, "prompt.request", params).expect("prompt.request answers")
}

fn ask_later(endpoint: &Path, params: Value) -> std::thread::JoinHandle<Value> {
    let endpoint = endpoint.to_path_buf();
    std::thread::spawn(move || ask(&endpoint, params))
}

fn input(title: &str, wait_ms: u64) -> Value {
    json!({"kind": "input", "title": title, "wait_ms": wait_ms})
}

fn error_code(error: &str) -> i64 {
    error
        .split("paneflow error ")
        .nth(1)
        .and_then(|rest| rest.split(':').next())
        .and_then(|code| code.trim().parse().ok())
        .unwrap_or_else(|| panic!("no error code in {error}"))
}

struct Follower {
    control: HostControl,
    header: Value,
}

impl Follower {
    fn open(endpoint: &Path, kinds: &[&str]) -> Self {
        let mut control = HostControl::connect(endpoint, "prompt-follower").expect("connects");
        let id = control
            .write_request("prompt.follow", json!({"kinds": kinds}))
            .expect("the follow request is written");
        loop {
            let line = control
                .read_stream_line(FRAME_WAIT)
                .expect("the worker answers")
                .expect("the stream stays open");
            let envelope: Value = serde_json::from_str(&line).expect("a JSON frame");
            if envelope["id"].as_u64() != Some(id) {
                continue;
            }
            assert!(
                envelope["error"].is_null(),
                "prompt.follow refused: {envelope}"
            );
            assert_eq!(envelope["result"]["type"], "following");
            return Self {
                control,
                header: envelope["result"].clone(),
            };
        }
    }

    fn frame(&mut self) -> Value {
        loop {
            let line = self
                .control
                .read_stream_line(FRAME_WAIT)
                .expect("a frame arrives")
                .expect("the stream stays open");
            let frame: Value = serde_json::from_str(&line).expect("a JSON frame");
            if frame["type"] != "keepalive" {
                return frame;
            }
        }
    }

    fn requested(&mut self) -> Value {
        let frame = self.frame();
        assert_eq!(frame["type"], "requested", "{frame}");
        frame
    }

    fn ended(&mut self) -> Value {
        let frame = self.frame();
        assert_eq!(frame["type"], "ended", "{frame}");
        frame
    }
}

fn reply(endpoint: &Path, id: &Value, answer: Value) -> Result<Value, String> {
    request(
        endpoint,
        "prompt.reply",
        json!({"id": id, "answer": answer}),
    )
}

fn cancel(endpoint: &Path, id: &Value) {
    request(endpoint, "prompt.reply", json!({"id": id, "cancel": true})).expect("cancelled");
}

struct Running {
    _home: tempfile::TempDir,
    worker: Option<paneflow_serve::worker::RunningWorker>,
    endpoint: PathBuf,
}

fn start() -> Running {
    let home = tempfile::tempdir().unwrap();
    let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");
    let endpoint = paneflow_home::serve_endpoint_path(home.path());
    Running {
        _home: home,
        worker: Some(worker),
        endpoint,
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.stop();
        }
    }
}

fn case_first_reply_wins(endpoint: &Path) -> &'static str {
    let mut first = Follower::open(endpoint, &["input"]);
    let mut second = Follower::open(endpoint, &["input", "pick"]);
    let asker = ask_later(endpoint, input("Branch", 25_000));
    let shown = first.requested();
    assert_eq!(second.requested()["id"], shown["id"]);
    assert_eq!(shown["spec"]["title"], "Branch");
    let replied = reply(endpoint, &shown["id"], json!("fix/login")).unwrap();
    assert_eq!(replied["reason"], "answered");
    let outcome = asker.join().unwrap();
    assert_eq!(outcome["status"], "answered");
    assert_eq!(outcome["answer"], "fix/login");
    let ended = second.ended();
    assert_eq!(ended["id"], shown["id"]);
    assert_eq!(ended["reason"], "answered");
    assert_eq!(first.ended()["reason"], "answered");
    let late = reply(endpoint, &shown["id"], json!("too late")).unwrap_err();
    assert_eq!(error_code(&late), -32060, "{late}");
    assert!(late.contains("not_pending"), "{late}");
    "first_reply_wins_and_the_other_follower_receives_ended"
}

fn case_cancel(endpoint: &Path) -> &'static str {
    let mut first = Follower::open(endpoint, &["input"]);
    let mut second = Follower::open(endpoint, &["input"]);
    let asker = ask_later(endpoint, input("Name", 25_000));
    let shown = first.requested();
    second.requested();
    cancel(endpoint, &shown["id"]);
    assert_eq!(asker.join().unwrap()["status"], "cancelled");
    assert_eq!(second.ended()["reason"], "cancelled");
    "a_cancel_ends_the_prompt_for_the_other_follower"
}

fn case_unsupported(endpoint: &Path) -> &'static str {
    let _inputs = Follower::open(endpoint, &["input"]);
    let outcome = ask(
        endpoint,
        json!({"kind": "pick", "title": "Project", "items": ["api", "web"]}),
    );
    assert_eq!(outcome, json!({"status": "unsupported"}));
    let late = Follower::open(endpoint, &["input", "pick"]);
    assert_eq!(late.header["pending"], 0, "nothing was queued");
    "a_kind_no_follower_shows_is_unsupported"
}

fn case_follower_leaves(endpoint: &Path) -> &'static str {
    let mut staying = Follower::open(endpoint, &["input"]);
    let mut leaving = Follower::open(endpoint, &["input"]);
    let asker = ask_later(endpoint, input("Name", 25_000));
    let shown = staying.requested();
    leaving.requested();
    drop(leaving);
    std::thread::sleep(DISCONNECT_NOTICED);
    reply(endpoint, &shown["id"], json!("kept")).unwrap();
    assert_eq!(asker.join().unwrap()["answer"], "kept");
    "a_follower_leaving_while_another_stays_keeps_the_prompt_pending"
}

fn case_late_follower(endpoint: &Path) -> &'static str {
    let _early = Follower::open(endpoint, &["input", "pick"]);
    let ids: Vec<Value> = ["first", "second", "third"]
        .iter()
        .map(|title| ask(endpoint, input(title, 0))["id"].clone())
        .collect();
    let mut late = Follower::open(endpoint, &["input", "pick"]);
    assert_eq!(late.header["pending"], 3);
    let seen: Vec<Value> = (0..3).map(|_| late.requested()["id"].clone()).collect();
    assert_eq!(seen, ids);
    for id in &ids {
        cancel(endpoint, id);
    }
    "a_late_follower_receives_the_pending_prompts_oldest_first"
}

fn case_no_client(endpoint: &Path) -> &'static str {
    assert_eq!(
        ask(endpoint, input("Alone", 0)),
        json!({"status": "no_client"})
    );
    let mut only = Follower::open(endpoint, &["input"]);
    let asker = ask_later(endpoint, input("Name", 25_000));
    only.requested();
    drop(only);
    let outcome = asker.join().unwrap();
    assert_eq!(outcome["status"], "no_client", "{outcome}");
    "no_client_with_no_follower_and_when_the_last_one_leaves"
}

fn case_pending_then_wait(endpoint: &Path) -> &'static str {
    let mut follower = Follower::open(endpoint, &["input"]);
    let pending = ask(endpoint, input("Later", 50));
    assert_eq!(pending["status"], "pending");
    let id = pending["id"].clone();
    assert_eq!(follower.requested()["id"], id);
    let waiting = {
        let endpoint = endpoint.to_path_buf();
        let id = id.clone();
        std::thread::spawn(move || {
            request(
                &endpoint,
                "prompt.wait",
                json!({"id": id, "wait_ms": 25_000}),
            )
            .unwrap()
        })
    };
    reply(endpoint, &id, json!("now")).unwrap();
    let outcome = waiting.join().unwrap();
    assert_eq!(
        outcome,
        json!({"status": "answered", "id": id, "answer": "now"})
    );
    "pending_after_wait_ms_then_prompt_wait_returns_the_answer"
}

fn case_malformed(endpoint: &Path) -> &'static str {
    let _follower = Follower::open(endpoint, &["input", "pick"]);
    for (params, field) in [
        (
            json!({"kind": "pick", "title": "Project", "items": ["api", {"id": "api"}]}),
            "'items[1].id'",
        ),
        (json!({"kind": "input"}), "'title'"),
        (
            json!({"kind": "pick", "title": "Tags", "items": ["a"], "multiple": true, "allow_other": true}),
            "'allow_other'",
        ),
    ] {
        let error = request(endpoint, "prompt.request", params).unwrap_err();
        assert_eq!(error_code(&error), ERR_INVALID_PARAMS, "{error}");
        assert!(error.contains(field), "{error} names {field}");
    }
    "a_malformed_request_is_refused_naming_the_field"
}

fn case_busy(endpoint: &Path) -> &'static str {
    let _follower = Follower::open(endpoint, &["input"]);
    let ids: Vec<Value> = (0..16)
        .map(|index| {
            let outcome = ask(endpoint, input(&format!("Prompt {index}"), 0));
            assert_eq!(outcome["status"], "pending", "{outcome}");
            outcome["id"].clone()
        })
        .collect();
    assert_eq!(
        ask(endpoint, input("One more", 0)),
        json!({"status": "busy"})
    );
    for id in &ids {
        cancel(endpoint, id);
    }
    "the_seventeenth_pending_prompt_is_busy"
}

fn case_timeout(endpoint: &Path) -> &'static str {
    let mut follower = Follower::open(endpoint, &["input"]);
    let asker = ask_later(
        endpoint,
        json!({"kind": "input", "title": "Quick", "timeout": 1, "wait_ms": 25_000}),
    );
    let shown = follower.requested();
    assert!(shown["remaining_ms"].as_u64().unwrap() <= 1_000);
    let outcome = asker.join().unwrap();
    assert_eq!(outcome["status"], "timeout", "{outcome}");
    let ended = follower.ended();
    assert_eq!(ended["reason"], "timeout");
    "an_unanswered_prompt_times_out_for_the_asker_and_the_followers"
}

fn case_shutdown(mut running: Running) -> &'static str {
    let endpoint = running.endpoint.clone();
    let mut follower = Follower::open(&endpoint, &["input"]);
    let asker = ask_later(&endpoint, input("Before the stop", 25_000));
    follower.requested();
    running.worker.take().unwrap().stop();
    let outcome = asker.join().unwrap();
    assert_eq!(outcome["status"], "failed", "{outcome}");
    assert!(
        outcome["detail"]
            .as_str()
            .unwrap()
            .contains("worker stopped"),
        "{outcome}"
    );
    "a_worker_shutdown_fails_the_waiting_asker"
}

fn case_agent_rate_limited(endpoint: &Path) -> &'static str {
    let mut follower = Follower::open(endpoint, &["input"]);
    let agent = |session: &str| json!({"kind": "input", "title": "Deploy?", "origin": {"kind": "agent", "session": session}});
    for _ in 0..3 {
        assert_eq!(ask(endpoint, agent("a1"))["status"], "pending");
        follower.requested();
    }
    assert_eq!(
        ask(endpoint, agent("a1")),
        json!({"status": "rate_limited"})
    );
    let other = ask(endpoint, agent("a2"));
    assert_eq!(other["status"], "pending");
    assert_eq!(
        follower.requested()["id"],
        other["id"],
        "the refused question was never shown"
    );
    "an_agent_past_its_burst_is_rate_limited_and_nothing_is_shown"
}

fn agent_prompt(session: &str, wait_ms: u64) -> Value {
    json!({"kind": "input", "title": "Allow rm -rf build?", "wait_ms": wait_ms, "origin": {"kind": "agent", "agent": "claude", "session": session}})
}

fn reply_with(endpoint: &Path, id: &Value, token: Option<&str>) -> Result<Value, String> {
    let mut params = json!({"id": id, "answer": "yes"});
    if let Some(token) = token {
        params["token"] = json!(token);
    }
    request(endpoint, "prompt.reply", params)
}

fn assert_not_pending(error: &str) {
    assert_eq!(error_code(error), -32060, "{error}");
    assert!(error.contains("not_pending"), "{error}");
}

fn case_reply_without_its_token(endpoint: &Path) -> &'static str {
    let mut window = Follower::open(endpoint, &["input"]);
    let pending = ask(endpoint, agent_prompt("a1", 0));
    assert_eq!(pending["status"], "pending", "{pending}");
    let shown = window.requested();
    let token = shown["token"].as_str().expect("a token").to_string();
    assert_eq!(token.len(), 32, "128 bits as hex");
    assert_not_pending(&reply_with(endpoint, &shown["id"], None).unwrap_err());
    let forged = "0".repeat(32);
    assert_not_pending(&reply_with(endpoint, &shown["id"], Some(&forged)).unwrap_err());
    let unknown = reply_with(endpoint, &json!(999), None).unwrap_err();
    assert_eq!(
        unknown.replace("999", "ID"),
        reply_with(endpoint, &shown["id"], None)
            .unwrap_err()
            .replace(&shown["id"].to_string(), "ID"),
        "the refusal reads as an unknown id"
    );
    let still = request(
        endpoint,
        "prompt.wait",
        json!({"id": shown["id"], "wait_ms": 0}),
    )
    .unwrap();
    assert_eq!(still["status"], "pending", "{still}");
    let replied = reply_with(endpoint, &shown["id"], Some(&token)).unwrap();
    assert_eq!(replied["reason"], "answered");
    "a_reply_to_an_agent_prompt_without_its_token_is_refused_and_it_stays_pending"
}

const PANE_TEST: &str = "pane_client";

#[test]
fn pane_client() {
    let Some((mode, home)) = pane::pane_mode() else {
        return;
    };
    let endpoint = paneflow_home::serve_endpoint_path(&home);
    let mut control = pane::connect(&home, "pane-client");
    match mode.split(':').collect::<Vec<_>>().as_slice() {
        ["reply", id, token] => {
            let follow = pane::worker_call(
                &mut control,
                "prompt.follow",
                json!({"kinds": ["input", "pick"]}),
            );
            pane::report("follow", &follow);
            let id: u64 = id.parse().unwrap();
            let replied = reply_with(&endpoint, &json!(id), Some(token)).map_err(Value::String);
            pane::report("reply", &replied);
        }
        _ => panic!("unknown pane client mode {mode}"),
    }
    pane::finish(&home);
}

fn case_pane_refused_on_an_agent_prompt() -> &'static str {
    let instance = pane::Instance::start(
        &json!({"shell_integration": false, "ipc": {"scripting": false, "orchestration": false}}),
        None,
    );
    let endpoint = paneflow_home::serve_endpoint_path(instance.home());
    let mut window = Follower::open(&endpoint, &["input"]);
    let asker = ask_later(&endpoint, agent_prompt("a1", 25_000));
    let shown = window.requested();
    let token = shown["token"].as_str().unwrap();
    let lines = instance.run_in_pane(PANE_TEST, &format!("reply:{}:{token}", shown["id"]));
    let follow = pane::labelled(&lines, "follow");
    assert_eq!(
        follow["ok"]["pending"], 0,
        "a pane follower is given no agent prompt and no token: {follow}"
    );
    let reply = pane::labelled(&lines, "reply");
    assert_not_pending(reply["error"].as_str().unwrap_or_default());
    let still = request(
        &endpoint,
        "prompt.wait",
        json!({"id": shown["id"], "wait_ms": 0}),
    )
    .unwrap();
    assert_eq!(still["status"], "pending", "{still}");
    let replied = reply_with(&endpoint, &shown["id"], Some(token)).unwrap();
    assert_eq!(replied["reason"], "answered");
    let outcome = asker.join().unwrap();
    assert!(
        outcome["status"] == "answered" || outcome["status"] == "pending",
        "{outcome}"
    );
    "a_pane_never_sees_nor_answers_an_agent_prompt_even_with_a_token"
}

#[test]
fn the_prompt_broker_passes_every_listed_conformance_case() {
    let mut listed = listed_cases();
    listed.sort();
    let cases: [fn(&Path) -> &'static str; 12] = [
        case_first_reply_wins,
        case_cancel,
        case_unsupported,
        case_follower_leaves,
        case_late_follower,
        case_no_client,
        case_pending_then_wait,
        case_malformed,
        case_busy,
        case_timeout,
        case_agent_rate_limited,
        case_reply_without_its_token,
    ];
    let mut ran: Vec<String> = cases
        .iter()
        .map(|case| {
            let running = start();
            case(&running.endpoint).to_string()
        })
        .collect();
    ran.push(case_shutdown(start()).to_string());
    ran.push(case_pane_refused_on_an_agent_prompt().to_string());
    ran.sort();
    assert_eq!(ran, listed, "every listed case has exactly one runner");
}
