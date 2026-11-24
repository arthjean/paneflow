#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::time::{Duration, Instant};

use paneflow_ipc_client::host_control::HostControl;
use serde_json::{Value, json};

mod common;

const PATIENCE: Duration = Duration::from_secs(30);

const INIT_LUA: &str = "paneflow.action{ name = 'deploy', title = 'Deploy', agents = true, args = { env = 'string' },\n\
   run = function(ctx, args) return { env = args.env, origin = ctx.origin } end }\n\
 paneflow.action{ name = 'secret', title = 'Secret', run = function() return 'hidden' end }\n\
 paneflow.action{ name = 'slow', title = 'Slow', agents = true, run = function() paneflow.sleep(1.5) return 'slept' end }\n\
 paneflow.action{ name = 'confirm', title = 'Confirm deploy', agents = true, run = function()\n\
   local answer, reason = paneflow.ui.input{ title = 'Type yes' }\n\
   return answer or reason end }\n";

fn call(home: &Path, method: &str, params: Value) -> Result<Value, String> {
    HostControl::connect(
        &paneflow_home::serve_endpoint_path(home),
        "agent-actions-test",
    )?
    .request_with_deadline(method, params, Duration::from_secs(60))
}

fn open(home: &Path) -> paneflow_serve::worker::RunningWorker {
    std::fs::write(paneflow_home::init_lua_path_in(home), INIT_LUA).unwrap();
    let worker = paneflow_serve::open(home).unwrap();
    let deadline = Instant::now() + PATIENCE;
    loop {
        let status = call(home, "config.status", json!({})).unwrap();
        if status["actions"] == 4 {
            return worker;
        }
        assert!(Instant::now() < deadline, "init.lua never loaded: {status}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn as_agent(session: &str, name: &str, extra: Value) -> Value {
    let mut params = json!({"name": name, "ctx": {"origin": "agent", "session": session}});
    if let (Some(params), Some(extra)) = (params.as_object_mut(), extra.as_object()) {
        params.extend(extra.clone());
    }
    params
}

fn write_session_with(home: &Path, session: &paneflow_config::schema::SessionId) {
    use paneflow_config::schema::{
        LayoutNode, SESSION_SCHEMA_VERSION, SessionState, SurfaceDefinition, TabLayoutIds,
        TabSession, WorkspaceSession,
    };
    let state = SessionState {
        version: SESSION_SCHEMA_VERSION,
        active_workspace: 0,
        workspaces: vec![WorkspaceSession {
            id: None,
            layout_id: Some(10),
            title: "agent".into(),
            cwd: "/work".into(),
            tabs: vec![TabSession {
                title: "claude".into(),
                layout: Some(LayoutNode::Pane {
                    surfaces: vec![SurfaceDefinition {
                        surface_type: Some("terminal".into()),
                        cwd: Some("/work".into()),
                        session: Some(session.clone()),
                        ..SurfaceDefinition::default()
                    }],
                }),
                layout_ids: Some(TabLayoutIds {
                    tab: 11,
                    panes: vec![12],
                    surfaces: vec![vec![13]],
                }),
                ..TabSession::default()
            }],
            active_tab: 0,
            legacy_layout: None,
            custom_buttons: Vec::new(),
            expanded_paths: Vec::new(),
            managed_worktrees: Vec::new(),
            sidebar_collapsed: false,
            muted: false,
            space: None,
        }],
        detached_panes: Vec::new(),
        active_space: None,
        next_layout_id: Some(20),
        layout_revision: Some(1),
    };
    std::fs::write(
        home.join("session.json"),
        serde_json::to_vec_pretty(&state).unwrap(),
    )
    .unwrap();
}

#[test]
fn an_agent_action_runs_with_the_ids_of_the_agents_own_pane_whatever_ids_it_claims() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        paneflow_home::init_lua_path_in(home.path()),
        "paneflow.action{ name = 'where', title = 'Where', agents = true, run = function(ctx)\n\
           return { surface = ctx.surface_id, workspace = ctx.workspace_id, origin = ctx.origin } end }\n",
    )
    .unwrap();
    let session = paneflow_config::schema::SessionId::new();
    write_session_with(home.path(), &session);
    let worker = common::open_starting_terminals(home.path());
    let deadline = Instant::now() + PATIENCE;
    while call(home.path(), "config.status", json!({})).unwrap()["actions"] != 1 {
        assert!(Instant::now() < deadline, "init.lua never loaded");
        std::thread::sleep(Duration::from_millis(20));
    }
    let (surface, workspace) = (13, 10);
    let ran = call(
        home.path(),
        "action.invoke",
        json!({"name": "where", "ctx": {"origin": "agent", "session": session.to_string(), "surface_id": 999_999, "workspace_id": 999_998}}),
    )
    .unwrap();
    assert_eq!(
        ran,
        json!({"result": {"surface": surface, "workspace": workspace, "origin": "agent"}})
    );
    worker.stop();
}

