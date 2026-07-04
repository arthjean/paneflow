//! opencode writer (EP-003 US-010).
//!
//! opencode's schema diverges from every other agent:
//! - the container key is **`mcp`**, not `mcpServers`;
//! - the entry is `{type: "local", command: [<path>], enabled: true}` -
//!   `command` is an **array**, with the binary path as its first element.
//!
//! Config lives in opencode's global config path, preferring JSONC when an
//! existing `opencode.jsonc` is present. No CLI mutates server config, so this
//! is always a direct merge - preserving `$schema` and sibling `mcp.*` entries.
//!
//! **Volatility:** opencode's config schema is young; re-verify the `mcp`
//! key, `type: "local"`, and array `command` if registration regresses.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use serde_json::json;

use crate::agents::{support, AgentConfigWriter, InstallOutcome, StatusOutcome, UninstallOutcome};
use crate::detect::{self, Presence};

const CLI: &str = "opencode";
const CONTAINER: &str = "mcp";

pub struct OpenCode {
    config_paths: Vec<PathBuf>,
}

impl OpenCode {
    #[must_use]
    pub fn new() -> Self {
        Self {
            config_paths: support::opencode_configs(),
        }
    }

    fn path(&self) -> Result<&Path> {
        self.config_paths
            .iter()
            .find(|p| p.exists())
            .or_else(|| self.config_paths.first())
            .map(PathBuf::as_path)
            .ok_or_else(|| anyhow!("cannot resolve opencode config path"))
    }

    fn entry(bridge: &str) -> serde_json::Value {
        // `command` is an ARRAY for opencode; `type: "local"` marks a stdio
        // child process; `enabled: true` activates it.
        json!({ "type": "local", "command": [bridge], "enabled": true })
    }

    fn validate_entry(entry: &serde_json::Value, expected: Option<&Path>) -> StatusOutcome {
        let found = support::array_command(entry);
        let shape_ok = found
            .as_deref()
            .is_some_and(|path| *entry == Self::entry(path));
        support::classify_entry(
            found,
            expected,
            shape_ok,
            "opencode MCP entry must be local, enabled, and use command array form",
        )
    }
}

impl Default for OpenCode {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentConfigWriter for OpenCode {
    fn id(&self) -> &'static str {
        "opencode"
    }
    fn label(&self) -> &'static str {
        "opencode"
    }

    fn presence(&self) -> Presence {
        detect::detect(Some(CLI), &self.config_paths)
    }

    fn install(&self, bridge: &Path) -> Result<InstallOutcome> {
        let bridge_s = bridge.to_string_lossy().into_owned();
        support::json_install(self.path()?, CONTAINER, Self::entry(&bridge_s))
    }

    fn uninstall(&self) -> Result<UninstallOutcome> {
        support::json_uninstall(self.path()?, CONTAINER)
    }

    fn status(&self, bridge: Option<&Path>) -> Result<StatusOutcome> {
        // opencode stores `command` as an array → use the array extractor.
        support::json_status(self.path()?, CONTAINER, bridge, Self::validate_entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_writer(path: PathBuf) -> OpenCode {
        OpenCode {
            config_paths: vec![path],
        }
    }

    #[test]
    fn install_writes_local_array_entry_under_mcp() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("opencode.json");
        let w = test_writer(p.clone());
        assert_eq!(
            w.install(Path::new("/data/paneflow-mcp")).unwrap(),
            InstallOutcome::Installed
        );
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        let entry = &v["mcp"]["paneflow"];
        assert_eq!(entry["type"], json!("local"));
        assert_eq!(
            entry["command"],
            json!(["/data/paneflow-mcp"]),
            "command is an array"
        );
        assert_eq!(entry["enabled"], json!(true));
        // Must NOT land under mcpServers.
        assert!(v.get("mcpServers").is_none());
    }

    #[test]
    fn install_preserves_schema_and_sibling_mcp_entries() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("opencode.json");
        std::fs::write(
            &p,
            serde_json::to_vec(&json!({
                "$schema": "https://opencode.ai/config.json",
                "mcp": { "weather": { "type": "local", "command": ["weather-mcp"], "enabled": true } }
            }))
            .unwrap(),
        )
        .unwrap();
        let w = test_writer(p.clone());
        w.install(Path::new("/data/paneflow-mcp")).unwrap();

        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(v["$schema"], json!("https://opencode.ai/config.json"));
        assert_eq!(v["mcp"]["weather"]["command"], json!(["weather-mcp"]));
        assert_eq!(
            v["mcp"]["paneflow"]["command"],
            json!(["/data/paneflow-mcp"])
        );
    }

    #[test]
    fn status_reads_array_command_and_flags_stale() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("opencode.json");
        let w = test_writer(p);
        w.install(Path::new("/old/paneflow-mcp")).unwrap();
        assert_eq!(
            w.status(Some(Path::new("/old/paneflow-mcp"))).unwrap(),
            StatusOutcome::Installed {
                path: "/old/paneflow-mcp".into()
            }
        );
        assert_eq!(
            w.status(Some(Path::new("/new/paneflow-mcp"))).unwrap(),
            StatusOutcome::StalePath {
                found: "/old/paneflow-mcp".into(),
                expected: "/new/paneflow-mcp".into()
            }
        );
    }

    #[test]
    fn status_needs_repair_when_disabled() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("opencode.json");
        std::fs::write(
            &p,
            serde_json::to_vec(&json!({
                "mcp": {
                    "paneflow": {
                        "type": "local",
                        "command": ["/data/paneflow-mcp"],
                        "enabled": false
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let w = test_writer(p);

        assert!(matches!(
            w.status(Some(Path::new("/data/paneflow-mcp"))).unwrap(),
            StatusOutcome::NeedsRepair { .. }
        ));
    }

    #[test]
    fn install_updates_existing_jsonc_candidate() {
        let dir = tempfile::TempDir::new().unwrap();
        let jsonc = dir.path().join("opencode.jsonc");
        let json = dir.path().join("opencode.json");
        std::fs::write(
            &jsonc,
            br#"
{
  // keep this file selected
  "mcp": {
    "weather": { "type": "local", "command": ["weather-mcp"], "enabled": true },
  },
}
"#,
        )
        .unwrap();
        let w = OpenCode {
            config_paths: vec![jsonc.clone(), json.clone()],
        };

        assert_eq!(
            w.install(Path::new("/data/paneflow-mcp")).unwrap(),
            InstallOutcome::Installed
        );
        assert!(jsonc.exists());
        assert!(!json.exists());
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&jsonc).unwrap()).unwrap();
        assert_eq!(
            v["mcp"]["paneflow"]["command"],
            json!(["/data/paneflow-mcp"])
        );
        assert_eq!(v["mcp"]["weather"]["command"], json!(["weather-mcp"]));
    }

    #[test]
    fn uninstall_malformed_config_is_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("opencode.json");
        std::fs::write(&p, b"{ broken").unwrap();
        let w = test_writer(p.clone());

        assert!(w.uninstall().is_err());
        assert_eq!(std::fs::read(&p).unwrap(), b"{ broken");
    }
}
