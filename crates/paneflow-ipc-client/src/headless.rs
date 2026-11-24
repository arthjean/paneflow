use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{json, Value};

use crate::host_control::{HostControl, REQUEST_DEADLINE};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutSurface {
    pub surface_id: u64,
    pub session: Option<String>,
    pub name: Option<String>,
    pub workspace_index: usize,
    pub workspace_id: u64,
    pub workspace_title: String,
    pub space: Option<String>,
    pub tab_id: u64,
    pub tab_title: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkerLayout {
    pub surfaces: Vec<LayoutSurface>,
    pub active_workspace: usize,
    pub active_pane_count: usize,
}

impl WorkerLayout {
    pub fn holds_session_surface(&self, surface_id: u64) -> bool {
        self.surfaces
            .iter()
            .any(|surface| surface.surface_id == surface_id && surface.session.is_some())
    }

    pub fn listed_sessions_only(&self, listed: &Value) -> Self {
        let listed: HashSet<&str> = array(&listed["surfaces"])
            .iter()
            .filter_map(|row| row["session"].as_str())
            .collect();
        Self {
            surfaces: self
                .surfaces
                .iter()
                .filter(|surface| {
                    surface
                        .session
                        .as_deref()
                        .is_some_and(|session| listed.contains(session))
                })
                .cloned()
                .collect(),
            ..self.clone()
        }
    }
}

pub fn worker_layout_of(view: &Value) -> WorkerLayout {
    let state = &view["state"];
    let active_workspace = state["active_workspace"]
        .as_u64()
        .and_then(|index| usize::try_from(index).ok())
        .unwrap_or_default();
    let mut surfaces = Vec::new();
    let mut active_pane_count = 0;
    for (workspace_index, workspace) in array(&state["workspaces"]).iter().enumerate() {
        let Some(workspace_id) = workspace["id"].as_u64() else {
            continue;
        };
        let workspace_title = text(&workspace["title"]);
        let space = workspace["space"].as_str().map(str::to_owned);
        for tab in array(&workspace["tabs"]) {
            let Some(tab_id) = tab["id"].as_u64() else {
                continue;
            };
            let mut panes = Vec::new();
            collect_panes(&tab["root"], &mut panes);
            if workspace_index == active_workspace {
                active_pane_count += panes.len();
            }
            for surface in panes.into_iter().flat_map(|pane| array(&pane["surfaces"])) {
                let Some(surface_id) = surface["id"].as_u64() else {
                    continue;
                };
                let definition = &surface["definition"];
                let terminal =
                    matches!(definition["surface_type"].as_str(), None | Some("terminal"));
                surfaces.push(LayoutSurface {
                    surface_id,
                    session: definition["session"]
                        .as_str()
                        .filter(|_| terminal)
                        .map(str::to_owned),
                    name: definition["custom_name"]
                        .as_str()
                        .or_else(|| definition["name"].as_str())
                        .map(str::to_owned),
                    workspace_index,
                    workspace_id,
                    workspace_title: workspace_title.clone(),
                    space: space.clone(),
                    tab_id,
                    tab_title: text(&tab["title"]),
                });
            }
        }
    }
    WorkerLayout {
        surfaces,
        active_workspace,
        active_pane_count,
    }
}

fn collect_panes<'a>(node: &'a Value, out: &mut Vec<&'a Value>) {
    match node["kind"].as_str() {
        Some("pane") => out.push(node),
        Some("split") => {
            for child in array(&node["children"]) {
                collect_panes(&child["node"], out);
            }
        }
        _ => {}
    }
}

fn array(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_owned()
}

pub fn worker_layout(endpoint: &Path, client: &str) -> Option<WorkerLayout> {
    let mut control = HostControl::open(endpoint, client, REQUEST_DEADLINE).ok()?;
    let view = control.request("layout.view", json!({})).ok()?;
    view.get("view")
        .filter(|view| view.get("state").is_some())
        .map(worker_layout_of)
}

pub fn requested_layout_surface(params: &Value, layout: &WorkerLayout) -> Option<u64> {
    params
        .get("surface_id")
        .and_then(Value::as_u64)
        .filter(|surface_id| layout.holds_session_surface(*surface_id))
}

