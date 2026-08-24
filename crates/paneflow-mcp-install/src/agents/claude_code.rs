//! Claude Code writer (EP-003 US-007).
//!
//! The writer atomically merges the entry into `~/.claude.json` under
//! `mcpServers.paneflow`. One canonical mutation path keeps installs
//! deterministic and testable without destructive remove/add shell-outs.
//!
//! The entry carries **no `env` block** (PRD D5): the bridge inherits
//! `PANEFLOW_SOCKET_PATH` from the pane it runs in. Per 2026 verification
//! the entry also carries `type: "stdio"`.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use serde_json::json;

use crate::agents::{support, AgentConfigWriter, InstallOutcome, StatusOutcome, UninstallOutcome};
use crate::detect::{self, Presence};

const CLI: &str = "claude";
const CONTAINER: &str = "mcpServers";

pub struct ClaudeCode {
    config_path: Option<PathBuf>,
}

impl ClaudeCode {
    #[must_use]
    pub fn new() -> Self {
        Self {
            config_path: support::claude_config(),
        }
    }

    fn path(&self) -> Result<&Path> {
        self.config_path
            .as_deref()
            .ok_or_else(|| anyhow!("cannot resolve home dir for ~/.claude.json"))
    }

    fn entry(bridge: &str) -> serde_json::Value {
        // No `env` (D5). `type: "stdio"` matches what `claude mcp add` writes.
        json!({ "type": "stdio", "command": bridge, "args": [] })
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
            "Claude Code MCP entry must be stdio, have empty args, and no env block",
        )
    }
}

impl Default for ClaudeCode {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentConfigWriter for ClaudeCode {
    fn id(&self) -> &'static str {
        "claude-code"
    }
    fn label(&self) -> &'static str {
        "Claude Code"
    }

    fn presence(&self) -> Presence {
        let paths: Vec<PathBuf> = self.config_path.clone().into_iter().collect();
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

    fn test_writer(path: PathBuf) -> ClaudeCode {
        ClaudeCode {
            config_path: Some(path),
        }
    }

    #[test]
    fn install_writes_stdio_entry_without_env() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join(".claude.json");
        let w = test_writer(p.clone());

        assert_eq!(
            w.install(Path::new("/data/paneflow-mcp")).unwrap(),
            InstallOutcome::Installed
        );
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        let entry = &v["mcpServers"]["paneflow"];
        assert_eq!(entry["type"], json!("stdio"));
        assert_eq!(entry["command"], json!("/data/paneflow-mcp"));
        assert_eq!(entry["args"], json!([]));
        assert!(
            entry.get("env").is_none(),
            "D5: entry must carry no env block"
        );
    }

    #[test]
    fn install_is_idempotent() {
        let dir = tempfile::TempDir::new().unwrap();
        let w = test_writer(dir.path().join(".claude.json"));
        w.install(Path::new("/data/paneflow-mcp")).unwrap();
        assert_eq!(
            w.install(Path::new("/data/paneflow-mcp")).unwrap(),
            InstallOutcome::AlreadyCurrent
        );
    }

    #[test]
    fn status_needs_repair_when_shape_differs() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join(".claude.json");
        std::fs::write(
            &p,
            serde_json::to_vec(&json!({
                "mcpServers": {
                    "paneflow": {
                        "type": "stdio",
                        "command": "/data/paneflow-mcp",
                        "args": [],
                        "env": { "SHOULD_NOT_BE_HERE": "1" }
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
    fn install_preserves_unrelated_claude_state() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join(".claude.json");
        std::fs::write(
            &p,
            serde_json::to_vec(&json!({
                "numStartups": 42,
                "mcpServers": { "github": { "command": "gh-mcp" } }
            }))
            .unwrap(),
        )
        .unwrap();
        let w = test_writer(p.clone());
        w.install(Path::new("/data/paneflow-mcp")).unwrap();

        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(v["numStartups"], json!(42));
        assert_eq!(v["mcpServers"]["github"]["command"], json!("gh-mcp"));
        assert_eq!(
            v["mcpServers"]["paneflow"]["command"],
            json!("/data/paneflow-mcp")
        );
    }

    #[test]
    fn uninstall_malformed_config_is_error() {
        // US-021: a present-but-unparseable config is corruption, not
        // "nothing to remove" - surface a loud error so the user fixes it
        // rather than silently believing the entry was already gone.
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join(".claude.json");
        std::fs::write(&p, b"{ broken").unwrap();
        let w = test_writer(p.clone());
        assert!(
            w.uninstall().is_err(),
            "uninstall on a malformed present config must error, not return NothingToRemove"
        );
        // The invalid file was NOT overwritten.
        assert_eq!(std::fs::read(&p).unwrap(), b"{ broken");
    }

    #[test]
    fn uninstall_absent_config_is_nothing_to_remove() {
        // Counterpart to the malformed case: a genuinely absent file is a
        // clean NothingToRemove, not an error.
        let dir = tempfile::TempDir::new().unwrap();
        let w = test_writer(dir.path().join("missing.json"));
        assert_eq!(w.uninstall().unwrap(), UninstallOutcome::NothingToRemove);
    }

    #[test]
    fn uninstall_then_status_roundtrip() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join(".claude.json");
        let w = test_writer(p);
        w.install(Path::new("/data/paneflow-mcp")).unwrap();
        assert_eq!(
            w.status(Some(Path::new("/data/paneflow-mcp"))).unwrap(),
            StatusOutcome::Installed {
                path: "/data/paneflow-mcp".into()
            }
        );
        assert_eq!(w.uninstall().unwrap(), UninstallOutcome::Removed);
        assert_eq!(
            w.status(Some(Path::new("/data/paneflow-mcp"))).unwrap(),
            StatusOutcome::NotInstalled
        );
    }
}
