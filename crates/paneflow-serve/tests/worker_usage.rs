#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use paneflow_host::protocol::{ClientHello, METHOD_AGENT_EVENT};
use paneflow_host::{HostClient, SessionSummary};
use paneflow_ipc_client::host_control::{HostControl, METHOD_AGENT_FOLLOW};
use paneflow_serve::protocol::{METHOD_USAGE_IMPORT, METHOD_USAGE_REFRESH, METHOD_USAGE_SNAPSHOT};
use serde_json::{Value, json};

fn shell_params(env: Value) -> Value {
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
        "env": env,
    })
}

fn declare_command(state: &str) -> String {
    #[cfg(windows)]
    return format!(
        "powershell -NoProfile -Command \"[Console]::Write([char]27 + ']7501;state={state}' + [char]7)\"\r\n"
    );
    #[cfg(unix)]
    return format!("printf '\\033]7501;state={state}\\007'\n");
}

fn canonical(path: &Path) -> String {
    paneflow_agent_config::usage::accounts::canonical_account_dir(path)
        .display()
        .to_string()
}

fn wait_for_usage(follower: &mut HostControl, accept: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let Ok(Some(line)) = follower.read_stream_line(Duration::from_secs(2)) else {
            continue;
        };
        let frame: Value = serde_json::from_str(&line).unwrap();
        if frame["type"] == "usage" && accept(&frame["usage"]) {
            return frame["usage"].clone();
        }
    }
    panic!("no matching usage frame within 30 s");
}

fn plan<'a>(usage: &'a Value, agent: &str, dir: &str) -> Option<&'a Value> {
    usage["plans"]
        .as_array()?
        .iter()
        .find(|plan| plan["agent"] == agent && plan["config_dir"] == dir)
}

fn day_tally(snapshot: &Value, agent: &str, dir: &str) -> (u64, u64) {
    let ledger = &snapshot["ledger"];
    let Some(index) = ledger["accounts"].as_array().and_then(|accounts| {
        accounts
            .iter()
            .position(|account| account["agent"] == agent && account["config_dir"] == dir)
    }) else {
        return (0, 0);
    };
    ledger["days"]
        .as_object()
        .into_iter()
        .flat_map(|days| days.values())
        .flat_map(|entries| entries.as_array().cloned().unwrap_or_default())
        .filter(|entry| entry["account"].as_u64() == Some(index as u64))
        .fold((0, 0), |(turns, active), entry| {
            (
                turns + entry["turns"].as_u64().unwrap_or(0),
                active + entry["active_ms"].as_u64().unwrap_or(0),
            )
        })
}

fn day_tokens(snapshot: &Value, agent: &str, dir: &str) -> (u64, u64) {
    let ledger = &snapshot["ledger"];
    let Some(index) = ledger["accounts"].as_array().and_then(|accounts| {
        accounts
            .iter()
            .position(|account| account["agent"] == agent && account["config_dir"] == dir)
    }) else {
        return (0, 0);
    };
    ledger["days"]
        .as_object()
        .into_iter()
        .flat_map(|days| days.values())
        .flat_map(|entries| entries.as_array().cloned().unwrap_or_default())
        .filter(|entry| entry["account"].as_u64() == Some(index as u64))
        .fold((0, 0), |(input, output), entry| {
            (
                input + entry["tokens_in"].as_u64().unwrap_or(0),
                output + entry["tokens_out"].as_u64().unwrap_or(0),
            )
        })
}

fn write_claude_transcript(config_dir: &Path) -> PathBuf {
    let dir = config_dir.join("projects").join("C--work");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("b08fea1d-0871-4dd1-acbb-f05e3b9d348b.jsonl");
    let assistant = |id: &str, input: u64, output: u64| {
        json!({
            "type": "assistant",
            "timestamp": "2026-10-31T22:09:12.042Z",
            "message": {
                "id": id,
                "usage": {
                    "input_tokens": input,
                    "cache_creation_input_tokens": 36_935,
                    "cache_read_input_tokens": 30_180,
                    "output_tokens": output,
                },
            },
        })
    };
    let lines = [
        json!({"type": "user", "timestamp": "2026-10-31T22:09:00.000Z"}),
        assistant("msg_01", 2, 172),
        assistant("msg_01", 2, 172),
        assistant("msg_02", 5, 40),
    ];
    let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
    std::fs::write(&path, text).unwrap();
    path
}

