use std::path::Path;
use std::sync::Arc;

use paneflow_api::Principal;
use paneflow_api::grants::{GrantHolder, GrantOrigin, GrantScope, Right};
use paneflow_config::schema::{SessionId, WorkspaceId};
use paneflow_host::manifest::SessionLifecycle;
use serde_json::{Value, json};

use crate::core_link::CoreFrame;
use crate::pane_grants::NewGrant;
use crate::registry::registry;
use crate::server::Worker;

const CLOSED: &str = r#"{"ipc": {"scripting": false, "orchestration": false}}"#;

fn open(home: &Path, config: &str) -> crate::worker::RunningWorker {
    std::fs::write(home.join("paneflow.json"), config).unwrap();
    crate::open(home).unwrap()
}

struct Accepting;

impl crate::launcher::SessionStarter for Accepting {
    fn connect(
        &self,
    ) -> Result<Box<dyn crate::launcher::SessionLink>, crate::launcher::LaunchError> {
        Ok(Box::new(Accepting))
    }
}

impl crate::launcher::SessionLink for Accepting {
    fn exists(&mut self, _: &SessionId) -> Result<bool, crate::launcher::LaunchError> {
        Ok(false)
    }

    fn create(
        &mut self,
        _: &paneflow_host::CreateSession,
    ) -> Result<(), crate::launcher::LaunchError> {
        Ok(())
    }
}

fn open_accepting(home: &Path, config: &str) -> crate::worker::RunningWorker {
    std::fs::write(home.join("paneflow.json"), config).unwrap();
    crate::open_with(
        home,
        crate::Launching {
            support: crate::launcher::LaunchSupport::default(),
            starter: Some(Arc::new(Accepting)),
        },
    )
    .unwrap()
}

fn row(session: &SessionId, generation: u64, workspace: &WorkspaceId) -> Value {
    json!({
        "session": session,
        "generation": generation,
        "workspace": workspace,
        "live": true,
        "lifecycle": SessionLifecycle::Running,
        "host_protocol_version": paneflow_host::HOST_PROTOCOL_VERSION,
        "host_build_id": "test-build",
    })
}

fn snapshot(worker: &Arc<Worker>, rows: Vec<Value>) {
    crate::worker::apply(worker, CoreFrame::Snapshot(rows), true);
}

fn pane(session: &SessionId, workspace: &WorkspaceId) -> Principal {
    Principal::Pane {
        session: session.to_string(),
        workspace: Some(workspace.to_string()),
    }
}

fn call(
    worker: &Arc<Worker>,
    method: &str,
    params: Value,
    acting: &Principal,
) -> Result<Value, Value> {
    registry()
        .unwrap()
        .call(worker, method, &params, acting)
        .map_err(|error| json!({"code": error.code, "message": error.message, "data": error.data}))
}

fn grant_for(
    session: &SessionId,
    workspace: &WorkspaceId,
    rights: &[Right],
    expires: Option<u64>,
) -> NewGrant {
    NewGrant {
        holder: GrantHolder {
            session: session.to_string(),
            generation: 1,
            workspace: Some(workspace.to_string()),
        },
        rights: rights.to_vec(),
        scope: GrantScope::Workspace,
        expires_at_ms: expires,
        delegated_from: None,
        origin: GrantOrigin::Prompt,
    }
}

fn hook_event(session: &SessionId, tool: &str) -> Value {
    json!({
        "session": session.to_string(),
        "kind": "ai.prompt_submit",
        "tool": tool,
        "pid": 42,
        "revision": 1,
        "hook_payload": {"hook_event_name": "UserPromptSubmit"},
    })
}