#[test]
fn an_agent_runs_only_the_actions_marked_for_agents_and_hears_one_refusal_for_the_rest() {
    let home = tempfile::tempdir().unwrap();
    let worker = open(home.path());
    let ran = call(
        home.path(),
        "action.invoke",
        as_agent("a1", "deploy", json!({"args": {"env": "staging"}})),
    )
    .unwrap();
    assert_eq!(
        ran,
        json!({"result": {"env": "staging", "origin": "agent"}})
    );
    let unmarked = call(
        home.path(),
        "action.invoke",
        as_agent("a1", "secret", json!({})),
    )
    .unwrap_err();
    let unknown = call(
        home.path(),
        "action.invoke",
        as_agent("a1", "nonexistent", json!({})),
    )
    .unwrap_err();
    assert_eq!(
        unmarked.replace("secret", "NAME"),
        unknown.replace("nonexistent", "NAME"),
        "an unmarked action is refused exactly like a missing one"
    );
    assert!(unmarked.contains("no action named 'secret'"), "{unmarked}");
    assert_eq!(
        call(
            home.path(),
            "action.invoke",
            json!({"name": "secret", "ctx": {"origin": "cli"}})
        )
        .unwrap(),
        json!({"result": "hidden"}),
        "the user still runs every action"
    );
    let listed = call(home.path(), "action.list", json!({})).unwrap();
    let marked: Vec<&str> = listed["actions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|action| action["agents"] == true)
        .map(|action| action["name"].as_str().unwrap())
        .collect();
    assert_eq!(marked, ["deploy", "slow", "confirm"]);
    worker.stop();
}

#[test]
fn a_run_still_going_after_its_wait_answers_running_with_a_handle_a_claimed_session_cannot_use() {
    let home = tempfile::tempdir().unwrap();
    let worker = open(home.path());
    let started = call(
        home.path(),
        "action.invoke",
        as_agent("a2", "slow", json!({"wait_ms": 100})),
    )
    .unwrap();
    assert_eq!(started["status"], "running", "{started}");
    let run_id = started["run_id"].as_str().expect("a run id");
    assert_eq!(run_id.len(), 32, "{started}");
    assert!(run_id.chars().all(|c| c.is_ascii_hexdigit()), "{started}");
    assert!(
        call(
            home.path(),
            "action.wait",
            json!({"run_id": run_id, "wait_ms": 25_001})
        )
        .is_err(),
        "'wait_ms' is bounded"
    );
    let outside = call(
        home.path(),
        "action.wait",
        json!({"run_id": run_id, "wait_ms": 25_000}),
    )
    .unwrap_err();
    assert!(
        outside.contains("no action run with id"),
        "a caller outside every pane is not the agent it names: {outside}"
    );
    let guessed = call(home.path(), "action.wait", json!({"run_id": "1"})).unwrap_err();
    assert!(guessed.contains("no action run with id"), "{guessed}");
    let quick = call(
        home.path(),
        "action.invoke",
        as_agent(
            "a3",
            "deploy",
            json!({"args": {"env": "prod"}, "wait_ms": 20_000}),
        ),
    )
    .unwrap();
    assert_eq!(
        quick,
        json!({"result": {"env": "prod", "origin": "agent"}, "status": "done"})
    );
    worker.stop();
}

#[test]
fn an_agent_past_its_burst_is_rate_limited_before_its_action_runs() {
    let home = tempfile::tempdir().unwrap();
    let worker = open(home.path());
    let deploy = || {
        call(
            home.path(),
            "action.invoke",
            as_agent("a4", "deploy", json!({"args": {"env": "x"}})),
        )
        .unwrap()
    };
    for _ in 0..3 {
        assert_eq!(deploy()["result"]["env"], "x");
    }
    assert_eq!(deploy(), json!({"status": "rate_limited"}));
    assert_eq!(
        call(
            home.path(),
            "action.invoke",
            as_agent("a5", "deploy", json!({"args": {"env": "y"}}))
        )
        .unwrap()["result"]["env"],
        "y",
        "another agent has its own budget"
    );
    worker.stop();
}

#[test]
fn a_question_from_an_action_an_agent_runs_names_the_agent_and_the_action() {
    let home = tempfile::tempdir().unwrap();
    let worker = open(home.path());
    let mut follower =
        HostControl::connect(&paneflow_home::serve_endpoint_path(home.path()), "window").unwrap();
    let header = follower
        .request("prompt.follow", json!({"kinds": ["input", "pick"]}))
        .unwrap();
    assert_eq!(header["type"], "following");
    let running = {
        let home = home.path().to_path_buf();
        std::thread::spawn(move || {
            let mut params = as_agent("a6", "confirm", json!({}));
            params["ctx"]["agent"] = json!("claude");
            call(&home, "action.invoke", params).unwrap()
        })
    };
    let shown = loop {
        let line = follower.read_stream_line(PATIENCE).unwrap().unwrap();
        let frame: Value = serde_json::from_str(&line).unwrap();
        if frame["type"] == "requested" {
            break frame;
        }
    };
    assert_eq!(shown["origin"]["kind"], "action");
    assert_eq!(shown["origin"]["title"], "Confirm deploy");
    assert_eq!(shown["origin"]["agent"], "Claude Code");
    call(
        home.path(),
        "prompt.reply",
        json!({"id": shown["id"], "token": shown["token"], "answer": "yes"}),
    )
    .unwrap();
    assert_eq!(running.join().unwrap(), json!({"result": "yes"}));
    worker.stop();
}
