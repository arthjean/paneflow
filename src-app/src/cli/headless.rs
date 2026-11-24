use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use paneflow_config::schema::{PaneFlowConfig, SpaceId};
use paneflow_ipc_client::host_control::{HostControl, REQUEST_DEADLINE};
use serde_json::{Value, json};

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

#[derive(Debug, Clone, PartialEq)]
pub(super) struct LayoutSurface {
    pub(super) surface_id: u64,
    pub(super) session: Option<String>,
    pub(super) name: Option<String>,
    pub(super) workspace_index: usize,
    pub(super) workspace_id: u64,
    pub(super) workspace_title: String,
    pub(super) space: Option<SpaceId>,
    pub(super) tab_id: u64,
    pub(super) tab_title: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct WorkerLayout {
    pub(super) surfaces: Vec<LayoutSurface>,
    pub(super) active_workspace: usize,
    pub(super) active_pane_count: usize,
}

impl WorkerLayout {
    fn holds_session_surface(&self, surface_id: u64) -> bool {
        self.surfaces
            .iter()
            .any(|surface| surface.surface_id == surface_id && surface.session.is_some())
    }
}

pub(super) fn worker_layout_of(snapshot: &paneflow_layout::Snapshot) -> WorkerLayout {
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
                        space: ws.space.clone(),
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

pub(super) fn worker_layout(endpoint: &Path) -> Option<WorkerLayout> {
    let mut control = HostControl::open(endpoint, CLIENT_NAME, REQUEST_DEADLINE).ok()?;
    let view = control.request("layout.view", json!({})).ok()?;
    let snapshot: paneflow_layout::Snapshot = serde_json::from_value(view["view"].clone()).ok()?;
    Some(worker_layout_of(&snapshot))
}

pub(super) fn current_worker_layout() -> Option<WorkerLayout> {
    worker_layout(&paneflow_home::serve_endpoint_path_for_current_home()?)
}

pub(super) fn requested_layout_surface(params: &Value, layout: &WorkerLayout) -> Option<u64> {
    params
        .get("surface_id")
        .and_then(Value::as_u64)
        .filter(|surface_id| layout.holds_session_surface(*surface_id))
}

pub(super) fn name_sessions(params: Value, layout: &WorkerLayout) -> Value {
    let Some(surface_id) = params.get("surface_id").and_then(Value::as_u64) else {
        return params;
    };
    let Some(session) = layout
        .surfaces
        .iter()
        .find(|surface| surface.surface_id == surface_id)
        .and_then(|surface| surface.session.clone())
    else {
        return params;
    };
    let mut params = params;
    if let Some(object) = params.as_object_mut() {
        object.remove("surface_id");
        object.insert("session".to_string(), json!(session));
    }
    params
}

pub(super) fn with_requested_surface(mut result: Value, requested: Option<u64>) -> Value {
    if let Some(surface_id) = requested
        && let Some(object) = result.as_object_mut()
        && object.contains_key("surface_id")
    {
        object.insert("surface_id".to_string(), json!(surface_id));
    }
    result
}

pub(super) fn with_layout_ids(
    listed: Value,
    layout: &WorkerLayout,
    config: &PaneFlowConfig,
) -> Value {
    let by_session: HashMap<&str, &Value> = listed["surfaces"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| row["session"].as_str().map(|session| (session, row)))
        .collect();
    let surfaces: Vec<Value> = layout
        .surfaces
        .iter()
        .map(|surface| {
            let host = surface
                .session
                .as_deref()
                .and_then(|session| by_session.get(session).copied());
            let mut row = host.cloned().unwrap_or_else(|| {
                json!({
                    "session": surface.session,
                    "cmd": Value::Null,
                    "live": false,
                    "lifecycle": if surface.session.is_some() { "unknown" } else { "unstarted" },
                })
            });
            row["surface_id"] = json!(surface.surface_id);
            row["workspace_id"] = json!(surface.workspace_id);
            row["workspace"] = json!(surface.workspace_index);
            row["workspace_title"] = json!(surface.workspace_title);
            row["space"] = crate::app::spaces::space_json(config, surface.space.as_ref());
            row["tab_id"] = json!(surface.tab_id);
            row["tab_title"] = json!(surface.tab_title);
            row["scope"] = json!("layout");
            if let Some(name) = &surface.name {
                row["name"] = json!(name);
            }
            row
        })
        .collect();
    let named: HashSet<&str> = layout
        .surfaces
        .iter()
        .filter_map(|surface| surface.session.as_deref())
        .collect();
    let mut surfaces = surfaces;
    surfaces.extend(
        listed["surfaces"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|row| {
                row["session"]
                    .as_str()
                    .is_none_or(|session| !named.contains(session))
            })
            .cloned(),
    );
    let mut result = listed;
    result["pane_count"] = json!(layout.active_pane_count);
    result["workspace"] = json!(layout.active_workspace);
    if result.get("scope_workspace_id").is_none() {
        result["scope_workspace_id"] = Value::Null;
    }
    result["surfaces"] = json!(surfaces);
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
        other.space = Some(work.clone());
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

    #[test]
    fn a_layout_surface_id_is_sent_to_the_host_as_its_session_and_comes_back() {
        let params = name_sessions(json!({"surface_id": 41, "lines": 10}), &layout());
        assert_eq!(params, json!({"session": SERVER, "lines": 10}));
        let unknown = name_sessions(json!({"surface_id": 7}), &layout());
        assert_eq!(unknown, json!({"surface_id": 7}), "an alias stays an alias");

        let requested = requested_layout_surface(&json!({"surface_id": 41}), &layout());
        assert_eq!(requested, Some(41));
        assert_eq!(
            requested_layout_surface(&json!({"surface_id": 42}), &layout()),
            None,
            "a surface without a session is never sent as one"
        );
        let status = with_requested_surface(json!({"surface_id": 0, "hooked": false}), requested);
        assert_eq!(status["surface_id"], 41);
        let sent = with_requested_surface(json!({"sent": true}), requested);
        assert!(sent.get("surface_id").is_none());
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
