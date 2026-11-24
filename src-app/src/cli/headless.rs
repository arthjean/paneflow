use std::collections::{HashMap, HashSet};
use std::path::Path;

use paneflow_ipc_client::host_control::{HostControl, REQUEST_DEADLINE};
use serde_json::{Value, json};

use super::CLIENT_NAME;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct LayoutSurface {
    pub(super) surface_id: u64,
    pub(super) session: Option<String>,
    pub(super) name: Option<String>,
    pub(super) workspace_id: u64,
    pub(super) workspace_title: String,
    pub(super) tab_id: u64,
    pub(super) tab_title: String,
}

pub(super) fn layout_surfaces(snapshot: &paneflow_layout::Snapshot) -> Vec<LayoutSurface> {
    let mut found = Vec::new();
    for ws in &snapshot.state.workspaces {
        for tab in &ws.tabs {
            let Some(root) = tab.root.as_ref() else {
                continue;
            };
            for pane in root.panes() {
                for surface in &pane.surfaces {
                    found.push(LayoutSurface {
                        surface_id: surface.id.get(),
                        session: surface.session().map(ToString::to_string),
                        name: surface.display_name().map(str::to_owned),
                        workspace_id: ws.id.get(),
                        workspace_title: ws.title.clone(),
                        tab_id: tab.id.get(),
                        tab_title: tab.title.clone(),
                    });
                }
            }
        }
    }
    found
}

pub(super) fn worker_layout(endpoint: &Path) -> Option<Vec<LayoutSurface>> {
    let mut control = HostControl::open(endpoint, CLIENT_NAME, REQUEST_DEADLINE).ok()?;
    let view = control.request("layout.view", json!({})).ok()?;
    let snapshot: paneflow_layout::Snapshot = serde_json::from_value(view["view"].clone()).ok()?;
    Some(layout_surfaces(&snapshot))
}

pub(super) fn current_worker_layout() -> Option<Vec<LayoutSurface>> {
    worker_layout(&paneflow_home::serve_endpoint_path_for_current_home()?)
}

pub(super) fn name_sessions(params: Value, layout: &[LayoutSurface]) -> Value {
    let Some(surface_id) = params.get("surface_id").and_then(Value::as_u64) else {
        return params;
    };
    let Some(session) = layout
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

pub(super) fn with_layout_ids(listed: Value, layout: &[LayoutSurface]) -> Value {
    let by_session: HashMap<&str, &Value> = listed["surfaces"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| row["session"].as_str().map(|session| (session, row)))
        .collect();
    let surfaces: Vec<Value> = layout
        .iter()
        .map(|surface| {
            let host = surface
                .session
                .as_deref()
                .and_then(|session| by_session.get(session).copied());
            let mut row = host.cloned().unwrap_or_else(|| {
                json!({
                    "session": surface.session,
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
            row
        })
        .collect();
    let named: HashSet<&str> = layout
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
    result["pane_count"] = json!(surfaces.len());
    result["surfaces"] = json!(surfaces);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> Vec<LayoutSurface> {
        vec![
            LayoutSurface {
                surface_id: 41,
                session: Some("0190e4d2-7b2a-7c3d-8e4f-a1b2c3d4e5f6".into()),
                name: Some("server".into()),
                workspace_id: 3,
                workspace_title: "api".into(),
                tab_id: 4,
                tab_title: String::new(),
            },
            LayoutSurface {
                surface_id: 42,
                session: None,
                name: None,
                workspace_id: 3,
                workspace_title: "api".into(),
                tab_id: 4,
                tab_title: String::new(),
            },
        ]
    }

    #[test]
    fn the_host_rows_take_the_layout_ids_and_names() {
        let listed = json!({"pane_count": 2, "workspace": null, "surfaces": [
            {"surface_id": 1, "session": "0190e4d2-7b2a-7c3d-8e4f-a1b2c3d4e5f6", "live": true, "name": "sh", "scope": "host"},
            {"surface_id": 2, "session": "0190e4d2-7b2a-7c3d-8e4f-a1b2c3d4e5f7", "live": true, "name": "agent", "scope": "host"}
        ]});
        let merged = with_layout_ids(listed, &layout());
        assert_eq!(merged["pane_count"], 3);
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
    fn a_layout_surface_id_is_sent_to_the_host_as_its_session() {
        let params = name_sessions(json!({"surface_id": 41, "lines": 10}), &layout());
        assert_eq!(
            params,
            json!({"session": "0190e4d2-7b2a-7c3d-8e4f-a1b2c3d4e5f6", "lines": 10})
        );
        let unknown = name_sessions(json!({"surface_id": 7}), &layout());
        assert_eq!(unknown, json!({"surface_id": 7}), "an alias stays an alias");
    }
}