fn reopen(home: &Path) -> paneflow_serve::worker::RunningWorker {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match paneflow_serve::open(home) {
            Ok(worker) => return worker,
            Err(paneflow_serve::WorkerError::Endpoint { source, .. })
                if source.kind() == std::io::ErrorKind::PermissionDenied
                    && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => panic!("the worker takes the home again: {error:?}"),
        }
    }
}

fn write_rollout(codex_home: &Path) -> PathBuf {
    let dir = codex_home
        .join("sessions")
        .join("2026")
        .join("10")
        .join("31");
    std::fs::create_dir_all(&dir).unwrap();
    let resets_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 20 * 86_400;
    let line = json!({
        "timestamp": "2026-10-31T22:09:12.042Z",
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": {"total_token_usage": {"input_tokens": 10, "output_tokens": 2}},
            "rate_limits": {
                "limit_id": "codex",
                "primary": {"used_percent": 3.0, "window_minutes": 43_200, "resets_at": resets_at},
                "secondary": null,
                "plan_type": "free"
            }
        }
    });
    let path = dir.join("rollout-2026-10-31T23-08-26-test.jsonl");
    std::fs::write(&path, format!("{line}\n")).unwrap();
    path
}

const ALLOWED_PLAN_KEYS: &[&str] = &[
    "version",
    "accounts",
    "agent",
    "config_dir",
    "label",
    "state",
    "primary",
    "secondary",
    "received_at_ms",
    "agent_version",
    "updated_at_ms",
    "used_percent",
    "window_secs",
    "resets_at",
];

fn object_keys(value: &Value, keys: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                keys.push(key.clone());
                object_keys(inner, keys);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| object_keys(item, keys)),
        _ => {}
    }
}