#[test]
fn a_live_grant_opens_the_spawn_gate_of_a_layout_op_and_a_restarted_holder_loses_it() {
    let home = tempfile::tempdir().unwrap();
    let running = open(home.path(), CLOSED);
    let worker = running.worker();
    let (session, workspace) = (SessionId::new(), WorkspaceId::new());
    snapshot(worker, vec![row(&session, 1, &workspace)]);
    let caller = pane(&session, &workspace);
    let descriptor = paneflow_api::Descriptor::of::<paneflow_api::methods::layout::Apply>();
    let closed =
        crate::authority::authorize_call(worker, &caller, &descriptor, &mut json!({})).unwrap();
    assert!(!closed.may_spawn());
    let (_, change) = worker.pane_grants.create(
        grant_for(&session, &workspace, &[Right::Layout], None),
        crate::state::now_ms(),
    );
    worker.publish_grant_change(change);
    assert!(
        !crate::authority::authorize_call(worker, &caller, &descriptor, &mut json!({}))
            .unwrap()
            .may_spawn(),
        "a workspace grant opens no layout op, which reaches every workspace"
    );
    let (_, change) = worker.pane_grants.create(
        NewGrant {
            scope: GrantScope::Instance,
            ..grant_for(&session, &workspace, &[Right::Layout], None)
        },
        crate::state::now_ms(),
    );
    worker.publish_grant_change(change);
    let open =
        crate::authority::authorize_call(worker, &caller, &descriptor, &mut json!({})).unwrap();
    assert!(open.may_spawn());
    let split = paneflow_api::Descriptor::of::<paneflow_api::methods::surface::Split>();
    assert!(
        !crate::authority::authorize_call(worker, &caller, &split, &mut json!({}))
            .unwrap()
            .may_spawn(),
        "a layout grant does not open the spawn right"
    );
    snapshot(worker, vec![row(&session, 2, &workspace)]);
    assert!(worker.pane_grants.set().grants.is_empty());
    assert!(
        !crate::authority::authorize_call(worker, &caller, &descriptor, &mut json!({}))
            .unwrap()
            .may_spawn()
    );
    running.stop();
}

#[test]
fn grants_of_live_sessions_survive_a_worker_restart() {
    let home = tempfile::tempdir().unwrap();
    let (session, workspace) = (SessionId::new(), WorkspaceId::new());
    let id = {
        let running = open(home.path(), CLOSED);
        let worker = running.worker();
        snapshot(worker, vec![row(&session, 1, &workspace)]);
        let (created, change) = worker.pane_grants.create(
            grant_for(&session, &workspace, &[Right::Write], None),
            crate::state::now_ms(),
        );
        worker.publish_grant_change(change);
        worker.pane_grants.create(
            grant_for(&session, &workspace, &[Right::Write], Some(1)),
            crate::state::now_ms(),
        );
        running.stop();
        created.id
    };
    let running = open(home.path(), CLOSED);
    let worker = running.worker();
    let ids: Vec<String> = worker
        .pane_grants
        .set()
        .grants
        .into_iter()
        .map(|grant| grant.id)
        .collect();
    assert_eq!(ids, vec![id]);
    running.stop();
}

fn observed(mut row: Value, runtime_id: &str) -> Value {
    row["observed_runtime"] = json!({
        "id": runtime_id,
        "pid": 4242,
        "pid_started_at": 7,
        "process_group": 4242,
        "process_name": "agent",
    });
    row
}

const CLAUDE_RUNTIME: &str = "com.anthropic.claude-code";

