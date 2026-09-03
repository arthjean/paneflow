use std::collections::{HashMap, HashSet};
use std::io;
use std::thread::sleep;
use std::time::{Duration, Instant};

use paneflow_ipc_client::{IpcClient, IpcTransport, StreamEvent};
use regex::Regex;
use serde_json::{Value, json};

use super::selector::{resolve_all, resolve_target};
use super::{CliError, EXIT_OK, EXIT_TIMEOUT};

const POLL_INTERVAL_MS: u64 = 500;
const DEFAULT_TIMEOUT_SECS: u64 = 300;
const DEFAULT_IDLE_FOR_MS: u64 = 1000;
const IDLE_SLICE_CAP_MS: u64 = 100;
const READ_WINDOW_LINES: u64 = 500;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MatchMode {
    Single,
    Any,
    All,
}

enum PaneState {
    Matched(Vec<String>),
    NoMatch,
    Gone,
}

#[derive(Clone, Debug)]
struct ReadSnapshot {
    text: String,
    output_generation: Option<u64>,
}

pub fn wait(
    client: &impl IpcTransport,
    target: &str,
    pattern: &str,
    timeout_secs: Option<u64>,
    mode: MatchMode,
) -> Result<i32, CliError> {
    let re = Regex::new(pattern)
        .map_err(|e| CliError::runtime(format!("invalid regex '{pattern}': {e}")))?;

    let ids: Vec<u64> = match mode {
        MatchMode::Single => vec![resolve_target(client, target)?],
        MatchMode::Any | MatchMode::All => resolve_all(client, target)?,
    };

    let timeout = Duration::from_secs(timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
    let deadline = Instant::now() + timeout;
    let baselines: HashMap<u64, Option<ReadSnapshot>> = ids
        .iter()
        .map(|&id| (id, read_snapshot(client, id).ok().flatten()))
        .collect();

    let mut all_matches: HashMap<u64, Vec<String>> = HashMap::new();

    loop {
        let mut matched_now: HashMap<u64, Vec<String>> = HashMap::new();
        let mut alive = 0usize;
        for &id in &ids {
            if mode == MatchMode::All && all_matches.contains_key(&id) {
                continue;
            }
            match read_matches_since(client, id, &re, baselines.get(&id).and_then(|b| b.as_ref()))?
            {
                PaneState::Matched(lines) => {
                    alive += 1;
                    matched_now.insert(id, lines.clone());
                    if mode == MatchMode::All {
                        all_matches.insert(id, lines);
                    }
                }
                PaneState::NoMatch => alive += 1,
                PaneState::Gone => {}
            }
        }

        let matched_count = match mode {
            MatchMode::All => all_matches.len(),
            MatchMode::Single | MatchMode::Any => matched_now.len(),
        };
        if is_done(mode, matched_count, ids.len()) {
            let matched_ids: Vec<u64> = match mode {
                MatchMode::All => ids
                    .iter()
                    .copied()
                    .filter(|id| all_matches.contains_key(id))
                    .collect(),
                MatchMode::Single | MatchMode::Any => ids
                    .iter()
                    .copied()
                    .filter(|id| matched_now.contains_key(id))
                    .collect(),
            };
            let matches_out: Vec<Value> = matched_ids
                .iter()
                .map(|id| {
                    let lines = match mode {
                        MatchMode::All => all_matches.get(id),
                        MatchMode::Single | MatchMode::Any => matched_now.get(id),
                    }
                    .cloned()
                    .unwrap_or_default();
                    json!({ "surface_id": id, "lines": lines })
                })
                .collect();
            super::print_json(
                &json!({ "matched": true, "panes": matched_ids, "matches": matches_out }),
            )?;
            return Ok(EXIT_OK);
        }

        if alive == 0 {
            return Err(CliError::runtime(
                "all target panes closed before the pattern appeared",
            ));
        }

        if Instant::now() >= deadline {
            eprintln!(
                "paneflow: timeout after {}s waiting for /{}/",
                timeout.as_secs(),
                pattern
            );
            return Ok(EXIT_TIMEOUT);
        }
        sleep(Duration::from_millis(POLL_INTERVAL_MS));
    }
}

fn is_done(mode: MatchMode, matched: usize, total: usize) -> bool {
    match mode {
        MatchMode::Single | MatchMode::Any => matched > 0,
        MatchMode::All => matched == total,
    }
}

fn read_snapshot(client: &impl IpcTransport, id: u64) -> Result<Option<ReadSnapshot>, CliError> {
    match client.call(
        "surface.read",
        json!({ "surface_id": id, "lines": READ_WINDOW_LINES, "fenced": false }),
    ) {
        Ok(result) => {
            if let Some(message) = legacy_error_message(&result) {
                if is_surface_gone_error(&message) {
                    return Ok(None);
                }
                return Err(CliError::runtime(message));
            }
            let text = result.get("text").and_then(Value::as_str).unwrap_or("");
            let output_generation = result.get("output_generation").and_then(Value::as_u64);
            Ok(Some(ReadSnapshot {
                text: text.to_string(),
                output_generation,
            }))
        }
        Err(e) if e.contains("unreachable") => Err(CliError::runtime(e)),
        Err(e) if is_surface_gone_error(&e) => Ok(None),
        Err(e) => Err(CliError::runtime(e)),
    }
}

fn legacy_error_message(value: &Value) -> Option<String> {
    let error = value.get("error")?;
    error
        .as_str()
        .map(str::to_string)
        .or_else(|| {
            error
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| Some(error.to_string()))
}

fn is_surface_gone_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("not found") || lower.contains("-32602")
}

fn read_matches_since(
    client: &impl IpcTransport,
    id: u64,
    re: &Regex,
    baseline: Option<&ReadSnapshot>,
) -> Result<PaneState, CliError> {
    let Some(current) = read_snapshot(client, id)? else {
        return Ok(PaneState::Gone);
    };
    let text = match baseline {
        Some(base)
            if matches!(
                (current.output_generation, base.output_generation),
                (Some(current), Some(previous)) if current <= previous
            ) =>
        {
            return Ok(PaneState::NoMatch);
        }
        Some(base) => new_text_since_baseline(&base.text, &current.text),
        None => current.text,
    };
    Ok(if re.is_match(&text) {
        let hits = text
            .lines()
            .filter(|l| re.is_match(l))
            .map(str::to_string)
            .collect();
        PaneState::Matched(hits)
    } else {
        PaneState::NoMatch
    })
}

fn new_text_since_baseline(baseline: &str, current: &str) -> String {
    if current == baseline {
        return String::new();
    }
    if let Some(rest) = current.strip_prefix(baseline) {
        return rest.to_string();
    }
    let old_lines: HashSet<&str> = baseline.lines().collect();
    current
        .lines()
        .filter(|line| !old_lines.contains(line))
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum IdleSignal {
    Activity,
    Quiet,
    Tick,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum IdleOutcome {
    Continue,
    Idle,
    Dead,
    TimedOut,
}

fn idle_decision(
    sig: IdleSignal,
    since_change: Duration,
    for_window: Duration,
    past_deadline: bool,
) -> IdleOutcome {
    match sig {
        IdleSignal::Closed => IdleOutcome::Dead,
        IdleSignal::Tick => {
            if since_change >= for_window {
                IdleOutcome::Idle
            } else if past_deadline {
                IdleOutcome::TimedOut
            } else {
                IdleOutcome::Continue
            }
        }
        IdleSignal::Activity | IdleSignal::Quiet => {
            if past_deadline {
                IdleOutcome::TimedOut
            } else {
                IdleOutcome::Continue
            }
        }
    }
}

fn classify_event_line(line: &str) -> IdleSignal {
    let kind = serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|v| v.get("type").and_then(Value::as_str).map(str::to_owned));
    match kind.as_deref() {
        Some("surface_changed") | Some("dropped") => IdleSignal::Activity,
        _ => IdleSignal::Quiet,
    }
}

fn pane_matches_since(
    client: &impl IpcTransport,
    id: u64,
    re: &Regex,
    baseline: Option<&ReadSnapshot>,
) -> bool {
    matches!(
        read_matches_since(client, id, re, baseline),
        Ok(PaneState::Matched(_))
    )
}

pub fn wait_idle(
    client: &IpcClient,
    target: &str,
    for_ms: Option<u64>,
    timeout_secs: Option<u64>,
    pattern: Option<&str>,
) -> Result<i32, CliError> {
    let id = resolve_target(client, target)?;
    let re: Option<Regex> = match pattern {
        Some(p) => Some(
            Regex::new(p).map_err(|e| CliError::runtime(format!("invalid regex '{p}': {e}")))?,
        ),
        None => None,
    };
    let window_ms = for_ms.unwrap_or(DEFAULT_IDLE_FOR_MS);
    let for_window = Duration::from_millis(window_ms);
    let slice = Duration::from_millis(window_ms.clamp(1, IDLE_SLICE_CAP_MS));
    let timeout = Duration::from_secs(timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
    let deadline = Instant::now() + timeout;

    let socket = paneflow_ipc_client::resolve_socket_path().ok_or_else(|| {
        CliError::target(
            "cannot locate the IPC socket; is Paneflow running? \
             (set PANEFLOW_SOCKET_PATH if you launched the CLI outside a Paneflow pane)",
        )
    })?;

    let baseline = read_snapshot(client, id).ok().flatten();

    let _ = ctrlc::set_handler(|| std::process::exit(130));

    let params = json!({ "surfaces": [id], "types": ["surface_changed"] });
    let mut since_change = Instant::now();
    let mut outcome = IdleOutcome::Dead;
    let mut matched = false;

    let stream_result = paneflow_ipc_client::subscribe_stream_timed(&socket, params, slice, |ev| {
        let past_deadline = Instant::now() >= deadline;
        let sig = match ev {
            StreamEvent::Line(l) => classify_event_line(l),
            StreamEvent::Tick => IdleSignal::Tick,
            StreamEvent::Closed => IdleSignal::Closed,
        };
        if sig == IdleSignal::Activity {
            if let Some(re) = &re
                && pane_matches_since(client, id, re, baseline.as_ref())
            {
                matched = true;
                outcome = IdleOutcome::Idle;
                return false;
            }
            since_change = Instant::now();
        }
        match idle_decision(sig, since_change.elapsed(), for_window, past_deadline) {
            IdleOutcome::Continue => true,
            other => {
                outcome = other;
                false
            }
        }
    });

    match stream_result {
        Ok(()) => match outcome {
            IdleOutcome::Idle => {
                super::print_json(
                    &json!({ "surface_id": id, "idle": !matched, "matched": matched }),
                )?;
                Ok(EXIT_OK)
            }
            IdleOutcome::TimedOut => {
                eprintln!(
                    "paneflow: timeout after {}s waiting for surface {id} to go idle",
                    timeout.as_secs()
                );
                Ok(EXIT_TIMEOUT)
            }
            IdleOutcome::Dead => Err(CliError::runtime(
                "the Paneflow event stream closed before the pane went idle (did Paneflow exit?)",
            )),
            IdleOutcome::Continue => Err(CliError::runtime(
                "idle wait ended without a verdict (internal)",
            )),
        },
        Err(e) if e.kind() == io::ErrorKind::Unsupported => wait_idle_poll(
            client,
            id,
            for_window,
            timeout,
            re.as_ref(),
            baseline.as_ref(),
        ),
        Err(e) => Err(CliError::target(format!("wait --idle failed: {e}"))),
    }
}

fn wait_idle_poll(
    client: &impl IpcTransport,
    id: u64,
    for_window: Duration,
    timeout: Duration,
    re: Option<&Regex>,
    baseline: Option<&ReadSnapshot>,
) -> Result<i32, CliError> {
    let deadline = Instant::now() + timeout;
    let mut last_snapshot = match read_snapshot(client, id)? {
        Some(s) => s,
        None => {
            return Err(CliError::runtime(
                "target pane closed before idle wait started",
            ));
        }
    };
    let mut since_change = Instant::now();
    loop {
        sleep(Duration::from_millis(IDLE_SLICE_CAP_MS));
        let past_deadline = Instant::now() >= deadline;
        let Some(current) = read_snapshot(client, id)? else {
            return Err(CliError::runtime("target pane closed before it went idle"));
        };
        let changed = match (current.output_generation, last_snapshot.output_generation) {
            (Some(current), Some(previous)) => current > previous,
            _ => current.text != last_snapshot.text,
        };
        if changed {
            last_snapshot = current;
            since_change = Instant::now();
            if let Some(re) = re
                && pane_matches_since(client, id, re, baseline)
            {
                super::print_json(&json!({ "surface_id": id, "idle": false, "matched": true }))?;
                return Ok(EXIT_OK);
            }
        }
        if since_change.elapsed() >= for_window {
            super::print_json(&json!({ "surface_id": id, "idle": true, "matched": false }))?;
            return Ok(EXIT_OK);
        }
        if past_deadline {
            eprintln!(
                "paneflow: timeout after {}s waiting for surface {id} to go idle",
                timeout.as_secs()
            );
            return Ok(EXIT_TIMEOUT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_done_single_and_any_need_one_match() {
        assert!(!is_done(MatchMode::Single, 0, 1));
        assert!(is_done(MatchMode::Single, 1, 1));
        assert!(!is_done(MatchMode::Any, 0, 3));
        assert!(is_done(MatchMode::Any, 1, 3));
    }

    #[test]
    fn is_done_all_needs_every_pane() {
        assert!(!is_done(MatchMode::All, 2, 3));
        assert!(is_done(MatchMode::All, 3, 3));
    }

    struct NeverCalled;
    impl IpcTransport for NeverCalled {
        fn call(&self, _: &str, _: Value) -> Result<Value, String> {
            Err("transport should not be called".to_string())
        }
    }

    #[test]
    fn invalid_regex_fails_before_any_ipc_call() {
        let err = wait(&NeverCalled, "x", "(unclosed", None, MatchMode::Single).unwrap_err();
        assert!(
            err.message.contains("invalid regex"),
            "got: {}",
            err.message
        );
    }

    struct FakeWait {
        reads: std::cell::RefCell<Vec<Option<&'static str>>>,
        read_calls: std::cell::Cell<u64>,
    }
    impl FakeWait {
        fn new(reads: Vec<Option<&'static str>>) -> Self {
            Self {
                reads: std::cell::RefCell::new(reads),
                read_calls: std::cell::Cell::new(0),
            }
        }
    }
    impl IpcTransport for FakeWait {
        fn call(&self, method: &str, _params: Value) -> Result<Value, String> {
            match method {
                "surface.list" => Ok(json!({
                    "surfaces": [{ "surface_id": 1u64, "name": "agent", "cmd": "claude", "cwd": "/tmp" }]
                })),
                "surface.read" => {
                    let call = self.read_calls.get() + 1;
                    self.read_calls.set(call);
                    let mut reads = self.reads.borrow_mut();
                    let next = if reads.len() > 1 {
                        reads.remove(0)
                    } else {
                        reads.first().copied().flatten()
                    };
                    match next {
                        Some(t) => Ok(json!({ "text": t, "output_generation": call })),
                        None => Err("paneflow error -32602: surface_id 1 not found".to_string()),
                    }
                }
                other => Err(format!("unexpected method {other}")),
            }
        }
    }

    #[test]
    fn wait_succeeds_and_surfaces_matched_line() {
        let fake = FakeWait::new(vec![
            Some("compiling...\n"),
            Some("compiling...\nBuild DONE in 3s\n"),
        ]);
        let code = wait(&fake, "1", "DONE", Some(5), MatchMode::Single).expect("ok");
        assert_eq!(code, EXIT_OK);
    }

    #[test]
    fn wait_times_out_with_dedicated_code() {
        let fake = FakeWait::new(vec![Some("still working\n")]);
        let code = wait(&fake, "1", "DONE", Some(0), MatchMode::Single).expect("ok");
        assert_eq!(code, EXIT_TIMEOUT);
    }

    #[test]
    fn wait_fails_fast_when_target_pane_gone() {
        let fake = FakeWait::new(vec![None]);
        let err = wait(&fake, "1", "DONE", Some(30), MatchMode::Single).unwrap_err();
        assert!(err.message.contains("closed"), "got: {}", err.message);
    }

    struct MultiWait {
        reads: std::cell::RefCell<HashMap<u64, Vec<&'static str>>>,
        generations: std::cell::RefCell<HashMap<u64, u64>>,
    }
    impl MultiWait {
        fn new() -> Self {
            Self {
                reads: std::cell::RefCell::new(HashMap::from([
                    (1, vec!["", "DONE one"]),
                    (2, vec!["", "", "DONE two"]),
                ])),
                generations: std::cell::RefCell::new(HashMap::new()),
            }
        }
    }
    impl IpcTransport for MultiWait {
        fn call(&self, method: &str, params: Value) -> Result<Value, String> {
            match method {
                "surface.list" => Ok(json!({
                    "surfaces": [
                        { "surface_id": 1u64, "name": "agent-a", "cmd": "agent", "cwd": "/tmp/a" },
                        { "surface_id": 2u64, "name": "agent-b", "cmd": "agent", "cwd": "/tmp/b" }
                    ]
                })),
                "surface.read" => {
                    let sid = params["surface_id"].as_u64().unwrap_or(0);
                    let mut generations = self.generations.borrow_mut();
                    let generation = generations.entry(sid).or_insert(0);
                    *generation += 1;
                    let mut reads = self.reads.borrow_mut();
                    let script = reads.entry(sid).or_default();
                    let text = if script.len() > 1 {
                        script.remove(0)
                    } else {
                        script.first().copied().unwrap_or_default()
                    };
                    Ok(json!({ "text": text, "output_generation": *generation }))
                }
                other => Err(format!("unexpected method {other}")),
            }
        }
    }

    #[test]
    fn wait_all_persists_matches_across_polls() {
        let fake = MultiWait::new();
        let code = wait(&fake, "cmdline:agent", "DONE", Some(2), MatchMode::All).expect("ok");
        assert_eq!(code, EXIT_OK);
    }

    struct ReadError(&'static str);
    impl IpcTransport for ReadError {
        fn call(&self, method: &str, _params: Value) -> Result<Value, String> {
            match method {
                "surface.read" => Err(self.0.to_string()),
                other => Err(format!("unexpected method {other}")),
            }
        }
    }

    #[test]
    fn read_snapshot_only_treats_not_found_as_gone() {
        assert!(
            read_snapshot(&ReadError("server error -32602: surface not found"), 1)
                .expect("ok")
                .is_none()
        );
        let err = read_snapshot(&ReadError("server error -32000: overloaded"), 1).unwrap_err();
        assert!(err.message.contains("overloaded"), "got: {}", err.message);
    }

    #[test]
    fn baseline_diff_ignores_prompt_echo_sentinel() {
        let base = "please print RENDER_AUDIT_DONE when complete\n";
        let current = "please print RENDER_AUDIT_DONE when complete\nactual work\n";
        assert_eq!(new_text_since_baseline(base, current), "actual work\n");

        let shifted = "actual work\nplease print RENDER_AUDIT_DONE when complete\nnew DONE\n";
        assert_eq!(
            new_text_since_baseline(base, shifted),
            "actual work\nnew DONE"
        );
    }

    const FW: Duration = Duration::from_millis(1000);

    #[test]
    fn idle_decision_tick_idles_only_after_window() {
        assert_eq!(
            idle_decision(IdleSignal::Tick, Duration::from_millis(1000), FW, false),
            IdleOutcome::Idle
        );
        assert_eq!(
            idle_decision(IdleSignal::Tick, Duration::from_millis(1500), FW, false),
            IdleOutcome::Idle
        );
        assert_eq!(
            idle_decision(IdleSignal::Tick, Duration::from_millis(300), FW, false),
            IdleOutcome::Continue
        );
    }

    #[test]
    fn idle_decision_exit_code_matrix() {
        assert_eq!(
            idle_decision(IdleSignal::Activity, Duration::from_millis(10), FW, true),
            IdleOutcome::TimedOut
        );
        assert_eq!(
            idle_decision(IdleSignal::Tick, Duration::from_millis(10), FW, true),
            IdleOutcome::TimedOut
        );
        assert_eq!(
            idle_decision(IdleSignal::Tick, Duration::from_millis(1000), FW, true),
            IdleOutcome::Idle
        );
        assert_eq!(
            idle_decision(IdleSignal::Closed, Duration::from_millis(10), FW, false),
            IdleOutcome::Dead
        );
        assert_eq!(
            idle_decision(IdleSignal::Closed, Duration::from_millis(9999), FW, true),
            IdleOutcome::Dead
        );
    }

    #[test]
    fn idle_decision_activity_and_heartbeat_keep_waiting() {
        assert_eq!(
            idle_decision(IdleSignal::Activity, Duration::from_millis(9999), FW, false),
            IdleOutcome::Continue
        );
        assert_eq!(
            idle_decision(IdleSignal::Quiet, Duration::from_millis(9999), FW, false),
            IdleOutcome::Continue
        );
    }

    #[test]
    fn classify_event_line_only_surface_changed_is_activity() {
        assert_eq!(
            classify_event_line(
                r#"{"type":"surface_changed","surface_id":1,"output_generation":5}"#
            ),
            IdleSignal::Activity
        );
        assert_eq!(
            classify_event_line(r#"{"type":"dropped","count":2}"#),
            IdleSignal::Activity
        );
        assert_eq!(
            classify_event_line(r#"{"type":"heartbeat"}"#),
            IdleSignal::Quiet
        );
        assert_eq!(
            classify_event_line(r#"{"type":"subscribed","id":1}"#),
            IdleSignal::Quiet
        );
        assert_eq!(classify_event_line("not json at all"), IdleSignal::Quiet);
        assert_eq!(classify_event_line(r#"{"no":"type"}"#), IdleSignal::Quiet);
    }
}
