use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use serde_json::json;

use crate::agents::{support, AgentConfigWriter, InstallOutcome, StatusOutcome, UninstallOutcome};
use crate::detect::{self, Presence};

const CONTAINER: &str = "mcp";
const CONFIG_FILE: &str = "mcp.json";

pub struct Fx {
    home: Option<PathBuf>,
}

impl Fx {
    #[must_use]
    pub fn new() -> Self {
        Self {
            home: dirs::home_dir().map(|home| home.join(".fx")),
        }
    }

    #[must_use]
    pub fn at(home: PathBuf) -> Self {
        Self { home: Some(home) }
    }

    fn home(&self) -> Result<&Path> {
        self.home
            .as_deref()
            .ok_or_else(|| anyhow!("cannot resolve the fx home directory"))
    }

    fn path(&self) -> Result<PathBuf> {
        Ok(self.home()?.join(CONFIG_FILE))
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
            .is_some_and(|path| support::has_fields(entry, &Self::entry(path)))
            && entry.get("environment").is_none();
        let outcome = support::classify_entry(
            found,
            expected,
            shape_ok,
            "fx MCP entry must be local, use the command array form, and carry no environment block",
        );
        if entry.get("enabled") == Some(&json!(false)) {
            support::disabled_by_user(outcome)
        } else {
            outcome
        }
    }
}

impl Default for Fx {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentConfigWriter for Fx {
    fn id(&self) -> &'static str {
        "fx"
    }

    fn label(&self) -> &'static str {
        "fx"
    }

    fn presence(&self) -> Presence {
        detect::detect(None, &self.home.iter().cloned().collect::<Vec<_>>())
    }

    fn install(&self, bridge: &Path) -> Result<InstallOutcome> {
        let bridge = bridge.to_string_lossy().into_owned();
        support::json_install(
            &self.path()?,
            CONTAINER,
            &Self::entry(&bridge),
            &Self::defaults(),
            false,
        )
    }

    fn uninstall(&self) -> Result<UninstallOutcome> {
        support::json_uninstall(&self.path()?, CONTAINER, false)
    }

    fn status(&self, bridge: Option<&Path>) -> Result<StatusOutcome> {
        support::json_status(
            &self.path()?,
            CONTAINER,
            false,
            bridge,
            Self::validate_entry,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{install_with, InstallKind};
    use crate::integrations::InstallMode;

    #[test]
    fn the_install_engine_writes_a_local_paneflow_entry_without_an_environment_block() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path().join(".fx");
        std::fs::create_dir_all(&home).unwrap();
        let bridge = dir.path().join("paneflow-mcp");
        std::fs::write(&bridge, b"bridge").unwrap();
        let writers: Vec<Box<dyn AgentConfigWriter>> = vec![Box::new(Fx::at(home.clone()))];

        let results = install_with(Some(&bridge), &writers, InstallMode::default()).unwrap();

        assert_eq!(results[0].id, "fx");
        assert_eq!(results[0].kind, InstallKind::Installed);
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(home.join("mcp.json")).unwrap()).unwrap();
        let bridge = bridge.to_string_lossy().into_owned();
        assert_eq!(
            written,
            json!({
                "mcp": {
                    "paneflow": {
                        "type": "local",
                        "command": [bridge],
                        "enabled": true
                    }
                }
            })
        );
        assert_eq!(
            Fx::at(home).status(Some(Path::new(&bridge))).unwrap(),
            StatusOutcome::Installed { path: bridge }
        );
    }

    #[test]
    fn a_machine_without_an_fx_home_is_absent_even_when_an_fx_binary_is_on_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let writer = Fx::at(dir.path().join(".fx"));
        assert!(!writer.presence().is_present());
    }

    #[test]
    fn an_entry_that_gained_an_environment_block_needs_repair() {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path().join(".fx");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join("mcp.json"),
            serde_json::to_vec(&json!({
                "mcp": {
                    "paneflow": {
                        "type": "local",
                        "command": ["/data/paneflow-mcp"],
                        "enabled": true,
                        "environment": {"PANEFLOW_SURFACE_ID": "1"}
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(matches!(
            Fx::at(home)
                .status(Some(Path::new("/data/paneflow-mcp")))
                .unwrap(),
            StatusOutcome::NeedsRepair { .. }
        ));
    }
}
