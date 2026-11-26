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
        json!({ "type": "local", "command": [bridge] })
    }

    fn defaults() -> serde_json::Value {
        json!({ "enabled": true })
    }

    fn validate_entry(entry: &serde_json::Value, expected: Option<&Path>) -> StatusOutcome {
        let found = support::array_command(entry);
        let shape_ok = found
            .as_deref()
            .is_some_and(|path| support::has_fields(entry, &Self::entry(path)));
        let outcome = support::classify_entry(
            found,
            expected,
            shape_ok,
            "opencode MCP entry must be local and use command array form",
        );
        if entry.get("enabled") == Some(&json!(false)) {
            support::disabled_by_user(outcome)
        } else {
            outcome
        }
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
        let path = self.path()?;
        support::json_install(
            path,
            CONTAINER,
            &Self::entry(&bridge_s),
            &Self::defaults(),
            crate::merge::has_jsonc_extension(path),
        )
    }

    fn uninstall(&self) -> Result<UninstallOutcome> {
        support::json_uninstall(
            self.path()?,
            CONTAINER,
            crate::merge::has_jsonc_extension(self.path()?),
        )
    }

    fn status(&self, bridge: Option<&Path>) -> Result<StatusOutcome> {
        support::json_status(
            self.path()?,
            CONTAINER,
            crate::merge::has_jsonc_extension(self.path()?),
            bridge,
            Self::validate_entry,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_fifo_jsonc_config_is_refused_as_not_regular_and_releases_the_lock() {
        let dir = tempfile::TempDir::new().unwrap();
        let jsonc = dir.path().join("opencode.jsonc");
        let made = std::process::Command::new("mkfifo")
            .arg(&jsonc)
            .status()
            .is_ok_and(|status| status.success());
        assert!(made);
        let w = test_writer(jsonc.clone());

        let installed = w.install(Path::new("/data/paneflow-mcp")).unwrap_err();
        assert_eq!(
            io_error_kind(&installed),
            Some(std::io::ErrorKind::InvalidInput)
        );
        let status = w.status(Some(Path::new("/data/paneflow-mcp"))).unwrap_err();
        assert_eq!(
            io_error_kind(&status),
            Some(std::io::ErrorKind::InvalidInput)
        );

        drop(crate::io::lock_config(&jsonc).unwrap());
    }

    #[cfg(unix)]
    fn io_error_kind(error: &anyhow::Error) -> Option<std::io::ErrorKind> {
        error
            .chain()
            .find_map(|cause| cause.downcast_ref::<std::io::Error>())
            .map(std::io::Error::kind)
    }

    fn test_writer(path: PathBuf) -> OpenCode {
        OpenCode {
            config_paths: vec![path],
        }
    }

    #[test]
    fn a_pathologically_nested_config_reports_a_parse_error_to_settings() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("opencode.jsonc");
        std::fs::write(
            &p,
            format!("{}{}", "{\"a\":".repeat(10_000) + "1", "}".repeat(10_000)),
        )
        .unwrap();
        let writers: Vec<Box<dyn AgentConfigWriter>> = vec![Box::new(test_writer(p))];

        let statuses = crate::api::status_with(Some(Path::new("/data/paneflow-mcp")), &writers);

        let crate::api::StatusKind::Error(message) = &statuses[0].kind else {
            unreachable!("expected a parse error, got {:?}", statuses[0].kind);
        };
        assert!(message.contains("not valid JSONC"), "{message}");
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
    fn status_reports_a_user_disabled_entry_and_install_keeps_it_disabled() {
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
        let w = test_writer(p.clone());

        assert_eq!(
            w.status(Some(Path::new("/data/paneflow-mcp"))).unwrap(),
            StatusOutcome::DisabledByUser {
                path: "/data/paneflow-mcp".into()
            }
        );
        assert_eq!(
            w.install(Path::new("/data/paneflow-mcp")).unwrap(),
            InstallOutcome::AlreadyCurrent
        );
        let root: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(root["mcp"]["paneflow"]["enabled"], json!(false));
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
        let after = std::fs::read_to_string(&jsonc).unwrap();
        assert!(after.contains("// keep this file selected"));
        let v = crate::merge::read_json_or_default(&jsonc).unwrap();
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
