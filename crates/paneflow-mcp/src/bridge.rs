use std::fmt;

use paneflow_ipc_client::IpcTransport;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Map, Value};

use crate::resolve::{self, SurfaceRef};
use crate::scope::BridgeScope;

pub const MAX_LINES: u64 = 4000;
pub const MAX_MATCHES: u64 = 1000;
pub const MAX_SAFE_JSON_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeError {
    Target(String),
    Transport {
        method: &'static str,
        message: String,
    },
    Protocol {
        method: &'static str,
        message: String,
    },
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Target(message) => f.write_str(message),
            Self::Transport { method, message } => {
                write!(f, "Paneflow IPC {method} failed: {message}")
            }
            Self::Protocol { method, message } => {
                write!(
                    f,
                    "Paneflow IPC {method} returned an invalid response: {message}"
                )
            }
        }
    }
}

impl std::error::Error for BridgeError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceTarget {
    Id(u64),
    Name(String),
}

impl SurfaceTarget {
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Id(surface_id) => surface_id.to_string(),
            Self::Name(name) => name.clone(),
        }
    }
}

impl<'de> Deserialize<'de> for SurfaceTarget {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        match value {
            Value::String(name) if !name.trim().is_empty() => Ok(Self::Name(name)),
            Value::String(_) => Err(serde::de::Error::custom("surface name must not be empty")),
            Value::Number(number) => {
                let id = number.as_u64().or_else(|| {
                    number.as_f64().and_then(|value| {
                        (value.is_finite()
                            && value.fract() == 0.0
                            && value >= 0.0
                            && value <= MAX_SAFE_JSON_INTEGER as f64)
                            .then_some(value as u64)
                    })
                });
                match id.filter(|id| *id <= MAX_SAFE_JSON_INTEGER) {
                    Some(id) => Ok(Self::Id(id)),
                    None => Err(serde::de::Error::custom(format!(
                        "surface_id must be an integer between 0 and {MAX_SAFE_JSON_INTEGER}"
                    ))),
                }
            }
            _ => Err(serde::de::Error::custom(
                "target must be a surface name or numeric surface_id",
            )),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Surface {
    pub surface_id: u64,
    pub name: String,
    pub title: String,
    pub cwd: Option<String>,
    pub cmd: Option<String>,
    pub workspace_id: Option<u64>,
    pub workspace: Option<u64>,
    pub scope: String,
    /// US-019 (`prd-cli-tab-hierarchy`): identity and title of the workspace
    /// tab owning this surface. Both are additive and absent for surfaces
    /// outside the CLI tab hierarchy - and absent altogether when talking to a
    /// Paneflow older than the tab hierarchy, which simply never sends them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SurfaceListResult {
    surfaces: Vec<Surface>,
}

#[derive(Debug, Deserialize)]
pub struct SurfaceReadResult {
    pub text: String,
    pub total_lines: u64,
    pub eof: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchMatch {
    pub line: i64,
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub struct SurfaceSearchResult {
    pub matches: Vec<SearchMatch>,
    pub truncated: bool,
}

/// Typed, scope-aware adapter over Paneflow's raw JSON IPC transport. MCP
/// tools and resources share this single canonical path for target resolution,
/// response validation, and stable-workspace authorization parameters.
pub struct Bridge<'a, T: IpcTransport + ?Sized> {
    transport: &'a T,
    scope: BridgeScope,
}

impl<'a, T: IpcTransport + ?Sized> Bridge<'a, T> {
    pub fn new(transport: &'a T, scope: BridgeScope) -> Self {
        Self { transport, scope }
    }

    pub fn scope(&self) -> BridgeScope {
        self.scope
    }

    pub fn surfaces(&self) -> Result<Vec<Surface>, BridgeError> {
        let result: SurfaceListResult = self.call("surface.list", self.scope.ipc_params())?;
        if let Some(expected) = self.scope.workspace_id() {
            if let Some(surface) = result
                .surfaces
                .iter()
                .find(|surface| surface.workspace_id != Some(expected))
            {
                return Err(BridgeError::Protocol {
                    method: "surface.list",
                    message: format!(
                        "surface_id {} escaped requested workspace_id {expected}",
                        surface.surface_id
                    ),
                });
            }
        }
        Ok(result.surfaces)
    }

    pub fn resolve_target(&self, target: &SurfaceTarget) -> Result<u64, BridgeError> {
        match target {
            SurfaceTarget::Id(surface_id) => Ok(*surface_id),
            SurfaceTarget::Name(name) => {
                let surfaces = self.surfaces()?;
                let refs: Vec<SurfaceRef> = surfaces
                    .iter()
                    .map(|surface| SurfaceRef {
                        surface_id: surface.surface_id,
                        name: surface.name.clone(),
                    })
                    .collect();
                resolve::resolve_target(&refs, name).map_err(BridgeError::Target)
            }
        }
    }

    pub fn read_surface(
        &self,
        surface_id: u64,
        lines: Option<u64>,
        offset: Option<u64>,
    ) -> Result<SurfaceReadResult, BridgeError> {
        let mut params = self.surface_params(surface_id);
        params.insert("fenced".into(), json!(false));
        if let Some(lines) = lines {
            params.insert("lines".into(), json!(lines));
        }
        if let Some(offset) = offset {
            params.insert("offset".into(), json!(offset));
        }
        self.call("surface.read", Value::Object(params))
    }

    pub fn search_surface(
        &self,
        surface_id: u64,
        pattern: &str,
        max_matches: Option<u64>,
    ) -> Result<SurfaceSearchResult, BridgeError> {
        let mut params = self.surface_params(surface_id);
        params.insert("pattern".into(), json!(pattern));
        if let Some(max_matches) = max_matches {
            params.insert("max_matches".into(), json!(max_matches));
        }
        self.call("surface.search", Value::Object(params))
    }