#[test]
fn a_profile_grants_the_panes_paneflow_sees_running_its_agent_and_no_self_declared_one() {
    let home = tempfile::tempdir().unwrap();
    let running = open(
        home.path(),
        r#"{"ipc": {}, "agents": {"claude": {"grants": [{"rights": ["write"], "scope": "workspace"}]},
            "codex": {"grants": [{"rights": ["admin"]}]}}}"#,
    );
    let worker = running.worker();
    let workspace = WorkspaceId::new();
    let (launched, later, claimed, plain) = (
        SessionId::new(),
        SessionId::new(),
        SessionId::new(),
        SessionId::new(),
    );
    snapshot(
        worker,
        vec![
            observed(row(&launched, 1, &workspace), CLAUDE_RUNTIME),
            row(&later, 1, &workspace),
            row(&claimed, 1, &workspace),
            row(&plain, 1, &workspace),
        ],
    );
    let holders = |worker: &Arc<Worker>| -> Vec<String> {
        let mut held: Vec<String> = worker
            .pane_grants
            .set()
            .grants
            .into_iter()
            .map(|grant| grant.holder.session)
            .collect();
        held.sort();
        held
    };
    assert_eq!(holders(worker), vec![launched.to_string()]);
    let stored = worker.pane_grants.set().grants;
    assert_eq!(stored[0].rights, vec![Right::Write]);
    assert_eq!(stored[0].expires_at_ms, None);
    assert_eq!(
        stored[0].origin,
        GrantOrigin::Profile {
            profile: "claude".into()
        }
    );
    crate::worker::apply(
        worker,
        CoreFrame::Event(Box::new(hook_event(&claimed, "claude"))),
        true,
    );
    assert!(
        !holders(worker).contains(&claimed.to_string()),
        "a hook that names an agent is not proof the agent runs"
    );
    crate::worker::apply(
        worker,
        CoreFrame::Session(Box::new(observed(
            row(&later, 1, &workspace),
            CLAUDE_RUNTIME,
        ))),
        true,
    );
    let mut expected = vec![launched.to_string(), later.to_string()];
    expected.sort();
    assert_eq!(holders(worker), expected);
    snapshot(
        worker,
        vec![
            observed(row(&later, 1, &workspace), CLAUDE_RUNTIME),
            row(&plain, 1, &workspace),
        ],
    );
    assert_eq!(holders(worker), vec![later.to_string()]);
    running.stop();
}

#[test]
fn a_profile_naming_an_unknown_right_stores_no_grant() {
    let home = tempfile::tempdir().unwrap();
    let running = open(
        home.path(),
        r#"{"ipc": {}, "agents": {"claude": {"grants": [{"rights": ["write"]}, {"rights": ["root"]}]}}}"#,
    );
    let worker = running.worker();
    let (session, workspace) = (SessionId::new(), WorkspaceId::new());
    snapshot(
        worker,
        vec![observed(row(&session, 1, &workspace), CLAUDE_RUNTIME)],
    );
    assert!(worker.pane_grants.set().grants.is_empty());
    running.stop();
}

