#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_in_result
)]

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use paneflow_host::protocol::{ClientHello, METHOD_AGENT_EVENT};
use paneflow_host::{HostClient, SessionSummary};
use paneflow_ipc_client::host_control::HostControl;
use paneflow_serve::protocol::{ERR_EVICTED, METHOD_EVENTS_FOLLOW, WORKER_PROTOCOL_VERSION};
use serde_json::{Value, json};

const CONFORMANCE: &str = include_str!("../../../protocol/events-conformance-v1.json");

const FRAME_WAIT: Duration = Duration::from_secs(15);

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

struct Follower {
    control: HostControl,
    header: Value,
    seen: Vec<Value>,
}

fn open_follow(endpoint: &Path, params: Value) -> Result<Follower, Value> {
    let mut control = HostControl::connect(endpoint, "events-conformance").expect("connects");
    let id = control
        .write_request(METHOD_EVENTS_FOLLOW, params)
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
        if envelope["error"].is_object() {
            return Err(envelope["error"].clone());
        }
        return Ok(Follower {
            control,
            header: envelope["result"].clone(),
            seen: Vec::new(),
        });
    }
}

fn follow(endpoint: &Path, params: Value) -> Follower {
    open_follow(endpoint, params).unwrap_or_else(|error| panic!("events.follow refused: {error}"))
}

impl Follower {
    fn frame(&mut self) -> Option<Value> {
        let line = self.control.read_stream_line(FRAME_WAIT).ok()??;
        Some(serde_json::from_str(&line).expect("a JSON frame"))
    }

    fn next_event(&mut self) -> Value {
        loop {
            let frame = self
                .frame()
                .expect("the stream stays open while an event is expected");
            if frame.get("seq").is_some() {
                self.seen.push(frame.clone());
                return frame;
            }
            assert_eq!(frame["type"], "keepalive", "a non-event frame: {frame}");
        }
    }

    fn until(&mut self, mut wanted: impl FnMut(&Value) -> bool) -> Value {
        let deadline = Instant::now() + FRAME_WAIT;
        while Instant::now() < deadline {
            let event = self.next_event();
            if wanted(&event) {
                return event;
            }
        }
        panic!("the wanted event did not arrive within {FRAME_WAIT:?}");
    }

    fn until_seq(&mut self, seq: u64) {
        self.until(|event| event["seq"].as_u64() >= Some(seq));
    }

    fn seqs(&self) -> Vec<u64> {
        self.seen
            .iter()
            .map(|event| event["seq"].as_u64().unwrap())
            .collect()
    }
}

type Log = std::sync::Arc<paneflow_serve::server::Worker>;

fn publish(log: &Log, events: Value) -> Result<Vec<u64>, String> {
    let events: Vec<paneflow_api::methods::events::PublishedEvent> =
        serde_json::from_value(events).map_err(|error| error.to_string())?;
    Ok(log.append_layout_events(events))
}

fn create_workspace(control: &mut HostControl, title: &str) -> Value {
    let lease = control
        .request("layout.lease_ids", json!({}))
        .expect("a lease is granted");
    let start = lease["start"].as_u64().unwrap();
    let surface = paneflow_layout::NewSurface {
        id: paneflow_layout::SurfaceId(start + 3),
        definition: paneflow_config::schema::SurfaceDefinition {
            surface_type: Some("terminal".to_string()),
            ..Default::default()
        },
    };
    let op = paneflow_layout::Op::CreateWorkspace {
        workspace: paneflow_layout::NewWorkspace {
            id: paneflow_layout::WorkspaceId(start),
            title: title.to_string(),
            cwd: "/work".into(),
            space: None,
            durable_id: None,
            custom_buttons: Vec::new(),
            managed_worktrees: Vec::new(),
            tabs: vec![paneflow_layout::NewTab {
                id: paneflow_layout::TabId(start + 1),
                title: String::new(),
                title_source: paneflow_config::schema::TabTitleSource::Preset,
                worktree: None,
                root: Some(paneflow_layout::NewNode::Pane(paneflow_layout::NewPane {
                    id: paneflow_layout::PaneId(start + 2),
                    surfaces: vec![surface],
                    active: 0,
                })),
            }],
            active_tab: 0,
        },
        select: true,
    };
    control
        .request(
            "layout.apply",
            json!({"op": serde_json::to_value(&op).unwrap(), "ids_from": start + 4}),
        )
        .expect("the worker applies the op")
}

