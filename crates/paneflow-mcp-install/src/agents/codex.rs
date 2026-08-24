//! Codex writer (EP-003 US-008).
//!
//! The writer uses a format-preserving `toml_edit` upsert of
//! `[mcp_servers.paneflow]` in `~/.codex/config.toml`, keeping comments and
//! sibling tables intact through one atomic, fully tested mutation path.
//!
//! **Volatility:** Codex's config schema moves fast (verified 2026:
//! `[mcp_servers.<name>]` with `command`/`args`). Re-verify that table shape
//! if registration regresses.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::agents::{support, AgentConfigWriter, InstallOutcome, StatusOutcome, UninstallOutcome};
use crate::detect::{self, Presence};

const CLI: &str = "codex";

pub struct Codex {
    config_path: Option<PathBuf>,
}

impl Codex {
    #[must_use]
    pub fn new() -> Self {
        Self {
            config_path: support::codex_config(),
        }
    }

    fn path(&self) -> Result<&Path> {
        self.config_path
            .as_deref()
            .ok_or_else(|| anyhow!("cannot resolve Codex config path"))
    }
}

impl Default for Codex {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentConfigWriter for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }
    fn label(&self) -> &'static str {
        "Codex"
    }

    fn presence(&self) -> Presence {
        // Detect via the config dir too: `~/.codex/` existing is a strong
        // signal even before `config.toml` is created.
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
        support::toml_install(self.path()?, &bridge_s)
    }

    fn uninstall(&self) -> Result<UninstallOutcome> {
        support::toml_uninstall(self.path()?)
    }

    fn status(&self, bridge: Option<&Path>) -> Result<StatusOutcome> {
        support::toml_status(self.path()?, bridge)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_writer(path: PathBuf) -> Codex {
        Codex {
            config_path: Some(path),
        }
    }

    #[test]
    fn install_writes_mcp_servers_table() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("config.toml");
        let w = test_writer(p.clone());
        assert_eq!(
            w.install(Path::new("/data/paneflow-mcp")).unwrap(),
            InstallOutcome::Installed
        );
        let txt = std::fs::read_to_string(&p).unwrap();
        assert!(txt.contains("paneflow"));
        assert!(txt.contains("/data/paneflow-mcp"));
        // Re-parse to confirm the table path.
        let doc = txt.parse::<toml_edit::DocumentMut>().unwrap();
        assert_eq!(
            doc["mcp_servers"]["paneflow"]["command"].as_str(),
            Some("/data/paneflow-mcp")
        );
    }

    #[test]
    fn install_preserves_existing_config_and_comments() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(
            &p,
            "# codex config\nmodel = \"gpt-5\"\n\n[mcp_servers.github]\ncommand = \"gh-mcp\"\n",
        )
        .unwrap();
        let w = test_writer(p.clone());
        w.install(Path::new("/data/paneflow-mcp")).unwrap();

        let txt = std::fs::read_to_string(&p).unwrap();
        assert!(txt.contains("# codex config"));
        assert!(txt.contains("model = \"gpt-5\""));
        assert!(txt.contains("gh-mcp"), "sibling server preserved");
        assert!(txt.contains("/data/paneflow-mcp"));
    }

    #[test]
    fn install_idempotent_and_uninstall() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("config.toml");
        let w = test_writer(p);
        w.install(Path::new("/data/paneflow-mcp")).unwrap();
        assert_eq!(
            w.install(Path::new("/data/paneflow-mcp")).unwrap(),
            InstallOutcome::AlreadyCurrent
        );
        assert_eq!(w.uninstall().unwrap(), UninstallOutcome::Removed);
        assert_eq!(w.uninstall().unwrap(), UninstallOutcome::NothingToRemove);
    }

    #[test]
    fn status_needs_repair_when_disabled() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(
            &p,
            "[mcp_servers.paneflow]\ncommand = \"/data/paneflow-mcp\"\nargs = []\nenabled = false\n",
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
        // US-021: symmetric with the Claude Code writer - a present-but-
        // unparseable config is corruption, surfaced loudly, not swallowed
        // as NothingToRemove.
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, b"this = = broken").unwrap();
        let w = test_writer(p.clone());
        assert!(
            w.uninstall().is_err(),
            "uninstall on a malformed present config must error, not return NothingToRemove"
        );
        assert_eq!(std::fs::read(&p).unwrap(), b"this = = broken");
    }

    #[test]
    fn uninstall_absent_config_is_nothing_to_remove() {
        let dir = tempfile::TempDir::new().unwrap();
        let w = test_writer(dir.path().join("missing.toml"));
        assert_eq!(w.uninstall().unwrap(), UninstallOutcome::NothingToRemove);
    }

    #[test]
    fn install_refuses_invalid_toml() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, b"this = = broken").unwrap();
        let w = test_writer(p.clone());
        assert!(w.install(Path::new("/data/paneflow-mcp")).is_err());
        assert_eq!(std::fs::read(&p).unwrap(), b"this = = broken");
    }
}