#[test]
fn a_pane_delegates_no_more_than_it_holds_and_only_to_a_pane_it_spawned() {
    let home = tempfile::tempdir().unwrap();
    let running = open(home.path(), CLOSED);
    let worker = running.worker();
    let workspace = WorkspaceId::new();
    let (parent, child, stranger) = (SessionId::new(), SessionId::new(), SessionId::new());
    snapshot(
        worker,
        vec![
            row(&parent, 1, &workspace),
            row(&child, 1, &workspace),
            row(&stranger, 1, &workspace),
        ],
    );
    let now = crate::state::now_ms();
    let expiry = now + 600_000;
    let (held, change) = worker.pane_grants.create(
        grant_for(
            &parent,
            &workspace,
            &[Right::Write, Right::Spawn],
            Some(expiry),
        ),
        now,
    );
    worker.publish_grant_change(change);
    worker
        .pane_grants
        .record_spawn(&parent.to_string(), &child.to_string());
    let caller = pane(&parent, &workspace);
    let delegated = call(
        worker,
        "grant.delegate",
        json!({"session": child, "rights": ["write"]}),
        &caller,
    )
    .unwrap();
    assert_eq!(delegated["granted"], true);
    assert_eq!(delegated["expires_at_ms"], json!(expiry));
    for (params, reason) in [
        (
            json!({"session": child, "rights": ["layout"]}),
            "grant_exceeds_holder",
        ),
        (
            json!({"session": child, "rights": ["write"], "scope": "instance"}),
            "grant_exceeds_holder",
        ),
        (
            json!({"session": child, "rights": ["write"], "duration_secs": 100_000}),
            "grant_exceeds_holder",
        ),
        (
            json!({"session": stranger, "rights": ["write"]}),
            "not_spawned_by_caller",
        ),
    ] {
        let refused = call(worker, "grant.delegate", params.clone(), &caller).unwrap_err();
        assert_eq!(refused["data"]["reason"], reason, "{params}: {refused}");
    }
    let child_caller = pane(&child, &workspace);
    let own = call(
        worker,
        "grant.revoke",
        json!({"id": held.id}),
        &child_caller,
    )
    .unwrap_err();
    assert_eq!(own["data"]["reason"], "principal_refused", "{own}");
    let listed = call(worker, "grant.list", json!({}), &child_caller).unwrap();
    assert_eq!(listed["grants"].as_array().unwrap().len(), 1, "{listed}");
    let before = worker
        .events
        .append("probe", Default::default(), Default::default());
    let revoked = call(
        worker,
        "grant.revoke",
        json!({"id": held.id}),
        &Principal::External,
    )
    .unwrap();
    let ids: Vec<&str> = revoked["revoked"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        vec![held.id.as_str(), delegated["id"].as_str().unwrap()]
    );
    let filter = crate::event_log::Filter::new(Some(&["grant.changed".to_string()]), None).unwrap();
    let changed: Vec<Value> = worker
        .events
        .follow(Some(before), None, filter)
        .unwrap()
        .replay
        .iter()
        .map(|logged| serde_json::from_str(&logged.line).unwrap())
        .collect();
    assert_eq!(changed.len(), 1, "{changed:?}");
    assert_eq!(
        changed[0]["data"]["revoked"], revoked["revoked"],
        "{changed:?}"
    );
    assert_eq!(
        changed[0]["data"]["revision"], revoked["revision"],
        "{changed:?}"
    );
    assert!(worker.pane_grants.set().grants.is_empty());
    running.stop();
}

#[test]
fn a_pane_revokes_what_it_delegated() {
    let home = tempfile::tempdir().unwrap();
    let running = open(home.path(), CLOSED);
    let worker = running.worker();
    let workspace = WorkspaceId::new();
    let (parent, child) = (SessionId::new(), SessionId::new());
    snapshot(
        worker,
        vec![row(&parent, 1, &workspace), row(&child, 1, &workspace)],
    );
    let (held, _) = worker.pane_grants.create(
        grant_for(&parent, &workspace, &[Right::Write], None),
        crate::state::now_ms(),
    );
    worker
        .pane_grants
        .record_spawn(&parent.to_string(), &child.to_string());
    let caller = pane(&parent, &workspace);
    let delegated = call(
        worker,
        "grant.delegate",
        json!({"session": child, "rights": ["write"], "duration_secs": 60}),
        &caller,
    )
    .unwrap();
    let own = call(worker, "grant.revoke", json!({"id": held.id}), &caller).unwrap_err();
    assert_eq!(own["data"]["reason"], "principal_refused", "{own}");
    let revoked = call(
        worker,
        "grant.revoke",
        json!({"id": delegated["id"]}),
        &caller,
    )
    .unwrap();
    assert_eq!(revoked["revoked"], json!([delegated["id"]]));
    running.stop();
}