#[test]
fn the_worker_stores_publishes_and_reloads_plan_limits_and_counts_turns() {
    let home = tempfile::tempdir().unwrap();
    let agents = tempfile::tempdir().unwrap();
    let claude_work = agents.path().join("claude-work");
    let codex_work = agents.path().join("codex-work");
    let codex_default = agents.path().join("codex-default");
    for dir in [&claude_work, &codex_work, &codex_default] {
        std::fs::create_dir_all(dir).unwrap();
    }
    unsafe {
        std::env::set_var(paneflow_home::HOME_ENV, home.path());
        std::env::set_var("CODEX_HOME", &codex_default);
    }

    let endpoint = paneflow_home::host_endpoint_path(home.path());
    let core = paneflow_host::SessionHost::open(home.path(), &endpoint).expect("a core opens");
    let core_server = paneflow_host::ServerHandle::spawn(Arc::clone(&core), endpoint.clone())
        .expect("the core serves its endpoint");
    let mut owner = HostClient::connect(&endpoint, &ClientHello::local("worker-usage-test"))
        .expect("the core accepts a controller");
    let create = |owner: &mut HostClient, env: Value| -> SessionSummary {
        serde_json::from_value(owner.call("session.create", shell_params(env)).unwrap()).unwrap()
    };
    let claude = create(
        &mut owner,
        json!({"CLAUDE_CONFIG_DIR": claude_work.display().to_string()}),
    );
    let codex = create(
        &mut owner,
        json!({"CODEX_HOME": codex_work.display().to_string()}),
    );

    let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");
    let connected_by = Instant::now() + Duration::from_secs(10);
    while worker.worker().snapshot_frame()["core_connected"] != true {
        assert!(Instant::now() < connected_by, "the worker follows the core");
        std::thread::sleep(Duration::from_millis(20));
    }
    let worker_endpoint = paneflow_home::serve_endpoint_path(home.path());
    let mut follower = HostControl::connect(&worker_endpoint, "worker-usage-follower").unwrap();
    let header = follower.request(METHOD_AGENT_FOLLOW, json!({})).unwrap();
    assert!(header["usage"]["plans"].is_array(), "{header}");

    let mut relay =
        HostClient::connect(&endpoint, &ClientHello::control("paneflow-ai-hook")).unwrap();
    let claude_dir = canonical(&claude_work);
    let sent_at = Instant::now();
    relay
        .call(
            METHOD_AGENT_EVENT,
            json!({
                "session": claude.manifest.session,
                "kind": "status_line",
                "tool": "claude",
                "hook_payload": {
                    "session_id": "b08fea1d-0871-4dd1-acbb-f05e3b9d348b",
                    "config_dir": claude_work.display().to_string(),
                    "rate_limits": {
                        "five_hour": {"used_percentage": 13, "resets_at": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + 3_600},
                        "seven_day": {"used_percentage": 59, "resets_at": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + 3 * 86_400},
                        "five_hour_extra": {"used_percentage": 250}
                    },
                },
            }),
        )
        .unwrap();
    let usage = wait_for_usage(&mut follower, |usage| {
        plan(usage, "claude-code", &claude_dir).is_some()
    });
    let published_in = sent_at.elapsed();
    assert!(
        published_in < Duration::from_secs(1),
        "the relay frame reaches followers in {published_in:?}"
    );
    let claude_plan = plan(&usage, "claude-code", &claude_dir).unwrap();
    assert_eq!(claude_plan["state"], "reported");
    assert_eq!(
        claude_plan["label"], claude_dir,
        "a directory no inventory knows is kept and labelled by its path"
    );
    assert_eq!(claude_plan["primary"]["used_percent"], 13.0);
    assert_eq!(claude_plan["secondary"]["used_percent"], 59.0);
    assert_eq!(claude_plan["secondary"]["window_secs"], 7 * 86_400);

    let plans_path = paneflow_home::usage_plans_path_in(home.path());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !plans_path.is_file() {
        assert!(Instant::now() < deadline, "plans.json is written");
        std::thread::sleep(Duration::from_millis(20));
    }
    relay
        .call(
            METHOD_AGENT_EVENT,
            json!({
                "session": claude.manifest.session,
                "kind": "status_line",
                "tool": "claude",
                "hook_payload": {"config_dir": claude_work.display().to_string(), "rate_limits": null},
            }),
        )
        .unwrap();
    let usage = wait_for_usage(&mut follower, |usage| {
        plan(usage, "claude-code", &claude_dir)
            .is_some_and(|plan| plan["state"] == "no_limits_reported")
    });
    assert_eq!(
        plan(&usage, "claude-code", &claude_dir).unwrap()["secondary"]["used_percent"],
        59.0,
        "the previous snapshot is kept"
    );
    let on_disk: Value = serde_json::from_slice(&std::fs::read(&plans_path).unwrap()).unwrap();
    assert_eq!(
        on_disk["accounts"][0]["state"], "reported",
        "a second change within 10 s waits for the next write"
    );

    let claude_generation = claude.manifest.generation;
    let hook = |relay: &mut HostClient, session: &SessionSummary, kind: &str, tool: &str| {
        relay
            .call(
                METHOD_AGENT_EVENT,
                json!({
                    "session": session.manifest.session,
                    "kind": kind,
                    "tool": tool,
                    "runtime_generation": session.manifest.generation,
                }),
            )
            .unwrap();
    };
    hook(&mut relay, &claude, "ai.prompt_submit", "claude");
    owner
        .input(
            &claude.manifest.session,
            claude_generation,
            declare_command("working").as_bytes(),
        )
        .unwrap();
    std::thread::sleep(Duration::from_millis(1_500));
    owner
        .input(
            &claude.manifest.session,
            claude_generation,
            declare_command("done").as_bytes(),
        )
        .unwrap();
    let mut control = HostControl::connect(&worker_endpoint, "worker-usage-control").unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let (turns, active_ms) = loop {
        let snapshot = control.request(METHOD_USAGE_SNAPSHOT, json!({})).unwrap();
        let (turns, active_ms) = day_tally(&snapshot, "claude-code", &claude_dir);
        if turns == 1 && active_ms > 0 {
            break (turns, active_ms);
        }
        assert!(Instant::now() < deadline, "the turn is booked: {snapshot}");
        std::thread::sleep(Duration::from_millis(100));
    };
    let transcript = write_claude_transcript(&claude_work);
    relay
        .call(
            METHOD_AGENT_EVENT,
            json!({
                "session": claude.manifest.session,
                "kind": "ai.stop",
                "tool": "claude",
                "runtime_generation": claude.manifest.generation,
                "hook_payload": {
                    "session_id": "b08fea1d-0871-4dd1-acbb-f05e3b9d348b",
                    "transcript_path": transcript.display().to_string(),
                },
            }),
        )
        .unwrap();
    assert_eq!(turns, 1);
    assert!(active_ms < 60_000, "{active_ms}");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let snapshot = control.request(METHOD_USAGE_SNAPSHOT, json!({})).unwrap();
        let tokens = day_tokens(&snapshot, "claude-code", &claude_dir);
        if tokens != (0, 0) {
            assert_eq!(
                tokens,
                (7, 212),
                "each message counts once and cache tokens are excluded"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the Stop transcript is read: {snapshot}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    write_rollout(&codex_work);
    let codex_dir = canonical(&codex_work);
    hook(&mut relay, &codex, "ai.prompt_submit", "codex");
    hook(&mut relay, &codex, "ai.stop", "codex");
    let usage = wait_for_usage(&mut follower, |usage| {
        plan(usage, "codex", &codex_dir).is_some()
    });
    let codex_plan = plan(&usage, "codex", &codex_dir).unwrap();
    assert_eq!(codex_plan["state"], "reported");
    assert_eq!(codex_plan["primary"]["used_percent"], 3.0);
    assert_eq!(codex_plan["primary"]["window_secs"], 43_200 * 60);

    let refreshed = control.request(METHOD_USAGE_REFRESH, json!({})).unwrap();
    let default_dir = canonical(&codex_default);
    assert_eq!(
        plan(&refreshed, "codex", &default_dir).map(|plan| plan["state"].clone()),
        Some(json!("no_codex_data")),
        "the default CODEX_HOME has no rollout yet: {refreshed}"
    );
    let snapshot = control.request(METHOD_USAGE_SNAPSHOT, json!({})).unwrap();
    assert_eq!(day_tally(&snapshot, "codex", &codex_dir).0, 1);
    assert_eq!(
        day_tokens(&snapshot, "codex", &codex_dir),
        (10, 2),
        "the rollout read books the growth of its cumulative totals"
    );

    drop(follower);
    drop(control);
    worker.stop();
    let restarted = reopen(home.path());
    let usage = restarted.worker().snapshot_frame()["usage"].clone();
    assert_eq!(
        plan(&usage, "claude-code", &claude_dir).unwrap()["state"],
        "no_limits_reported",
        "the stop flush wrote the deferred change"
    );
    assert_eq!(
        plan(&usage, "codex", &codex_dir).unwrap()["primary"]["used_percent"],
        3.0
    );
    let reloaded = restarted.worker().lock_usage().snapshot();
    assert_eq!(
        day_tally(&reloaded, "claude-code", &claude_dir),
        (turns, active_ms)
    );
    assert_eq!(day_tokens(&reloaded, "claude-code", &claude_dir), (7, 212));
    let ledger: Value = serde_json::from_slice(
        &std::fs::read(paneflow_home::usage_ledger_path_in(home.path())).unwrap(),
    )
    .unwrap();
    let cursor = &ledger["cursors"][canonical(&transcript)];
    assert_eq!(
        cursor["offset"].as_u64(),
        Some(std::fs::metadata(&transcript).unwrap().len()),
        "the transcript offset is kept under cursors: {ledger}"
    );

    let plans: Value = serde_json::from_slice(&std::fs::read(&plans_path).unwrap()).unwrap();
    let mut keys = Vec::new();
    object_keys(&plans, &mut keys);
    for key in &keys {
        assert!(
            ALLOWED_PLAN_KEYS.contains(&key.as_str()),
            "plans.json holds `{key}`"
        );
    }
    let text = plans.to_string();
    assert!(
        !text.contains("b08fea1d"),
        "no provider session id is stored"
    );

    restarted.stop();
    for summary in [&claude, &codex] {
        let _ = owner.call("session.stop", json!({"session": summary.manifest.session}));
    }
    core_server.stop().unwrap();
    drop(core);
}

fn finished_import(control: &mut HostControl, files: u64) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let snapshot = control.request(METHOD_USAGE_SNAPSHOT, json!({})).unwrap();
        if snapshot["import"]["running"] == false && snapshot["import"]["files"] == files {
            return snapshot;
        }
        assert!(Instant::now() < deadline, "the import finishes: {snapshot}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn import_past_activity_books_the_past_days_of_each_account_once() {
    let home = tempfile::tempdir().unwrap();
    let claude = home.path().join("claude-import");
    let codex = home.path().join("codex-import");
    let stamp = (chrono::Utc::now() - chrono::Duration::days(3)).to_rfc3339();
    let transcript = claude.join("projects").join("C--work");
    std::fs::create_dir_all(&transcript).unwrap();
    let claude_lines = [
        json!({"type": "user", "timestamp": stamp, "message": {"role": "user", "content": "fix the build"}}),
        json!({"type": "assistant", "timestamp": stamp, "message": {"id": "msg_a", "usage": {"input_tokens": 4, "output_tokens": 40}}}),
        json!({"type": "user", "timestamp": stamp, "message": {"content": [{"type": "tool_result", "tool_use_id": "t"}]}}),
        json!({"type": "user", "timestamp": stamp, "message": {"role": "user", "content": "now the tests"}}),
        json!({"type": "assistant", "timestamp": stamp, "message": {"id": "msg_b", "usage": {"input_tokens": 1, "output_tokens": 10}}}),
    ];
    std::fs::write(
        transcript.join("s.jsonl"),
        claude_lines.map(|line| format!("{line}\n")).concat(),
    )
    .unwrap();
    let rollouts = codex.join("sessions").join("2026").join("10").join("28");
    std::fs::create_dir_all(&rollouts).unwrap();
    let codex_lines = [
        json!({"timestamp": stamp, "type": "event_msg", "payload": {"type": "task_started"}}),
        json!({"timestamp": stamp, "type": "event_msg", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 30, "cached_input_tokens": 10, "output_tokens": 7}}}}),
    ];
    std::fs::write(
        rollouts.join("rollout-2026-10-28T10-00-00-a.jsonl"),
        codex_lines.map(|line| format!("{line}\n")).concat(),
    )
    .unwrap();

    let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");
    let endpoint = paneflow_home::serve_endpoint_path(home.path());
    let mut control = HostControl::connect(&endpoint, "worker-usage-import").unwrap();
    let params = json!({
        "accounts": [
            {"agent": "claude-code", "config_dir": claude.display().to_string()},
            {"agent": "codex", "config_dir": codex.display().to_string()},
        ],
        "force": true,
    });
    let started = control
        .request(METHOD_USAGE_IMPORT, params.clone())
        .unwrap();
    assert!(started["import"].is_object(), "{started}");
    let snapshot = finished_import(&mut control, 2);
    let claude_dir = canonical(&claude);
    let codex_dir = canonical(&codex);
    assert_eq!(day_tally(&snapshot, "claude-code", &claude_dir).0, 2);
    assert_eq!(day_tokens(&snapshot, "claude-code", &claude_dir), (5, 50));
    assert_eq!(day_tally(&snapshot, "codex", &codex_dir).0, 1);
    assert_eq!(day_tokens(&snapshot, "codex", &codex_dir), (20, 7));

    control.request(METHOD_USAGE_IMPORT, params).unwrap();
    let again = finished_import(&mut control, 2);
    assert_eq!(
        again["ledger"]["days"], snapshot["ledger"]["days"],
        "a second import adds nothing"
    );

    drop(control);
    worker.stop();
    let restarted = reopen(home.path());
    let usage = restarted.worker().lock_usage().snapshot();
    assert_eq!(usage["import"]["files"], 2, "the import record persists");
    assert!(
        !restarted.worker().lock_usage().begin_import(false),
        "a finished import is not repeated at the next start"
    );
    restarted.stop();
}