    fn surface_params(&self, surface_id: u64) -> Map<String, Value> {
        let mut params = Map::new();
        params.insert("surface_id".into(), json!(surface_id));
        self.scope.insert_ipc_param(&mut params);
        params
    }

    fn call<R: DeserializeOwned>(
        &self,
        method: &'static str,
        params: Value,
    ) -> Result<R, BridgeError> {
        let value = self
            .transport
            .call(method, params)
            .map_err(|message| BridgeError::Transport { method, message })?;
        serde_json::from_value(value).map_err(|error| BridgeError::Protocol {
            method,
            message: error.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::FakeTransport;

    fn surface(surface_id: u64, workspace_id: Option<u64>) -> Value {
        json!({
            "surface_id": surface_id,
            "name": format!("surface-{surface_id}"),
            "title": "shell",
            "cwd": null,
            "cmd": "zsh",
            "workspace_id": workspace_id,
            "workspace": 0,
            "scope": "workspace"
        })
    }

    /// US-019: the tab is read when the app sends it, and its absence - an
    /// older Paneflow that predates the tab hierarchy - is not an error and
    /// leaves the reported payload byte-identical to what it was before.
    #[test]
    fn surface_list_reports_the_owning_tab_and_tolerates_its_absence() {
        let mut tabbed = surface(7, Some(42));
        tabbed["tab_id"] = json!(11);
        tabbed["tab_title"] = json!("build");
        let transport = FakeTransport::new().with("surface.list", json!({"surfaces": [tabbed]}));
        let bridge = Bridge::new(&transport, BridgeScope::Workspace(42));
        let surfaces = bridge.surfaces().expect("valid list");
        assert_eq!(surfaces[0].tab_id, Some(11));
        assert_eq!(surfaces[0].tab_title.as_deref(), Some("build"));
        assert_eq!(
            serde_json::to_value(&surfaces[0]).unwrap()["tab_title"],
            "build"
        );

        // Older server: no tab keys at all.
        let transport =
            FakeTransport::new().with("surface.list", json!({"surfaces": [surface(7, Some(42))]}));
        let bridge = Bridge::new(&transport, BridgeScope::Workspace(42));
        let legacy = bridge.surfaces().expect("a tab-less list still parses");
        assert_eq!(legacy[0].tab_id, None);
        let rendered = serde_json::to_value(&legacy[0]).unwrap();
        assert!(
            rendered.get("tab_id").is_none() && rendered.get("tab_title").is_none(),
            "an absent tab is omitted, not rendered as null"
        );
    }

    #[test]
    fn scoped_surface_list_passes_stable_id_to_server() {
        let transport =
            FakeTransport::new().with("surface.list", json!({"surfaces": [surface(7, Some(42))]}));
        let bridge = Bridge::new(&transport, BridgeScope::Workspace(42));

        assert_eq!(bridge.surfaces().expect("valid list").len(), 1);
        assert_eq!(
            transport.last_params("surface.list").unwrap()["workspace_id"],
            42
        );
    }

    #[test]
    fn scoped_surface_list_rejects_server_scope_escape() {
        let transport =
            FakeTransport::new().with("surface.list", json!({"surfaces": [surface(7, Some(99))]}));
        let bridge = Bridge::new(&transport, BridgeScope::Workspace(42));

        let error = bridge.surfaces().expect_err("scope escape must fail");
        assert!(error.to_string().contains("escaped requested workspace_id"));
    }

    #[test]
    fn malformed_ipc_shapes_are_errors_not_empty_successes() {
        let transport = FakeTransport::new()
            .with("surface.list", json!({"unexpected": []}))
            .with("surface.read", json!({"text": "missing metadata"}))
            .with("surface.search", json!({"matches": "not an array"}));
        let bridge = Bridge::new(&transport, BridgeScope::All);

        assert!(matches!(
            bridge.surfaces(),
            Err(BridgeError::Protocol { .. })
        ));
        assert!(matches!(
            bridge.read_surface(1, None, None),
            Err(BridgeError::Protocol { .. })
        ));
        assert!(matches!(
            bridge.search_surface(1, "error", None),
            Err(BridgeError::Protocol { .. })
        ));
    }

    #[test]
    fn scoped_read_is_authorized_atomically_by_server() {
        let transport = FakeTransport::new().with(
            "surface.read",
            json!({"text": "ok", "total_lines": 1, "eof": true}),
        );
        let bridge = Bridge::new(&transport, BridgeScope::Workspace(42));

        bridge.read_surface(7, Some(20), Some(2)).expect("read");
        let params = transport.last_params("surface.read").unwrap();
        assert_eq!(params["workspace_id"], 42);
        assert_eq!(params["surface_id"], 7);
        assert_eq!(params["fenced"], false);
        assert_eq!(params["lines"], 20);
        assert_eq!(params["offset"], 2);
        assert!(transport.last_params("surface.list").is_none());
    }

    #[test]
    fn numeric_targets_are_bounded_to_json_safe_integers() {
        let parse = |value| serde_json::from_value::<SurfaceTarget>(value);
        assert_eq!(parse(json!(42)).unwrap(), SurfaceTarget::Id(42));
        assert_eq!(parse(json!(42.0)).unwrap(), SurfaceTarget::Id(42));
        assert!(parse(json!(42.5)).is_err());
        assert!(parse(json!(MAX_SAFE_JSON_INTEGER + 1)).is_err());
        assert!(parse(json!(u64::MAX)).is_err());
    }
}
