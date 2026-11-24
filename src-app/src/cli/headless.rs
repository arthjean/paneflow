use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

use paneflow_config::schema::{PaneFlowConfig, SpaceId};
use serde_json::{Value, json};

pub(super) use paneflow_ipc_client::headless::{
    LayoutSurface, WorkerLayout, name_sessions, requested_layout_surface, with_requested_surface,
    worker_layout_of,
};

use super::CLIENT_NAME;
use crate::agent_launcher::TerminalAgent;
use crate::ai_types::AgentSession;
use crate::app::host_agents::{
    host_observed_agent, row_from_snapshot, seed_projected_session, waiting_since_instant,
};
use crate::app::ipc_handler::{WsFleet, build_fleet_rows, surface_status_value};

const WINDOW_STATUS_FIELDS: [&str; 8] = [
    "state",
    "state_seq",
    "tool",
    "active_tool_name",
    "message",
    "last_result",
    "waiting_ms",
    "idle_ms",
];

pub(super) fn current_worker_layout() -> Option<WorkerLayout> {
    paneflow_ipc_client::headless::worker_layout(
        &paneflow_home::serve_endpoint_path_for_current_home()?,
        CLIENT_NAME,
    )
}

fn space_of(surface: &LayoutSurface) -> Option<SpaceId> {
    surface
        .space
        .as_deref()
        .and_then(|raw| SpaceId::parse(raw).ok())
}

pub(super) fn with_layout_ids(
    listed: Value,
    layout: &WorkerLayout,
    config: &PaneFlowConfig,
) -> Value {
    let mut result =
        paneflow_ipc_client::headless::with_layout_ids(listed, layout, |surface, row| {
            row["workspace"] = json!(surface.workspace_index);
            row["space"] = crate::app::spaces::space_json(config, space_of(surface).as_ref());
        });
    result["pane_count"] = json!(layout.active_pane_count);
    result["workspace"] = json!(layout.active_workspace);
    if result.get("scope_workspace_id").is_none() {
        result["scope_workspace_id"] = Value::Null;
    }
    result
}

fn projection_of<'a>(row: &Value, projections: &'a [Value]) -> Option<&'a Value> {
    let session = row.get("session").and_then(Value::as_str)?;
    projections.iter().find(|projection| {
        projection.get("session").and_then(Value::as_str) == Some(session)
            && row
                .get("generation")
                .is_none_or(|generation| projection.get("generation") == Some(generation))
    })
}

fn seed_window_session(
    sessions: &mut HashMap<u32, AgentSession>,
    projection: &Value,
    surface_id: u64,
) -> Option<TerminalAgent> {
    let row = row_from_snapshot(projection)?;
    seed_projected_session(sessions, &row, surface_id, |_| None);
    let updated_at_ms = projection
        .get("activity")
        .and_then(|activity| activity.get("updated_at_ms"))
        .and_then(Value::as_u64);
    if let Some(session) = sessions
        .values_mut()
        .find(|session| session.surface_id == Some(surface_id))
        && updated_at_ms.is_some()
    {
        session.last_activity = waiting_since_instant(updated_at_ms);
    }
    host_observed_agent(Some(&row))
}

pub(super) fn agent_status(mut status: Value, projections: &[Value]) -> Value {
    if status.get("error").is_some() {
        return status;
    }
    let Some(surface_id) = status.get("surface_id").and_then(Value::as_u64) else {
        return status;
    };
    let Some(projection) = projection_of(&status, projections) else {
        return status;
    };
    let mut sessions = HashMap::new();
    seed_window_session(&mut sessions, projection, surface_id);
    let window = surface_status_value(
        surface_id,
        sessions.values().next(),
        0,
        Instant::now(),
        projection.get("state_seq").and_then(Value::as_u64),
    );
    if let Some(object) = status.as_object_mut() {
        for field in WINDOW_STATUS_FIELDS {
            match window.get(field) {
                Some(value) => object.insert(field.to_string(), value.clone()),
                None => object.remove(field),
            };
        }
    }
    status
}