#[test]
fn a_pane_that_splits_through_the_worker_may_delegate_to_the_new_pane() {
    let home = tempfile::tempdir().unwrap();
    let running = open_accepting(home.path(), CLOSED);
    let worker = running.worker();
    let cwd = tempfile::tempdir().unwrap();
    call(
        worker,
        "workspace.up",
        json!({"panes": [{"cwd": cwd.path()}]}),
        &Principal::Window,
    )
    .unwrap();
    let workspace = WorkspaceId::new();
    let parent = SessionId::new();
    snapshot(worker, vec![row(&parent, 1, &workspace)]);
    let caller = pane(&parent, &workspace);
    let split = call(
        worker,
        "surface.split",
        json!({"direction": "vertical", "cwd": cwd.path()}),
        &caller,
    )
    .unwrap();
    let surface = paneflow_layout::SurfaceId(split["surface_id"].as_u64().unwrap());
    let child = worker
        .layout
        .current_state()
        .surface(surface)
        .and_then(|surface| surface.definition.session.clone())
        .expect("the split surface names its session");
    assert_eq!(
        worker.pane_grants.spawned_by(child.as_str()).as_deref(),
        Some(parent.to_string().as_str())
    );
    snapshot(
        worker,
        vec![row(&parent, 1, &workspace), row(&child, 1, &workspace)],
    );
    worker.pane_grants.create(
        grant_for(&parent, &workspace, &[Right::Write], None),
        crate::state::now_ms(),
    );
    let delegated = call(
        worker,
        "grant.delegate",
        json!({"session": child, "rights": ["write"]}),
        &caller,
    )
    .unwrap();
    assert_eq!(delegated["granted"], true, "{delegated}");
    running.stop();
}

fn first_surface(state: &paneflow_layout::State, index: usize) -> (u64, SessionId) {
    let surface = state.workspaces[index]
        .tabs
        .iter()
        .filter_map(|tab| tab.root.as_ref())
        .flat_map(|root| root.panes())
        .flat_map(|pane| pane.surfaces.iter())
        .next()
        .expect("the workspace holds a surface");
    (
        surface.id.get(),
        surface
            .session()
            .cloned()
            .expect("the surface names a session"),
    )
}

fn split_beside(worker: &Arc<Worker>, anchor: u64, child: &SessionId) -> Value {
    let state = worker.layout.current_state();
    let target = state
        .find_surface(paneflow_layout::SurfaceId(anchor))
        .expect("the anchor is in the layout")
        .pane;
    let op = paneflow_layout::Op::Split {
        target,
        direction: paneflow_layout::Direction::Vertical,
        edge: paneflow_layout::Edge::After,
        pane: paneflow_layout::NewPane {
            id: paneflow_layout::PaneId(worker.layout.reserve_id()),
            surfaces: vec![paneflow_layout::NewSurface {
                id: paneflow_layout::SurfaceId(worker.layout.reserve_id()),
                definition: paneflow_config::schema::SurfaceDefinition {
                    surface_type: Some("terminal".to_string()),
                    session: Some(child.clone()),
                    ..paneflow_config::schema::SurfaceDefinition::default()
                },
            }],
            active: 0,
        },
        focus: false,
    };
    serde_json::to_value(op).unwrap()
}

#[test]
fn a_split_the_window_makes_for_a_pane_is_recorded_as_spawned_by_that_pane_only_from_the_window() {
    let home = tempfile::tempdir().unwrap();
    let running = open_accepting(home.path(), CLOSED);
    let worker = running.worker();
    let cwd = tempfile::tempdir().unwrap();
    call(
        worker,
        "workspace.up",
        json!({"panes": [{"cwd": cwd.path()}]}),
        &Principal::Window,
    )
    .unwrap();
    let state = worker.layout.current_state();
    let (anchor, parent) = first_surface(&state, state.workspaces.len() - 1);
    let workspace = WorkspaceId::new();

    let forged = SessionId::new();
    for caller in [Principal::External, pane(&parent, &workspace)] {
        let refused = call(
            worker,
            "layout.apply",
            json!({"op": split_beside(worker, anchor, &forged), "spawned_by": parent}),
            &caller,
        );
        assert!(
            refused.is_err(),
            "{caller:?} named the spawner: {refused:?}"
        );
    }
    assert_eq!(worker.pane_grants.spawned_by(forged.as_str()), None);

    let child = SessionId::new();
    call(
        worker,
        "layout.apply",
        json!({"op": split_beside(worker, anchor, &child), "spawned_by": parent}),
        &Principal::Window,
    )
    .unwrap();
    assert_eq!(
        worker.pane_grants.spawned_by(child.as_str()).as_deref(),
        Some(parent.to_string().as_str())
    );
    running.stop();
}

