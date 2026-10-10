use std::collections::HashMap;
use std::io;
use std::thread::sleep;
use std::time::{Duration, Instant};

use paneflow_ipc_client::{IpcTransport, StreamEvent};
use regex::Regex;
use serde_json::{Value, json};

use super::selector::{resolve_all, resolve_target};
use super::send_cmd::status_of;
use super::surface_read::{
    READ_WINDOW_LINES, ReadSnapshot, SurfaceRead, read_baseline, read_surface, text_after_baseline,
};
use super::worker_state::{ATTENTION, BLOCKED, IDLE, reduced_state, reports_turns};
use super::{CliError, EXIT_OK, EXIT_TIMEOUT};

const POLL_INTERVAL_MS: u64 = 500;
const TURN_POLL: Duration = Duration::from_millis(250);
const DEFAULT_TIMEOUT_SECS: u64 = 300;
const DEFAULT_IDLE_FOR_MS: u64 = 1000;
const IDLE_SLICE_CAP_MS: u64 = 100;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MatchMode {
    Single,
    Any,
    All,
}

enum PaneState {
    Matched(Vec<String>),
    NoMatch,
    Skipped,
    Gone,
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
    let mut baselines: HashMap<u64, Option<ReadSnapshot>> = HashMap::with_capacity(ids.len());
    for &id in &ids {
        let baseline = read_baseline(client, id, READ_WINDOW_LINES).map_err(CliError::runtime)?;
        baselines.insert(id, baseline);
    }

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
                PaneState::NoMatch | PaneState::Skipped => alive += 1,
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

fn read_snapshot(client: &impl IpcTransport, id: u64) -> Result<SurfaceRead, CliError> {
    read_surface(client, id, READ_WINDOW_LINES)
        .map_err(|error| CliError::runtime(error.to_string()))
}

fn read_matches_since(
    client: &impl IpcTransport,
    id: u64,
    re: &Regex,
    baseline: Option<&ReadSnapshot>,
) -> Result<PaneState, CliError> {
    let current = match read_snapshot(client, id)? {
        SurfaceRead::Snapshot(current) => current,
        SurfaceRead::Gone => return Ok(PaneState::Gone),
        SurfaceRead::Skipped => return Ok(PaneState::Skipped),
    };
    let text = match baseline {
        Some(base) => match text_after_baseline(base, &current) {
            Some(text) => text,
            None => return Ok(PaneState::NoMatch),
        },
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

fn turn_settled(state: Option<&str>) -> bool {
    matches!(state, Some(IDLE | ATTENTION | BLOCKED))
}

fn follows_turns(status: &Value) -> bool {
    reports_turns(status) && status.get("state_seq").is_some()
}

fn wait_turn_end(
    client: &impl IpcTransport,
    id: u64,
    status: Value,
    deadline: Instant,
    re: Option<&Regex>,
) -> Result<i32, CliError> {
    let baseline = match re {
        Some(_) => read_baseline(client, id, READ_WINDOW_LINES).map_err(CliError::runtime)?,
        None => None,
    };
    let mut current = status;
    loop {
        let state = reduced_state(&current);
        if turn_settled(state) {
            super::print_json(&json!({
                "surface_id": id,
                "idle": true,
                "matched": false,
                "state": state,
                "signal": "agent_state",
            }))?;
            return Ok(EXIT_OK);
        }
        if let Some(re) = re
            && pane_matches_since(client, id, re, baseline.as_ref())
        {
            super::print_json(
                &json!({ "surface_id": id, "idle": false, "matched": true, "state": state }),
            )?;
            return Ok(EXIT_OK);
        }
        if Instant::now() >= deadline {
            eprintln!("paneflow: timeout waiting for the agent in surface {id} to end its turn");
            return Ok(EXIT_TIMEOUT);
        }
        sleep(TURN_POLL);
        match status_of(client, id) {
            Some(status) => current = status,
            None if matches!(read_snapshot(client, id)?, SurfaceRead::Gone) => {
                return Err(CliError::runtime("target pane closed before it went idle"));
            }
            None => {}
        }
    }
}

pub fn wait_idle(
    client: &impl IpcTransport,
    socket: Option<&std::path::Path>,
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

    if let Some(status) = status_of(client, id).filter(follows_turns) {
        return wait_turn_end(client, id, status, deadline, re.as_ref());
    }
    let Some(socket) = socket else {
        return Err(super::CliTransport::no_controller("paneflow wait --idle"));
    };

    let Some(baseline) = read_baseline(client, id, READ_WINDOW_LINES).map_err(CliError::runtime)?
    else {
        return Err(CliError::runtime(
            "target pane closed before idle wait started",
        ));
    };
    let baseline = Some(baseline);

    let _ = ctrlc::set_handler(|| std::process::exit(130));

    let params = json!({ "surfaces": [id], "types": ["surface_changed"] });
    let mut since_change = Instant::now();
    let mut outcome = IdleOutcome::Dead;
    let mut matched = false;

    let stream_result = paneflow_ipc_client::subscribe_stream_timed(socket, params, slice, |ev| {
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
            IdleOutcome::Idle => report_idle(client, id, matched),
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

fn report_idle(client: &impl IpcTransport, id: u64, matched: bool) -> Result<i32, CliError> {
    if !matched && matches!(read_snapshot(client, id)?, SurfaceRead::Gone) {
        return Err(CliError::runtime("target pane closed before it went idle"));
    }
    super::print_json(&json!({ "surface_id": id, "idle": !matched, "matched": matched }))?;
    Ok(EXIT_OK)
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
    let mut last_snapshot =
        match read_baseline(client, id, READ_WINDOW_LINES).map_err(CliError::runtime)? {
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
        let current = match read_snapshot(client, id)? {
            SurfaceRead::Snapshot(current) => current,
            SurfaceRead::Gone => {
                return Err(CliError::runtime("target pane closed before it went idle"));
            }
            SurfaceRead::Skipped => continue,
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
    use paneflow_ipc_client::IpcCallError;

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

    struct HookedAgent {
        statuses: std::cell::RefCell<Vec<Value>>,
        status_calls: std::cell::Cell<usize>,
    }
    impl IpcTransport for HookedAgent {
        fn call(&self, method: &str, _params: Value) -> Result<Value, String> {
            match method {
                "surface.list" => Ok(json!({
                    "surfaces": [{ "surface_id": 1u64, "name": "agent", "cmd": "claude", "cwd": "/tmp" }]
                })),
                "surface.read" => {
                    Ok(json!({ "text": "running a silent tool\n", "output_generation": 7 }))
                }
                "surface.status" => {
                    self.status_calls.set(self.status_calls.get() + 1);
                    let mut statuses = self.statuses.borrow_mut();
                    Ok(if statuses.len() > 1 {
                        statuses.remove(0)
                    } else {
                        statuses[0].clone()
                    })
                }
                other => Err(format!("unexpected method {other}")),
            }
        }
    }

    fn hooked(state: &str, state_seq: u64) -> Value {
        json!({
            "state": state,
            "state_seq": state_seq,
            "activity_source": "declared",
            "agent_runtime": "com.anthropic.claude-code",
            "output_generation": 7,
        })
    }

    #[test]
    fn a_declaring_agent_in_a_silent_tool_is_not_idle_before_it_declares_the_turn_over() {
        let mut statuses = vec![hooked("thinking", 1); 5];
        statuses.push(hooked("finished", 2));
        let agent = HookedAgent {
            statuses: std::cell::RefCell::new(statuses),
            status_calls: std::cell::Cell::new(0),
        };
        let never_listened = std::path::Path::new("paneflow-no-such-event-stream");
        let code = wait_idle(&agent, Some(never_listened), "1", Some(5), Some(30), None)
            .expect("the turn end ends the wait");
        assert_eq!(code, EXIT_OK);
        assert_eq!(
            agent.status_calls.get(),
            6,
            "the wait outlived a 5 ms quiet window and returned on the declared state"
        );
    }

    #[test]
    fn an_agent_already_waiting_returns_at_once() {
        let agent = HookedAgent {
            statuses: std::cell::RefCell::new(vec![hooked("waiting_for_input", 4)]),
            status_calls: std::cell::Cell::new(0),
        };
        assert_eq!(
            wait_idle(&agent, None, "1", None, Some(5), None).expect("settled"),
            EXIT_OK
        );
        assert_eq!(agent.status_calls.get(), 1);
    }

    #[test]
    fn a_pane_without_turn_signals_keeps_the_quiescence_wait() {
        let shell = HookedAgent {
            statuses: std::cell::RefCell::new(vec![json!({"state": "idle", "state_seq": 0})]),
            status_calls: std::cell::Cell::new(0),
        };
        let err = wait_idle(&shell, None, "1", None, Some(5), None)
            .expect_err("a shell needs the output stream");
        assert!(err.message.contains("wait --idle"), "{}", err.message);
        let quiet_runtime = HookedAgent {
            statuses: std::cell::RefCell::new(vec![json!({
                "state": "thinking",
                "state_seq": 3,
                "agent_runtime": "com.sourcegraph.amp",
            })]),
            status_calls: std::cell::Cell::new(0),
        };
        assert!(wait_idle(&quiet_runtime, None, "1", None, Some(5), None).is_err());
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
        assert!(matches!(
            read_snapshot(&ReadError("server error -32602: surface not found"), 1).expect("ok"),
            SurfaceRead::Gone
        ));
        let err =
            read_snapshot(&ReadError("server error -32003: runtime unavailable"), 1).unwrap_err();
        assert!(
            err.message.contains("runtime unavailable"),
            "got: {}",
            err.message
        );
    }

    struct Scripted {
        replies: std::cell::RefCell<Vec<Result<&'static str, IpcCallError>>>,
        reads: std::cell::Cell<u32>,
    }
    impl Scripted {
        fn new(replies: Vec<Result<&'static str, IpcCallError>>) -> Self {
            Self {
                replies: std::cell::RefCell::new(replies),
                reads: std::cell::Cell::new(0),
            }
        }
    }
    impl IpcTransport for Scripted {
        fn call(&self, method: &str, params: Value) -> Result<Value, String> {
            self.try_call(method, params).map_err(|e| e.to_string())
        }
        fn try_call(&self, method: &str, _params: Value) -> Result<Value, IpcCallError> {
            match method {
                "surface.list" => Ok(json!({
                    "surfaces": [{ "surface_id": 1u64, "name": "agent", "cmd": "claude", "cwd": "/tmp" }]
                })),
                "surface.read" => {
                    let call = self.reads.get() + 1;
                    self.reads.set(call);
                    let mut replies = self.replies.borrow_mut();
                    let next = if replies.len() > 1 {
                        replies.remove(0)
                    } else {
                        replies[0].clone()
                    };
                    next.map(|text| json!({ "text": text, "output_generation": call }))
                }
                other => Err(IpcCallError::Failed(format!("unexpected method {other}"))),
            }
        }
    }

    fn busy() -> Result<&'static str, IpcCallError> {
        Err(IpcCallError::Busy(
            "paneflow error -32000: busy".to_string(),
        ))
    }

    #[test]
    fn busy_replies_are_retried_then_count_as_a_skipped_poll() {
        let fake = Scripted::new(vec![
            Ok("working\n"),
            busy(),
            busy(),
            busy(),
            busy(),
            Ok("working\nDONE\n"),
        ]);
        let code = wait(&fake, "1", "DONE", Some(30), MatchMode::Single).expect("ok");
        assert_eq!(code, EXIT_OK, "a busy stretch does not end the wait");
        assert_eq!(
            fake.reads.get(),
            6,
            "four busy replies: one call and three retries"
        );
    }

    #[test]
    fn a_failing_baseline_is_retried_three_times_then_fails_the_command() {
        let fake = Scripted::new(vec![Err(IpcCallError::Failed(
            "paneflow error -32003: the terminal runtime is unavailable".to_string(),
        ))]);
        let err = wait(&fake, "1", "DONE", Some(30), MatchMode::Single).unwrap_err();
        assert!(err.message.contains("baseline"), "got: {}", err.message);
        assert_eq!(fake.reads.get(), 4);
    }

    #[test]
    fn a_stopped_paneflow_fails_the_wait_at_once() {
        let dir = tempfile::tempdir().expect("tmp");
        let client = paneflow_ipc_client::IpcClient::new(dir.path().join("absent.sock"));
        let started = Instant::now();
        let err = wait(&client, "1", "DONE", Some(30), MatchMode::Single).unwrap_err();
        assert!(
            err.message.contains("IPC unreachable"),
            "got: {}",
            err.message
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "no retry on a dead socket"
        );
    }

    #[test]
    fn an_unreachable_baseline_is_not_retried() {
        let fake = Scripted::new(vec![Err(IpcCallError::Unreachable(
            "paneflow IPC unreachable at x".to_string(),
        ))]);
        let err = wait(&fake, "1", "DONE", Some(30), MatchMode::Single).unwrap_err();
        assert!(
            err.message.contains("IPC unreachable"),
            "got: {}",
            err.message
        );
        assert_eq!(fake.reads.get(), 1);
    }

    #[test]
    fn an_idle_verdict_on_a_gone_pane_is_an_error() {
        let gone = FakeWait::new(vec![None]);
        let err = report_idle(&gone, 1, false).unwrap_err();
        assert!(err.message.contains("closed"), "got: {}", err.message);
        let live = FakeWait::new(vec![Some("quiet\n")]);
        assert_eq!(report_idle(&live, 1, false).expect("idle"), EXIT_OK);
    }

    #[test]
    fn wait_idle_polling_a_gone_pane_exits_nonzero() {
        let fake = FakeWait::new(vec![Some("prompt\n"), None]);
        let err = wait_idle_poll(
            &fake,
            1,
            Duration::from_secs(5),
            Duration::from_secs(5),
            None,
            None,
        )
        .unwrap_err();
        assert!(err.message.contains("closed"), "got: {}", err.message);
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
