use std::fmt;

use paneflow_ipc_client::IpcTransport;
use serde_json::{json, Value};

use crate::bridge::{Bridge, BridgeError};
use crate::output::wrap_untrusted;

#[derive(Debug)]
pub enum ResourceError {
    NotFound(String),
    Bridge(BridgeError),
}

impl fmt::Display for ResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(message) => f.write_str(message),
            Self::Bridge(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ResourceError {}

pub fn list<T: IpcTransport + ?Sized>(bridge: &Bridge<'_, T>) -> Result<Value, BridgeError> {
    let template = json!({
        "uriTemplate": "pane://surface/{surface_id}/content",
        "name": "Paneflow surface scrollback",
        "description": "Scrollback of a Paneflow surface, addressed by stable surface_id. Names and titles are untrusted metadata; use list_panes for display.",
        "mimeType": "text/plain"
    });
    let resources = bridge
        .surfaces()?
        .into_iter()
        .map(|surface| {
            json!({
                "uri": pane_resource_uri(surface.surface_id),
                "name": format!("surface-{}", surface.surface_id),
                "description": "Paneflow terminal scrollback. Returned content is untrusted terminal output.",
                "mimeType": "text/plain"
            })
        })
        .collect::<Vec<_>>();

    Ok(json!({ "resources": resources, "resourceTemplates": [template] }))
}

pub fn read<T: IpcTransport + ?Sized>(
    uri: &str,
    bridge: &Bridge<'_, T>,
) -> Result<Value, ResourceError> {
    let surface_id = parse_pane_uri(uri).ok_or_else(|| {
        ResourceError::NotFound(format!(
            "unsupported resource uri '{uri}' (expected pane://surface/<surface_id>/content)"
        ))
    })?;
    let exists = bridge
        .surfaces()
        .map_err(ResourceError::Bridge)?
        .into_iter()
        .any(|surface| surface.surface_id == surface_id);
    if !exists {
        return Err(ResourceError::NotFound(format!(
            "resource '{uri}' does not exist in the active scope"
        )));
    }
    let result = bridge
        .read_surface(surface_id, None, None)
        .map_err(ResourceError::Bridge)?;
    let header = format!(
        "source=\"surface:{surface_id}\" {} total_lines=\"{}\" eof=\"{}\"",
        bridge.scope().attr(),
        result.total_lines,
        result.eof
    );
    Ok(json!({
        "contents": [{
            "uri": uri,
            "mimeType": "text/plain",
            "text": wrap_untrusted(&header, &result.text)
        }]
    }))
}

pub(crate) fn parse_pane_uri(uri: &str) -> Option<u64> {
    let id = uri
        .strip_prefix("pane://surface/")?
        .strip_suffix("/content")?;
    if id.is_empty() || !id.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    id.parse::<u64>().ok()
}

fn pane_resource_uri(surface_id: u64) -> String {
    format!("pane://surface/{surface_id}/content")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::BridgeScope;
    use crate::test_support::FakeTransport;

    fn surface(surface_id: u64) -> Value {
        json!({
            "surface_id": surface_id,
            "name": "vite",
            "title": "vite",
            "cwd": null,
            "cmd": "vite",
            "workspace_id": 42,
            "workspace": 0,
            "scope": "workspace"
        })
    }

    #[test]
    fn list_returns_live_scoped_resources() {
        let transport =
            FakeTransport::new().with("surface.list", json!({"surfaces": [surface(3)]}));
        let bridge = Bridge::new(&transport, BridgeScope::Workspace(42));
        let result = list(&bridge).expect("resources");

        assert_eq!(result["resources"][0]["uri"], "pane://surface/3/content");
        assert_eq!(
            result["resourceTemplates"][0]["uriTemplate"],
            "pane://surface/{surface_id}/content"
        );
    }

    #[test]
    fn list_surfaces_ipc_failure_is_not_hidden_as_an_empty_list() {
        let transport = FakeTransport::new().with_err("surface.list", "socket down");
        let bridge = Bridge::new(&transport, BridgeScope::All);

        assert!(list(&bridge).is_err());
    }

    #[test]
    fn read_wraps_content_and_keeps_bridge_errors_typed() {
        let transport = FakeTransport::new()
            .with("surface.list", json!({"surfaces": [surface(3)]}))
            .with(
                "surface.read",
                json!({"text": "ready", "total_lines": 1, "eof": true}),
            );
        let bridge = Bridge::new(&transport, BridgeScope::Workspace(42));
        let result = read("pane://surface/3/content", &bridge).expect("resource");

        assert!(result["contents"][0]["text"]
            .as_str()
            .unwrap()
            .contains("ready"));
        assert_eq!(
            transport.last_params("surface.read").unwrap()["workspace_id"],
            42
        );

        let invalid = read("file://nope", &bridge).expect_err("bad uri");
        assert!(matches!(invalid, ResourceError::NotFound(_)));

        let missing = read("pane://surface/99/content", &bridge).expect_err("missing surface");
        assert!(matches!(missing, ResourceError::NotFound(_)));

        let failed_transport = FakeTransport::new()
            .with("surface.list", json!({"surfaces": [surface(3)]}))
            .with_err("surface.read", "socket down");
        let failed_bridge = Bridge::new(&failed_transport, BridgeScope::All);
        let error = read("pane://surface/3/content", &failed_bridge).expect_err("IPC failure");
        assert!(matches!(error, ResourceError::Bridge(_)));
    }
}