#[test]
fn a_workspace_spawn_grant_launches_a_command_in_the_callers_workspace_only() {
    let home = tempfile::tempdir().unwrap();
    let running = open_accepting(home.path(), CLOSED);
    let worker = running.worker();
    let cwd = tempfile::tempdir().unwrap();
    for _ in 0..2 {
        call(
            worker,
            "workspace.up",
            json!({"panes": [{"cwd": cwd.path()}]}),
            &Principal::Window,
        )
        .unwrap();
    }
    let state = worker.layout.current_state();
    let count = state.workspaces.len();
    assert!(count >= 2, "{count} workspaces");
    let (own_surface, session) = first_surface(&state, count - 2);
    let (other_surface, _) = first_surface(&state, count - 1);
    let workspace = WorkspaceId::new();
    snapshot(worker, vec![row(&session, 1, &workspace)]);
    let caller = pane(&session, &workspace);
    let launching = |surface: u64| json!({"direction": "vertical", "surface_id": surface, "command": "true", "cwd": cwd.path()});
    let closed = call(worker, "surface.split", launching(own_surface), &caller).unwrap_err();
    assert_eq!(closed["code"], -32601, "{closed}");
    worker.pane_grants.create(
        grant_for(&session, &workspace, &[Right::Spawn], None),
        crate::state::now_ms(),
    );
    let outside = call(worker, "surface.split", launching(other_surface), &caller).unwrap_err();
    assert_eq!(outside["data"]["reason"], "scope_refused", "{outside}");
    assert_eq!(outside["code"], -32602, "{outside}");
    call(worker, "surface.split", launching(own_surface), &caller).unwrap();
    worker.pane_grants.create(
        NewGrant {
            scope: GrantScope::Instance,
            ..grant_for(&session, &workspace, &[Right::Spawn], None)
        },
        crate::state::now_ms(),
    );
    call(worker, "surface.split", launching(other_surface), &caller).unwrap();
    running.stop();
}

#[test]
fn a_workspace_grant_is_not_delegated_into_another_workspace_and_a_layout_grant_is_instance_wide() {
    let home = tempfile::tempdir().unwrap();
    let running = open(home.path(), CLOSED);
    let worker = running.worker();
    let (here, elsewhere) = (WorkspaceId::new(), WorkspaceId::new());
    let (parent, child) = (SessionId::new(), SessionId::new());
    snapshot(
        worker,
        vec![row(&parent, 1, &here), row(&child, 1, &elsewhere)],
    );
    worker.pane_grants.create(
        grant_for(&parent, &here, &[Right::Write], None),
        crate::state::now_ms(),
    );
    worker
        .pane_grants
        .record_spawn(&parent.to_string(), &child.to_string());
    let caller = pane(&parent, &here);
    let delegation = json!({"session": child, "rights": ["write"]});
    let refused = call(worker, "grant.delegate", delegation.clone(), &caller).unwrap_err();
    assert_eq!(
        refused["data"]["reason"], "grant_exceeds_holder",
        "{refused}"
    );
    worker.pane_grants.create(
        NewGrant {
            scope: GrantScope::Instance,
            ..grant_for(&parent, &here, &[Right::Write], None)
        },
        crate::state::now_ms(),
    );
    let delegated = call(worker, "grant.delegate", delegation, &caller).unwrap();
    assert_eq!(delegated["granted"], true, "{delegated}");
    let layout = call(
        worker,
        "grant.request",
        json!({"rights": ["layout"]}),
        &caller,
    )
    .unwrap_err();
    assert_eq!(layout["code"], -32602, "{layout}");
    assert!(
        layout["message"]
            .as_str()
            .unwrap()
            .contains("scope instance"),
        "{layout}"
    );
    running.stop();
}
