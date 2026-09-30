use std::fmt;

use paneflow_ipc_client::IpcTransport;
use serde_json::{json, Value};

use crate::bridge::{Bridge, BridgeError, MAX_LINES, MAX_SAFE_JSON_INTEGER};
use crate::output::wrap_untrusted;

#[derive(Debug)]
pub enum ResourceError {
    NotFound(String),
    Invalid(String),
    Bridge(BridgeError),
}

impl fmt::Display for ResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(message) | Self::Invalid(message) => f.write_str(message),
            Self::Bridge(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ResourceError {}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PaneUri {
    surface_id: u64,
    lines: Option<u64>,
    offset: Option<u64>,
}

#[must_use]
pub fn templates() -> Value {
    json!({
        "resourceTemplates": [{
            "uriTemplate": "pane://surface/{surface_id}/content{?lines,offset}",
            "name": "Paneflow surface scrollback",
            "description": "Scrollback of a Paneflow surface, addressed by stable surface_id. Pages back with `lines` (1-4000, default 200) and `offset` (lines skipped from the most recent end). Names and titles are untrusted metadata; use list_panes for display.",
            "mimeType": "text/plain"
        }]
    })
}

pub fn list<T: IpcTransport + ?Sized>(bridge: &Bridge<'_, T>) -> Result<Value, BridgeError> {
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

    Ok(json!({ "resources": resources }))
}

pub fn read<T: IpcTransport + ?Sized>(
    uri: &str,
    bridge: &Bridge<'_, T>,
) -> Result<Value, ResourceError> {
    let target = parse_pane_uri(uri)?;
    let surface_id = target.surface_id;
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
        .read_surface(surface_id, target.lines, target.offset)
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

pub(crate) fn parse_pane_uri(uri: &str) -> Result<PaneUri, ResourceError> {
    let unsupported = || {
        ResourceError::NotFound(format!(
            "unsupported resource uri '{uri}' (expected pane://surface/<surface_id>/content)"
        ))
    };
    let (path, query) = uri.split_once('?').unwrap_or((uri, ""));
    let id = path
        .strip_prefix("pane://surface/")
        .and_then(|rest| rest.strip_suffix("/content"))
        .filter(|id| !id.is_empty() && id.chars().all(|character| character.is_ascii_digit()))
        .ok_or_else(unsupported)?;
    let surface_id = id.parse::<u64>().map_err(|_| unsupported())?;
    let mut target = PaneUri {
        surface_id,
        lines: None,
        offset: None,
    };
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let (slot, minimum, maximum) = match key {
            "lines" => (&mut target.lines, 1, MAX_LINES),
            "offset" => (&mut target.offset, 0, MAX_SAFE_JSON_INTEGER),
            other => {
                return Err(ResourceError::Invalid(format!(
                    "unknown resource parameter '{other}' (expected lines or offset)"
                )));
            }
        };
        let parsed = value
            .parse::<u64>()
            .ok()
            .filter(|parsed| (minimum..=maximum).contains(parsed))
            .ok_or_else(|| {
                ResourceError::Invalid(format!(
                    "'{key}' must be an integer between {minimum} and {maximum}"
                ))
            })?;
        if slot.replace(parsed).is_some() {
            return Err(ResourceError::Invalid(format!("'{key}' is repeated")));
        }
    }
    Ok(target)
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
        let transport = FakeTransport::new().with(
            "surface.list",
            json!({"surfaces": [surface(3)], "scope_workspace_id": 42}),
        );
        let bridge = Bridge::new(&transport, BridgeScope::Session("0a9e5266".into()));
        let result = list(&bridge).expect("resources");

        assert_eq!(result["resources"][0]["uri"], "pane://surface/3/content");
        assert!(result.get("resourceTemplates").is_none());
        assert_eq!(
            templates()["resourceTemplates"][0]["uriTemplate"],
            "pane://surface/{surface_id}/content{?lines,offset}"
        );
    }

    #[test]
    fn pagination_parameters_are_validated() {
        assert_eq!(
            parse_pane_uri("pane://surface/3/content?offset=10").unwrap(),
            PaneUri {
                surface_id: 3,
                lines: None,
                offset: Some(10)
            }
        );
        for uri in [
            "pane://surface/3/content?lines=0",
            "pane://surface/3/content?lines=4001",
            "pane://surface/3/content?lines=abc",
            "pane://surface/3/content?cursor=2",
            "pane://surface/3/content?lines=5&lines=6",
        ] {
            assert!(
                matches!(parse_pane_uri(uri), Err(ResourceError::Invalid(_))),
                "{uri}"
            );
        }
        assert!(matches!(
            parse_pane_uri("pane://surface/x/content?lines=5"),
            Err(ResourceError::NotFound(_))
        ));
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
            .with(
                "surface.list",
                json!({"surfaces": [surface(3)], "scope_workspace_id": 42}),
            )
            .with(
                "surface.read",
                json!({"text": "ready", "total_lines": 1, "eof": true}),
            );
        let bridge = Bridge::new(&transport, BridgeScope::Session("0a9e5266".into()));
        let result = read("pane://surface/3/content", &bridge).expect("resource");

        assert!(result["contents"][0]["text"]
            .as_str()
            .unwrap()
            .contains("ready"));
        assert_eq!(
            transport.last_params("surface.read").unwrap()["scope_session"],
            "0a9e5266"
        );

        read("pane://surface/3/content?lines=500&offset=200", &bridge).expect("paged read");
        let paged = transport.last_params("surface.read").unwrap();
        assert_eq!(paged["lines"], 500);
        assert_eq!(paged["offset"], 200);

        let invalid = read("file://nope", &bridge).expect_err("bad uri");
        assert!(matches!(invalid, ResourceError::NotFound(_)));

        let missing = read("pane://surface/99/content", &bridge).expect_err("missing surface");
        assert!(matches!(missing, ResourceError::NotFound(_)));

        let failed_transport = FakeTransport::new()
            .with(
                "surface.list",
                json!({"surfaces": [surface(3)], "scope_workspace_id": 42}),
            )
            .with_err("surface.read", "socket down");
        let failed_bridge = Bridge::new(&failed_transport, BridgeScope::All);
        let error = read("pane://surface/3/content", &failed_bridge).expect_err("IPC failure");
        assert!(matches!(error, ResourceError::Bridge(_)));
    }
}