pub(super) fn surface_names(listed: &Value) -> HashMap<u64, String> {
    listed["surfaces"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            Some((
                row["surface_id"].as_u64()?,
                row["name"].as_str()?.to_string(),
            ))
        })
        .collect()
}

pub(super) fn fleet(
    layout: &WorkerLayout,
    names: &HashMap<u64, String>,
    projections: &[Value],
) -> Value {
    let mut workspaces: BTreeMap<usize, (HashMap<u32, AgentSession>, HashSet<String>)> =
        BTreeMap::new();
    for surface in &layout.surfaces {
        let Some(projection) = surface.session.as_deref().and_then(|session| {
            projections
                .iter()
                .find(|projection| projection["session"].as_str() == Some(session))
        }) else {
            continue;
        };
        let (sessions, detected) = workspaces.entry(surface.workspace_index).or_default();
        if let Some(agent) = seed_window_session(sessions, projection, surface.surface_id) {
            detected.insert(agent.binary().to_string());
        }
    }
    let fleets: Vec<WsFleet> = workspaces
        .iter()
        .map(|(idx, (sessions, detected))| WsFleet {
            idx: *idx,
            sessions,
            detected,
        })
        .collect();
    json!({ "agents": build_fleet_rows(&fleets, names, Instant::now()) })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVER: &str = "0190e4d2-7b2a-7c3d-8e4f-a1b2c3d4e5f6";
    const AGENT: &str = "0190e4d2-7b2a-7c3d-8e4f-a1b2c3d4e5f8";

    fn surface(surface_id: u64, session: Option<&str>, workspace_index: usize) -> LayoutSurface {
        LayoutSurface {
            surface_id,
            session: session.map(str::to_string),
            name: None,
            workspace_index,
            workspace_id: 3 + workspace_index as u64,
            workspace_title: "api".into(),
            space: None,
            tab_id: 4,
            tab_title: String::new(),
        }
    }

    fn layout() -> WorkerLayout {
        let mut server = surface(41, Some(SERVER), 0);
        server.name = Some("server".into());
        WorkerLayout {
            surfaces: vec![server, surface(42, None, 0)],
            active_workspace: 0,
            active_pane_count: 2,
        }
    }

    fn declaring_amp(session: &str, pid: u32) -> Value {
        json!({
            "session": session,
            "generation": 1,
            "live": true,
            "state_seq": 7,
            "activity_source": "declared",
            "runtime_id": "com.sourcegraph.amp",
            "foreground_runtime_id": "com.sourcegraph.amp",
            "activity": {"tool": "amp", "state": "finished", "source": "hook", "pid": pid, "updated_at_ms": 1},
        })
    }

    #[test]
    fn the_host_rows_take_the_layout_ids_and_names() {
        let listed = json!({"pane_count": 2, "workspace": null, "surfaces": [
            {"surface_id": 1, "session": SERVER, "live": true, "name": "sh", "scope": "host"},
            {"surface_id": 2, "session": "0190e4d2-7b2a-7c3d-8e4f-a1b2c3d4e5f7", "live": true, "name": "agent", "scope": "host"}
        ]});
        let merged = with_layout_ids(listed, &layout(), &PaneFlowConfig::default());
        let outside = &merged["surfaces"][2];
        assert_eq!(
            outside["surface_id"], 2,
            "a session outside the layout keeps its host alias"
        );
        assert_eq!(outside["name"], "agent");
        assert_eq!(outside["scope"], "host");
        let rows = merged["surfaces"].as_array().unwrap();
        assert_eq!(rows[0]["surface_id"], 41);
        assert_eq!(rows[0]["name"], "server");
        assert_eq!(rows[0]["live"], true);
        assert_eq!(rows[0]["workspace_id"], 3);
        assert_eq!(rows[1]["surface_id"], 42);
        assert_eq!(rows[1]["lifecycle"], "unstarted");
    }

    #[test]
    fn a_windowless_listing_names_the_workspace_index_and_space_like_the_window() {
        let work = SpaceId::new();
        let config: PaneFlowConfig = serde_json::from_value(json!({
            "spaces": [{"id": work, "name": "Work"}]
        }))
        .expect("config");
        let mut layout = layout();
        let mut other = surface(43, Some(AGENT), 1);
        other.space = Some(work.to_string());
        layout.surfaces.push(other);
        let listed = json!({"pane_count": 9, "workspace": null, "surfaces": [
            {"surface_id": 1, "session": SERVER, "workspace": "bdf606c3-64dc-49de-bcde-f75afe5f578a", "cmd": null, "scope": "host"},
            {"surface_id": 2, "session": AGENT, "workspace": "9b4a978a-1dcc-46b4-8117-5cd37e60329e", "cmd": "amp", "scope": "host"}
        ]});

        let merged = with_layout_ids(listed, &layout, &config);

        assert_eq!(merged["workspace"], 0, "the active workspace index");
        assert_eq!(
            merged["pane_count"], 2,
            "the panes of the active workspace, as the window counts them"
        );
        let rows = merged["surfaces"].as_array().unwrap();
        assert_eq!(rows[0]["workspace"], 0);
        assert_eq!(rows[0]["space"], Value::Null, "the default space");
        assert_eq!(rows[2]["workspace"], 1);
        assert_eq!(rows[2]["space"], json!({"id": work, "name": "Work"}));
        assert_eq!(rows[2]["cmd"], "amp");
    }

    fn typed_walk(snapshot: &paneflow_layout::Snapshot) -> WorkerLayout {
        let state = &snapshot.state;
        let mut surfaces = Vec::new();
        for (workspace_index, ws) in state.workspaces.iter().enumerate() {
            for tab in &ws.tabs {
                let Some(root) = tab.root.as_ref() else {
                    continue;
                };
                for pane in root.panes() {
                    for surface in &pane.surfaces {
                        surfaces.push(LayoutSurface {
                            surface_id: surface.id.get(),
                            session: surface.session().map(ToString::to_string),
                            name: surface.display_name().map(str::to_owned),
                            workspace_index,
                            workspace_id: ws.id.get(),
                            workspace_title: ws.title.clone(),
                            space: ws.space.as_ref().map(ToString::to_string),
                            tab_id: tab.id.get(),
                            tab_title: tab.title.clone(),
                        });
                    }
                }
            }
        }
        let active_pane_count = state
            .workspaces
            .get(state.active_workspace)
            .map_or(0, |ws| {
                ws.tabs
                    .iter()
                    .filter_map(|tab| tab.root.as_ref())
                    .map(|root| root.panes().len())
                    .sum()
            });
        WorkerLayout {
            surfaces,
            active_workspace: state.active_workspace,
            active_pane_count,
        }
    }

    fn layout_surface(
        id: u64,
        kind: &str,
        custom: Option<&str>,
        name: Option<&str>,
        session: &str,
    ) -> Value {
        json!({"id": id, "definition": {
            "surface_type": kind,
            "name": name,
            "custom_name": custom,
            "command": null,
            "cwd": "/tmp",
            "env": null,
            "focus": null,
            "session": session
        }})
    }

    fn pane(id: u64, surfaces: Vec<Value>) -> Value {
        json!({"kind": "pane", "id": id, "surfaces": surfaces, "active": 0, "detached": false})
    }

    fn workspace(id: u64, title: &str, space: Option<&SpaceId>, root: Value) -> Value {
        json!({
            "id": id,
            "title": title,
            "cwd": "/tmp",
            "space": space,
            "tabs": [{
                "id": id + 100,
                "title": "main",
                "title_source": "preset",
                "root": root,
                "focused": null,
                "zoomed": null,
                "worktree": null,
                "extras": {"worktree_raw": null, "unread": false, "pull_request": null, "dock": null}
            }],
            "active_tab": 0,
            "extras": {
                "durable_id": "9a344e15-99c2-4749-9699-de8c5b12bf29",
                "legacy_layout": null,
                "custom_buttons": [],
                "expanded_paths": [],
                "managed_worktrees": [],
                "sidebar_collapsed": false,
                "muted": false
            }
        })
    }

    #[test]
    fn the_shared_layout_walk_matches_the_typed_layout_model() {
        let work = SpaceId::new();
        let split = json!({"kind": "split", "id": 8, "direction": "vertical", "children": [
            {"node": pane(3, vec![
                layout_surface(1, "terminal", Some("tests"), None, SERVER),
                layout_surface(9, "markdown", None, Some("notes"), AGENT),
            ]), "ratio": 0.5},
            {"node": {"kind": "split", "id": 10, "direction": "horizontal", "children": [
                {"node": pane(4, vec![
                    layout_surface(2, "terminal", None, Some("caller"), "0aa99b53-e5c7-4587-ae7f-588cf7c4cb77"),
                ]), "ratio": 1.0}
            ]}, "ratio": 0.5}
        ]});
        let view = json!({
            "revision": 4,
            "state": {
                "version": 3,
                "workspaces": [
                    workspace(6, "probe", None, split),
                    workspace(20, "work", Some(&work), pane(21, vec![
                        layout_surface(22, "terminal", None, None, "0aa99b53-e5c7-4587-ae7f-588cf7c4cb70"),
                    ])),
                ],
                "active_workspace": 1,
                "active_space": null
            }
        });
        let snapshot: paneflow_layout::Snapshot =
            serde_json::from_value(view).expect("a layout.view snapshot");
        let wire = serde_json::to_value(&snapshot).expect("the snapshot serializes");
        let typed = typed_walk(&snapshot);
        assert_eq!(typed.surfaces.len(), 4, "{typed:?}");
        assert_eq!(typed.active_pane_count, 1);
        assert_eq!(worker_layout_of(&wire), typed);
    }

    #[test]
    fn a_windowless_status_carries_the_agent_fields_of_the_window() {
        let host = json!({
            "surface_id": 41,
            "session": SERVER,
            "generation": 1,
            "hooked": false,
            "output_generation": 12,
            "foreground_runtime": "com.sourcegraph.amp",
        });
        let status = agent_status(host, &[declaring_amp(SERVER, 77)]);
        assert_eq!(status["surface_id"], 41);
        assert_eq!(status["tool"], "amp");
        assert_eq!(status["state"], "finished");
        assert_eq!(status["state_seq"], 7);
        assert_eq!(status["hooked"], false);
        assert_eq!(status["output_generation"], 12);
        assert!(status["idle_ms"].is_u64());
        assert_eq!(status["waiting_ms"], Value::Null);
        assert_eq!(status["active_tool_name"], Value::Null);

        let shell = agent_status(
            json!({"surface_id": 41, "session": SERVER, "generation": 1, "hooked": false}),
            &[json!({"session": SERVER, "generation": 1, "state_seq": 3, "activity": null})],
        );
        assert_eq!(shell["state"], "idle");
        assert_eq!(shell["state_seq"], 3);
        assert!(shell.get("tool").is_none());
    }

    #[test]
    fn a_windowless_fleet_lists_every_agent_with_its_pid_workspace_index_and_surface() {
        let mut layout = layout();
        layout.surfaces.push(surface(43, Some(AGENT), 2));
        let names = HashMap::from([(41, "server".to_string()), (43, "reviewer".to_string())]);

        let fleet = fleet(
            &layout,
            &names,
            &[declaring_amp(SERVER, 86_790), declaring_amp(AGENT, 86_694)],
        );

        let agents = fleet["agents"].as_array().unwrap();
        assert_eq!(agents.len(), 2, "{fleet}");
        assert_eq!(agents[0]["surface_id"], 41);
        assert_eq!(agents[0]["surface_name"], "server");
        assert_eq!(agents[0]["workspace"], 0);
        assert_eq!(agents[0]["pid"], 86_790);
        assert_eq!(agents[0]["tool"], "amp");
        assert_eq!(agents[0]["hooked"], false);
        assert_eq!(agents[1]["surface_id"], 43);
        assert_eq!(agents[1]["workspace"], 2, "an index, never a durable id");
        assert_eq!(agents[1]["pid"], 86_694);
    }
}