fn shell_params() -> Value {
    #[cfg(windows)]
    let (shell, args) = ("cmd.exe", vec!["/Q", "/D"]);
    #[cfg(unix)]
    let (shell, args) = ("/bin/sh", vec!["-s"]);
    json!({
        "shell": shell,
        "args": args,
        "cwd": std::env::temp_dir().display().to_string(),
        "cols": 80,
        "rows": 24,
    })
}

fn osc_command(osc: &str) -> String {
    #[cfg(windows)]
    return format!(
        "powershell -NoProfile -Command \"[Console]::Write([char]27 + ']{osc}' + [char]7)\"\r\n"
    );
    #[cfg(unix)]
    return format!("printf '\\033]{osc}\\007'\n");
}

fn exit_command(code: i32) -> String {
    #[cfg(windows)]
    return format!("exit {code}\r\n");
    #[cfg(unix)]
    return format!("exit {code}\n");
}

fn start_shell(owner: &mut HostClient) -> SessionSummary {
    serde_json::from_value(
        owner
            .call("session.create", shell_params())
            .expect("a session starts"),
    )
    .unwrap()
}

#[test]
fn the_worker_event_log_passes_every_listed_conformance_case() {
    let mut listed = listed_cases();
    listed.sort();
    let mut ran: Vec<String> = Vec::new();

    let home = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var(paneflow_home::HOME_ENV, home.path()) };
    let core_endpoint = paneflow_home::host_endpoint_path(home.path());
    let core = paneflow_host::SessionHost::open(home.path(), &core_endpoint).expect("a core opens");
    let core_server = paneflow_host::ServerHandle::spawn(Arc::clone(&core), core_endpoint.clone())
        .expect("the core serves its endpoint");
    let mut owner = HostClient::connect(&core_endpoint, &ClientHello::local("events-conformance"))
        .expect("the core accepts a client");

    let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");
    let endpoint = paneflow_home::serve_endpoint_path(home.path());

    let mut first = follow(&endpoint, json!({}));
    let mut second = follow(&endpoint, json!({}));
    assert_eq!(first.header["type"], "following");
    let start = first.header["seq"]
        .as_u64()
        .expect("the header carries seq");
    let epoch = first.header["epoch"]
        .as_str()
        .expect("the header carries the epoch")
        .to_owned();
    assert!(!epoch.is_empty());
    assert_eq!(second.header["epoch"], epoch.as_str());

    ran.push(case_layout_events_appended_by_the_worker(
        &endpoint, &mut first,
    ));
    ran.push(case_terminal_events_from_the_host_stream(
        &mut owner, &mut first,
    ));
    ran.push(case_agent_state_changed_from_the_reducer(
        &mut owner, &mut first,
    ));
    ran.push(case_header_then_consecutive_seq_to_two_followers(
        start,
        &first,
        &mut second,
    ));
    let log = std::sync::Arc::clone(worker.worker());
    ran.push(case_since_replays_exactly_the_missed_events(
        &log, &endpoint, &epoch, &first,
    ));
    ran.push(case_filters_keep_the_log_seq(&log, &endpoint));
    ran.push(case_keepalive_when_quiet(&endpoint));
    ran.push(case_evicted_when_since_left_the_ring_or_the_epoch_differs(
        &log, &endpoint, &epoch,
    ));
    ran.push(case_a_follower_over_eight_mebibytes_behind_is_closed_alone(
        &log, &endpoint,
    ));
    drop(log);

    worker.stop();
    let _ = core_server.stop();
    drop(core);

    ran.sort();
    assert_eq!(
        ran, listed,
        "every case listed in protocol/events-conformance-v1.json has a runner"
    );
}