pub fn name_sessions(params: Value, layout: &WorkerLayout) -> Value {
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

pub fn with_requested_surface(mut result: Value, requested: Option<u64>) -> Value {
    if let Some(surface_id) = requested {
        if let Some(object) = result.as_object_mut() {
            if object.contains_key("surface_id") {
                object.insert("surface_id".to_string(), json!(surface_id));
            }
        }
    }
    result
}

pub fn with_layout_ids(
    listed: Value,
    layout: &WorkerLayout,
    decorate: impl Fn(&LayoutSurface, &mut Value),
) -> Value {
    let by_session: HashMap<&str, &Value> = array(&listed["surfaces"])
        .iter()
        .filter_map(|row| row["session"].as_str().map(|session| (session, row)))
        .collect();
    let mut surfaces: Vec<Value> = layout
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
            row["workspace_title"] = json!(surface.workspace_title);
            row["tab_id"] = json!(surface.tab_id);
            row["tab_title"] = json!(surface.tab_title);
            row["scope"] = json!("layout");
            if let Some(name) = &surface.name {
                row["name"] = json!(name);
            }
            decorate(surface, &mut row);
            row
        })
        .collect();
    let named: HashSet<&str> = layout
        .surfaces
        .iter()
        .filter_map(|surface| surface.session.as_deref())
        .collect();
    surfaces.extend(
        array(&listed["surfaces"])
            .iter()
            .filter(|row| {
                row["session"]
                    .as_str()
                    .is_none_or(|session| !named.contains(session))
            })
            .cloned(),
    );
    let mut result = listed;
    result["pane_count"] = json!(surfaces.len());
    result["surfaces"] = json!(surfaces);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVER: &str = "0190e4d2-7b2a-7c3d-8e4f-a1b2c3d4e5f6";

    fn layout() -> WorkerLayout {
        let surface = |surface_id, session: Option<&str>, name: Option<&str>| LayoutSurface {
            surface_id,
            session: session.map(str::to_owned),
            name: name.map(str::to_owned),
            workspace_index: 0,
            workspace_id: 3,
            workspace_title: "api".into(),
            space: None,
            tab_id: 4,
            tab_title: String::new(),
        };
        WorkerLayout {
            surfaces: vec![
                surface(41, Some(SERVER), Some("server")),
                surface(42, None, None),
            ],
            active_workspace: 0,
            active_pane_count: 2,
        }
    }

    #[test]
    fn the_host_rows_take_the_layout_ids_and_names() {
        let listed = json!({"pane_count": 2, "workspace": null, "surfaces": [
            {"surface_id": 1, "session": SERVER, "live": true, "name": "sh", "scope": "host", "workspace": "bdf6"},
            {"surface_id": 2, "session": "0190e4d2-7b2a-7c3d-8e4f-a1b2c3d4e5f7", "live": true, "name": "agent", "scope": "host"}
        ]});
        let merged = with_layout_ids(listed, &layout(), |_, _| {});
        assert_eq!(merged["pane_count"], 3);
        let rows = merged["surfaces"].as_array().unwrap();
        assert_eq!(rows[0]["surface_id"], 41);
        assert_eq!(rows[0]["name"], "server");
        assert_eq!(rows[0]["live"], true);
        assert_eq!(rows[0]["workspace_id"], 3);
        assert_eq!(
            rows[0]["workspace"], "bdf6",
            "an undecorated row keeps the host workspace"
        );
        assert_eq!(rows[1]["surface_id"], 42);
        assert_eq!(rows[1]["lifecycle"], "unstarted");
        assert_eq!(
            rows[2]["surface_id"], 2,
            "a session outside the layout keeps its host alias"
        );
        assert_eq!(rows[2]["scope"], "host");

        let decorated = with_layout_ids(json!({"surfaces": []}), &layout(), |surface, row| {
            row["workspace"] = json!(surface.workspace_index);
        });
        assert_eq!(decorated["surfaces"][0]["workspace"], 0);
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
    fn only_the_layout_surfaces_the_host_listed_are_kept() {
        let listed = json!({"surfaces": [{"surface_id": 1, "session": SERVER}]});
        let kept = layout().listed_sessions_only(&listed);
        assert_eq!(kept.surfaces.len(), 1);
        assert_eq!(kept.surfaces[0].surface_id, 41);
        assert_eq!(kept.active_pane_count, 2);
    }

    #[test]
    fn the_layout_walk_reads_split_trees_and_skips_markdown_sessions() {
        let pane =
            |id: u64, surfaces: Value| json!({"kind": "pane", "id": id, "surfaces": surfaces});
        let view = json!({"state": {"active_workspace": 1, "workspaces": [
            {"id": 2, "title": "other", "tabs": [{"id": 7, "title": "", "root": pane(8, json!([]))}]},
            {
                "id": 6,
                "title": "probe",
                "space": "550e8400-e29b-41d4-a716-446655440001",
                "tabs": [{
                    "id": 5,
                    "title": "main",
                    "root": {"kind": "split", "id": 8, "children": [
                        {"node": pane(3, json!([
                            {"id": 1, "definition": {"surface_type": "terminal", "name": "shell", "custom_name": "tests", "session": "s-1"}},
                            {"id": 9, "definition": {"surface_type": "markdown", "name": "notes", "session": "s-9"}}
                        ]))},
                        {"node": {"kind": "split", "id": 10, "children": [
                            {"node": pane(4, json!([{"id": 2, "definition": {"name": "caller", "session": "s-2"}}]))}
                        ]}}
                    ]}
                }]
            }
        ]}});
        let layout = worker_layout_of(&view);
        assert_eq!(layout.active_workspace, 1);
        assert_eq!(layout.active_pane_count, 2);
        let ids: Vec<u64> = layout
            .surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .collect();
        assert_eq!(ids, [1, 9, 2]);
        let found = &layout.surfaces;
        assert_eq!(found[0].name.as_deref(), Some("tests"));
        assert_eq!(found[0].session.as_deref(), Some("s-1"));
        assert_eq!(found[1].session, None);
        assert_eq!(found[2].name.as_deref(), Some("caller"));
        assert_eq!(found[2].workspace_index, 1);
        assert_eq!(found[2].workspace_title, "probe");
        assert_eq!(
            found[2].space.as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440001")
        );
        assert_eq!(found[2].tab_title, "main");
    }
}
