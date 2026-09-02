use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use serde_json::json;

use crate::agents::{support, AgentConfigWriter, InstallOutcome, StatusOutcome, UninstallOutcome};
use crate::detect::{self, Presence};

const CLI: &str = "gemini";
const CONTAINER: &str = "mcpServers";

pub struct Gemini {
    config_path: Option<PathBuf>,
}

impl Gemini {
    #[must_use]
    pub fn new() -> Self {
        Self {
            config_path: support::gemini_config(),
        }
    }

    fn path(&self) -> Result<&Path> {
        self.config_path
            .as_deref()
            .ok_or_else(|| anyhow!("cannot resolve home dir for ~/.gemini/settings.json"))
    }

    fn entry(bridge: &str) -> serde_json::Value {
        json!({ "command": bridge, "args": [], "trust": true })
    }

    fn validate_entry(entry: &serde_json::Value, expected: Option<&Path>) -> StatusOutcome {
        let found = support::string_command(entry);
        let shape_ok = found
            .as_deref()
            .is_some_and(|path| *entry == Self::entry(path));
        support::classify_entry(
            found,
            expected,
            shape_ok,
            "Gemini MCP entry must have empty args and trust=true",
        )
    }
}

impl Default for Gemini {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentConfigWriter for Gemini {
    fn id(&self) -> &'static str {
        "gemini"
    }
    fn label(&self) -> &'static str {
        "Gemini CLI"
    }

    fn presence(&self) -> Presence {
        let mut paths: Vec<PathBuf> = Vec::new();
        if let Some(cfg) = &self.config_path {
            paths.push(cfg.clone());
            if let Some(parent) = cfg.parent() {
                paths.push(parent.to_path_buf());
            }
        }
        detect::detect(Some(CLI), &paths)
    }

    fn install(&self, bridge: &Path) -> Result<InstallOutcome> {
        let bridge_s = bridge.to_string_lossy().into_owned();
        support::json_install(self.path()?, CONTAINER, Self::entry(&bridge_s))
    }

    fn uninstall(&self) -> Result<UninstallOutcome> {
        support::json_uninstall(self.path()?, CONTAINER)
    }

    fn status(&self, bridge: Option<&Path>) -> Result<StatusOutcome> {
        support::json_status(self.path()?, CONTAINER, bridge, Self::validate_entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_writer(path: PathBuf) -> Gemini {
        Gemini {
            config_path: Some(path),
        }
    }

    #[test]
    fn install_writes_trusted_entry() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("settings.json");
        let w = test_writer(p.clone());
        assert_eq!(
            w.install(Path::new("/data/paneflow-mcp")).unwrap(),
            InstallOutcome::Installed
        );
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        let entry = &v["mcpServers"]["paneflow"];
        assert_eq!(entry["command"], json!("/data/paneflow-mcp"));
        assert_eq!(entry["trust"], json!(true));
        assert!(entry.get("env").is_none(), "D5: no env block");
    }

    #[test]
    fn install_preserves_other_settings_and_servers() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("settings.json");
        std::fs::write(
            &p,
            serde_json::to_vec(&json!({
                "theme": "GitHub",
                "mcpServers": { "context7": { "command": "c7" } }
            }))
            .unwrap(),
        )
        .unwrap();
        let w = test_writer(p.clone());
        w.install(Path::new("/data/paneflow-mcp")).unwrap();

        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(v["theme"], json!("GitHub"));
        assert_eq!(v["mcpServers"]["context7"]["command"], json!("c7"));
        assert_eq!(v["mcpServers"]["paneflow"]["trust"], json!(true));
    }

    #[test]
    fn idempotent_and_uninstall() {
        let dir = tempfile::TempDir::new().unwrap();
        let w = test_writer(dir.path().join("settings.json"));
        w.install(Path::new("/data/paneflow-mcp")).unwrap();
        assert_eq!(
            w.install(Path::new("/data/paneflow-mcp")).unwrap(),
            InstallOutcome::AlreadyCurrent
        );
        assert_eq!(w.uninstall().unwrap(), UninstallOutcome::Removed);
    }

    #[test]
    fn status_needs_repair_when_not_trusted() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("settings.json");
        std::fs::write(
            &p,
            serde_json::to_vec(&json!({
                "mcpServers": {
                    "paneflow": {
                        "command": "/data/paneflow-mcp",
                        "args": [],
                        "trust": false
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
    fn uninstall_malformed_config_is_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("settings.json");
        std::fs::write(&p, b"{ broken").unwrap();
        let w = test_writer(p.clone());

        assert!(w.uninstall().is_err());
        assert_eq!(std::fs::read(&p).unwrap(), b"{ broken");
    }
}