fn case_layout_events_appended_by_the_worker(endpoint: &Path, first: &mut Follower) -> String {
    let mut control = HostControl::connect(endpoint, "events-conformance").expect("connects");
    let applied = create_workspace(&mut control, "conformance");
    let kinds: Vec<String> = applied["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["type"].as_str().unwrap().to_string())
        .collect();
    assert!(
        kinds.contains(&"workspace.created".to_string()),
        "{kinds:?}"
    );
    assert!(kinds.contains(&"pane.created".to_string()), "{kinds:?}");
    let mut seqs = Vec::new();
    for kind in &kinds {
        let event = first.next_event();
        assert_eq!(&event["type"], kind.as_str());
        assert!(event["ts"].as_u64().is_some_and(|ts| ts > 0));
        assert!(event["data"].is_object());
        seqs.push(event["seq"].as_u64().unwrap());
    }
    assert!(
        seqs.windows(2).all(|pair| pair[1] == pair[0] + 1),
        "{seqs:?}"
    );
    let workspace = first
        .seen
        .iter()
        .rev()
        .find(|event| event["type"] == "workspace.created")
        .unwrap()["workspace_id"]
        .clone();
    assert!(workspace.as_u64().is_some());
    let pane = first
        .seen
        .iter()
        .rev()
        .find(|event| event["type"] == "pane.created")
        .unwrap();
    assert_eq!(pane["workspace_id"], workspace);
    assert!(pane["pane_id"].as_u64().is_some());

    let id = control
        .write_request(
            "layout.apply",
            json!({"op": {"op": "rename_surface", "surface": 987_654_321u64, "name": "x"}}),
        )
        .unwrap();
    let refused = loop {
        let line = control.read_stream_line(FRAME_WAIT).unwrap().unwrap();
        let envelope: Value = serde_json::from_str(&line).unwrap();
        if envelope["id"].as_u64() == Some(id) {
            break envelope;
        }
    };
    assert!(refused["error"].is_object(), "{refused}");
    let renamed = control
        .request(
            "layout.apply",
            json!({"op": {"op": "rename_workspace", "workspace": workspace, "title": "renamed"}}),
        )
        .expect("the rename applies");
    assert_eq!(renamed["events"][0]["type"], "workspace.renamed");
    let next = first.next_event();
    assert_eq!(next["type"], "workspace.renamed");
    assert_eq!(
        next["seq"].as_u64(),
        Some(seqs[seqs.len() - 1] + 1),
        "a refused op appends nothing"
    );
    "layout_events_appended_by_the_worker".to_string()
}

fn case_terminal_events_from_the_host_stream(
    owner: &mut HostClient,
    first: &mut Follower,
) -> String {
    let created = start_shell(owner);
    let session = created.manifest.session.to_string();
    let generation = created.manifest.generation;
    let moved_to = std::env::temp_dir().join("paneflow-events-conformance");
    let osc7 = format!("7;file://localhost{}", moved_to.display());
    owner
        .input(
            &created.manifest.session,
            generation,
            osc_command(&osc7).as_bytes(),
        )
        .expect("the directory report reaches the PTY");
    let moved = first.until(|event| {
        event["type"] == "terminal.cwd_changed" && event["session"] == session.as_str()
    });
    assert!(
        moved["data"]["cwd"]
            .as_str()
            .is_some_and(|cwd| cwd.contains("paneflow-events-conformance")),
        "{moved}"
    );
    owner
        .input(
            &created.manifest.session,
            generation,
            exit_command(3).as_bytes(),
        )
        .expect("the exit reaches the PTY");
    let exited = first
        .until(|event| event["type"] == "terminal.exited" && event["session"] == session.as_str());
    assert_eq!(exited["data"]["exit_code"], 3, "{exited}");
    assert_eq!(exited["data"]["lifecycle"], "exited");
    "terminal_events_from_the_host_stream".to_string()
}

fn case_agent_state_changed_from_the_reducer(
    owner: &mut HostClient,
    first: &mut Follower,
) -> String {
    let created = start_shell(owner);
    let session = created.manifest.session.clone();
    let generation = created.manifest.generation;
    owner
        .call(
            METHOD_AGENT_EVENT,
            json!({
                "session": session,
                "kind": "ai.prompt_submit",
                "tool": "claude",
                "emitted_at_ms": 1_000,
                "runtime_generation": generation,
                "hook_payload": {"hook_event_name": "UserPromptSubmit"},
            }),
        )
        .expect("the core accepts the opening hook");
    owner
        .input(
            &session,
            generation,
            osc_command("7501;state=working").as_bytes(),
        )
        .expect("the declaration reaches the PTY");
    let busy = first.until(|event| {
        event["type"] == "agent.state_changed" && event["session"] == session.to_string()
    });
    assert_eq!(busy["data"]["tool"], "claude", "{busy}");
    assert_eq!(busy["data"]["previous"], "idle");
    assert_eq!(busy["data"]["state"], "busy");
    owner
        .input(
            &session,
            generation,
            osc_command("7501;state=done").as_bytes(),
        )
        .expect("the completion reaches the PTY");
    let idle = first.until(|event| {
        event["type"] == "agent.state_changed" && event["session"] == session.to_string()
    });
    assert_eq!(idle["data"]["previous"], "busy");
    assert_eq!(idle["data"]["state"], "idle");
    "agent_state_changed_from_the_reducer".to_string()
}

fn case_header_then_consecutive_seq_to_two_followers(
    start: u64,
    first: &Follower,
    second: &mut Follower,
) -> String {
    let seen = first.seqs();
    let last = *seen.last().expect("the first follower saw events");
    let expected: Vec<u64> = (start + 1..=last).collect();
    assert_eq!(
        seen, expected,
        "the first follower saw every seq once, in order"
    );
    second.until_seq(last);
    assert_eq!(
        second.seqs(),
        expected,
        "a concurrent follower saw the same seq values"
    );
    let types: Vec<&str> = second
        .seen
        .iter()
        .map(|event| event["type"].as_str().unwrap())
        .collect();
    for family in ["pane.", "terminal.", "agent."] {
        assert!(
            types.iter().any(|kind| kind.starts_with(family)),
            "{family} events reached the follower: {types:?}"
        );
    }
    assert_eq!(
        first.seen, second.seen,
        "both followers saw identical events"
    );
    "header_then_consecutive_seq_to_two_followers".to_string()
}

fn case_since_replays_exactly_the_missed_events(
    log: &Log,
    endpoint: &Path,
    epoch: &str,
    first: &Follower,
) -> String {
    let seen = first.seqs();
    let since = seen[seen.len() / 2];
    let mut resumed = follow(endpoint, json!({"since": since, "epoch": epoch}));
    let last = *seen.last().unwrap();
    resumed.until_seq(last);
    let missed: Vec<u64> = seen.iter().copied().filter(|seq| *seq > since).collect();
    assert_eq!(resumed.seqs(), missed, "no gap and no duplicate");
    let live = publish(log, json!([{"type": "surface.renamed", "surface_id": 4}])).unwrap();
    assert_eq!(resumed.next_event()["seq"].as_u64(), Some(live[0]));
    "since_replays_exactly_the_missed_events".to_string()
}

fn case_filters_keep_the_log_seq(log: &Log, endpoint: &Path) -> String {
    let mut typed = follow(endpoint, json!({"types": ["surface.*", "pane.closed"]}));
    let mut scoped = follow(endpoint, json!({"workspace_id": 7}));
    let seqs = publish(
        log,
        json!([
            {"type": "pane.created", "workspace_id": 8, "pane_id": 1},
            {"type": "surface.created", "workspace_id": 7, "surface_id": 2},
            {"type": "pane.closed", "workspace_id": 8, "pane_id": 1},
            {"type": "layout.changed", "workspace_id": 7, "tab_id": 3},
        ]),
    )
    .unwrap();
    let kept = typed.next_event();
    assert_eq!(kept["type"], "surface.created");
    assert_eq!(kept["seq"].as_u64(), Some(seqs[1]));
    let kept = typed.next_event();
    assert_eq!(kept["type"], "pane.closed");
    assert_eq!(kept["seq"].as_u64(), Some(seqs[2]));
    let in_workspace = [scoped.next_event(), scoped.next_event()];
    assert_eq!(in_workspace[0]["seq"].as_u64(), Some(seqs[1]));
    assert_eq!(in_workspace[1]["seq"].as_u64(), Some(seqs[3]));
    "filters_keep_the_log_seq".to_string()
}

fn case_keepalive_when_quiet(endpoint: &Path) -> String {
    let mut quiet = follow(endpoint, json!({"types": ["nothing.*"]}));
    let frame = quiet.frame().expect("the stream stays open");
    assert_eq!(frame, json!({"type": "keepalive"}));
    "keepalive_when_quiet".to_string()
}

fn case_evicted_when_since_left_the_ring_or_the_epoch_differs(
    log: &Log,
    endpoint: &Path,
    epoch: &str,
) -> String {
    let batch: Vec<Value> = (0..256)
        .map(|index| json!({"type": "layout.changed", "tab_id": index}))
        .collect();
    let mut last = 0;
    for _ in 0..5 {
        last = *publish(log, Value::Array(batch.clone()))
            .unwrap()
            .last()
            .unwrap();
    }
    assert!(
        last > 1024 + 1,
        "seq keeps increasing past the ring: {last}"
    );
    let refused = open_follow(endpoint, json!({"since": 1, "epoch": epoch}))
        .err()
        .expect("seq 1 left the ring");
    assert_eq!(refused["code"], ERR_EVICTED, "{refused}");
    assert_eq!(refused["data"]["reason"], "evicted");
    assert_eq!(refused["data"]["seq"].as_u64(), Some(last));
    assert_eq!(refused["data"]["epoch"], epoch);

    let foreign = open_follow(endpoint, json!({"since": last, "epoch": "another-start"}))
        .err()
        .expect("another worker start's epoch is refused");
    assert_eq!(foreign["code"], ERR_EVICTED);
    assert_eq!(foreign["data"]["epoch"], epoch);

    let foreign_from_now = open_follow(endpoint, json!({"epoch": "another-start"}))
        .err()
        .expect("another worker start's epoch is refused with no since too");
    assert_eq!(foreign_from_now["code"], ERR_EVICTED);
    assert_eq!(foreign_from_now["data"]["seq"].as_u64(), Some(last));

    let mut oldest = follow(endpoint, json!({"since": last - 1024, "epoch": epoch}));
    oldest.until_seq(last);
    assert_eq!(
        oldest.seen.len(),
        1024,
        "the ring keeps exactly 1,024 events"
    );
    "evicted_when_since_left_the_ring_or_the_epoch_differs".to_string()
}

fn case_a_follower_over_eight_mebibytes_behind_is_closed_alone(
    log: &Log,
    endpoint: &Path,
) -> String {
    let mut stalled = follow(endpoint, json!({}));
    let reading = follow(endpoint, json!({}));
    let reader = std::thread::spawn(move || {
        let mut reading = reading;
        let mut count = 0usize;
        loop {
            let event = reading.next_event();
            if event["type"] == "layout.resync" {
                return count;
            }
            count += 1;
        }
    });
    let pad = "x".repeat(40 * 1024);
    let total = 8 * 1024 * 1024 / pad.len() + 16;
    for _ in 0..total {
        publish(
            log,
            json!([{"type": "layout.changed", "data": {"pad": pad}}]),
        )
        .unwrap();
    }
    publish(log, json!([{"type": "layout.resync"}])).unwrap();
    assert_eq!(
        reader.join().unwrap(),
        total,
        "a follower that keeps reading receives every event"
    );
    let mut received = 0usize;
    let ended = loop {
        match stalled.frame() {
            Some(frame) if frame.get("seq").is_some() => received += 1,
            Some(frame) if frame["type"] == "end" => break Some(frame),
            Some(_) => {}
            None => break None,
        }
    };
    assert!(
        received < total,
        "the stalled follower was closed before the end of the burst: {received} of {total}"
    );
    if let Some(end) = ended {
        assert!(end["reason"].as_str().unwrap().contains("8 MiB"), "{end}");
    }
    "a_follower_over_eight_mebibytes_behind_is_closed_alone".to_string()
}
