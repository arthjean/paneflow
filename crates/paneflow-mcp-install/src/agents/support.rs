use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use paneflow_agent_config::jsonc;

use crate::agents::{InstallOutcome, StatusOutcome, UninstallOutcome};
use crate::{io, merge};

pub(crate) const ENTRY: &str = "paneflow";

pub(crate) fn claude_config() -> Option<PathBuf> {
    paneflow_agent_config::ClaudePaths::current().map(|paths| paths.global_config)
}

pub(crate) fn codex_config() -> Option<PathBuf> {
    codex_config_from(dirs::home_dir(), std::env::var_os("CODEX_HOME"))
}

fn codex_config_from(home: Option<PathBuf>, codex_home: Option<OsString>) -> Option<PathBuf> {
    paneflow_agent_config::codex_home_from(codex_home, home).map(|h| h.join("config.toml"))
}

pub(crate) fn gemini_config() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".gemini").join("settings.json"))
}

pub(crate) fn opencode_configs() -> Vec<PathBuf> {
    opencode_configs_from(
        dirs::home_dir(),
        dirs::config_dir(),
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("OPENCODE_CONFIG"),
        std::env::var_os("OPENCODE_CONFIG_DIR"),
    )
}

fn opencode_configs_from(
    home: Option<PathBuf>,
    _platform_config_dir: Option<PathBuf>,
    _xdg_config_home: Option<OsString>,
    opencode_config: Option<OsString>,
    opencode_config_dir: Option<OsString>,
) -> Vec<PathBuf> {
    if let Some(config) = opencode_config
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
    {
        return vec![config];
    }

    let mut out = Vec::new();
    if let Some(dir) =
        paneflow_agent_config::absolute_env_dir("OPENCODE_CONFIG_DIR", opencode_config_dir)
    {
        push_opencode_files(&mut out, &dir);
        return out;
    }

    #[cfg(windows)]
    {
        if let Some(home) = home.clone() {
            push_opencode_names(&mut out, home.join(".config"));
        }
        if let Some(dir) = _platform_config_dir {
            push_opencode_names(&mut out, dir);
        }
    }

    #[cfg(not(windows))]
    {
        if let Some(dir) = _xdg_config_home
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .or_else(|| home.map(|h| h.join(".config")))
        {
            push_opencode_names(&mut out, dir);
        }
    }

    out
}

fn push_opencode_names(out: &mut Vec<PathBuf>, config_base: PathBuf) {
    push_opencode_files(out, &config_base.join("opencode"));
}

fn push_opencode_files(out: &mut Vec<PathBuf>, dir: &Path) {
    out.push(dir.join("opencode.jsonc"));
    out.push(dir.join("opencode.json"));
}

pub(crate) fn json_install(
    path: &Path,
    container: &str,
    managed: &serde_json::Value,
    defaults: &serde_json::Value,
    jsonc: bool,
) -> Result<InstallOutcome> {
    io::with_config_lock(path, || {
        if jsonc {
            let source = read_jsonc_source(path)?;
            let root = jsonc::parse(&source)
                .with_context(|| format!("{} is not valid JSONC", path.display()))?;
            let existing = root.get(container).and_then(|value| value.get(ENTRY));
            let had_prior = existing.is_some();
            let entry = merge::merged_entry(existing, managed, defaults);
            let Some(updated) = jsonc::upsert_entry(&source, container, ENTRY, &entry)
                .with_context(|| format!("edit {} failed", path.display()))?
            else {
                return Ok(InstallOutcome::AlreadyCurrent);
            };
            io::write_if_changed_unlocked(path, updated.as_bytes())?;
            return Ok(if had_prior {
                InstallOutcome::Updated
            } else {
                InstallOutcome::Installed
            });
        }

        let mut root = merge::read_json_or_default(path)?;
        let had_prior = root.get(container).and_then(|c| c.get(ENTRY)).is_some();
        let changed = merge::merge_json_entry(&mut root, container, ENTRY, managed, defaults)?;
        if !changed {
            return Ok(InstallOutcome::AlreadyCurrent);
        }
        io::write_if_changed_unlocked(path, &merge::json_to_bytes(&root)?)?;
        Ok(if had_prior {
            InstallOutcome::Updated
        } else {
            InstallOutcome::Installed
        })
    })
}

pub(crate) fn json_uninstall(
    path: &Path,
    container: &str,
    jsonc: bool,
) -> Result<UninstallOutcome> {
    if !path.exists() {
        return Ok(UninstallOutcome::NothingToRemove);
    }
    io::with_config_lock(path, || {
        if !path.exists() {
            return Ok(UninstallOutcome::NothingToRemove);
        }
        if jsonc {
            let source = read_jsonc_source(path)?;
            let Some(updated) = jsonc::remove_entry(&source, container, ENTRY)
                .with_context(|| format!("edit {} failed", path.display()))?
            else {
                return Ok(UninstallOutcome::NothingToRemove);
            };
            io::write_if_changed_unlocked(path, updated.as_bytes())?;
            return Ok(UninstallOutcome::Removed);
        }

        let mut root = merge::read_json_or_default(path)?;
        if !merge::remove_json_entry(&mut root, container, ENTRY) {
            return Ok(UninstallOutcome::NothingToRemove);
        }
        io::write_if_changed_unlocked(path, &merge::json_to_bytes(&root)?)?;
        Ok(UninstallOutcome::Removed)
    })
}

pub(crate) fn json_status(
    path: &Path,
    container: &str,
    jsonc: bool,
    expected: Option<&Path>,
    validate: impl Fn(&serde_json::Value, Option<&Path>) -> StatusOutcome,
) -> Result<StatusOutcome> {
    if !path.exists() {
        return Ok(StatusOutcome::NotInstalled);
    }
    let root = merge::read_config_or_default(path, jsonc)?;
    let Some(container_value) = root.get(container) else {
        return Ok(StatusOutcome::NotInstalled);
    };
    let Some(container_object) = container_value.as_object() else {
        bail!("config key `{container}` is not an object - refusing to classify it");
    };
    let Some(entry) = container_object.get(ENTRY) else {
        return Ok(StatusOutcome::NotInstalled);
    };
    Ok(validate(entry, expected))
}

fn read_jsonc_source(path: &Path) -> Result<String> {
    match crate::merge::read_agent_config_string(path) {
        Ok(source) => Ok(source),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok("{}\n".to_string()),
        Err(error) => Err(error).with_context(|| format!("read {} failed", path.display())),
    }
}

pub(crate) const CODEX_TABLE: &str = "mcp_servers";

pub(crate) const CODEX_BRIDGE_ENV_VARS: &[&str] = &[
    "PANEFLOW_SESSION_ID",
    "PANEFLOW_WORKSPACE_ID",
    "PANEFLOW_HOME",
    "PANEFLOW_SOCKET_PATH",
    "PANEFLOW_HOST_ENDPOINT",
    "XDG_RUNTIME_DIR",
];

pub(crate) fn codex_entry(command: &str) -> merge::TomlEntry<'_> {
    merge::TomlEntry {
        command,
        args: &[],
        env_vars: CODEX_BRIDGE_ENV_VARS,
    }
}

pub(crate) fn toml_install(path: &Path, command: &str) -> Result<InstallOutcome> {
    io::with_config_lock(path, || {
        let mut doc = merge::read_toml_or_default(path)?;
        let had_prior = doc.get(CODEX_TABLE).and_then(|t| t.get(ENTRY)).is_some();
        let changed =
            merge::upsert_toml_entry(&mut doc, CODEX_TABLE, ENTRY, &codex_entry(command))?;
        if !changed {
            return Ok(InstallOutcome::AlreadyCurrent);
        }
        io::write_if_changed_unlocked(path, &merge::toml_to_bytes(&doc))?;
        Ok(if had_prior {
            InstallOutcome::Updated
        } else {
            InstallOutcome::Installed
        })
    })
}

pub(crate) fn toml_uninstall(path: &Path) -> Result<UninstallOutcome> {
    if !path.exists() {
        return Ok(UninstallOutcome::NothingToRemove);
    }
    io::with_config_lock(path, || {
        if !path.exists() {
            return Ok(UninstallOutcome::NothingToRemove);
        }
        let mut doc = merge::read_toml_or_default(path)?;
        if !merge::remove_toml_entry(&mut doc, CODEX_TABLE, ENTRY) {
            return Ok(UninstallOutcome::NothingToRemove);
        }
        io::write_if_changed_unlocked(path, &merge::toml_to_bytes(&doc))?;
        Ok(UninstallOutcome::Removed)
    })
}

pub(crate) fn toml_status(path: &Path, expected: Option<&Path>) -> Result<StatusOutcome> {
    if !path.exists() {
        return Ok(StatusOutcome::NotInstalled);
    }
    let doc = merge::read_toml_or_default(path)?;
    let Some(entry) = doc.get(CODEX_TABLE).and_then(|t| t.get(ENTRY)) else {
        return Ok(StatusOutcome::NotInstalled);
    };
    let found = entry
        .get("command")
        .and_then(|c| c.as_str())
        .map(str::to_string);
    let args_ok = entry
        .get("args")
        .and_then(|a| a.as_array())
        .is_some_and(|args| args.is_empty());
    let env_vars: Vec<&str> = entry
        .get("env_vars")
        .and_then(|e| e.as_array())
        .map(|names| names.iter().filter_map(|name| name.as_str()).collect())
        .unwrap_or_default();
    let missing: Vec<&str> = CODEX_BRIDGE_ENV_VARS
        .iter()
        .copied()
        .filter(|name| !env_vars.contains(name))
        .collect();
    let disabled = entry.get("enabled").and_then(|e| e.as_bool()) == Some(false);
    let outcome = classify_entry(
        found,
        expected,
        args_ok && missing.is_empty(),
        &if missing.is_empty() {
            "Codex MCP entry must have empty args".to_string()
        } else {
            format!(
                "Codex MCP entry does not forward {} to the bridge",
                missing.join(", ")
            )
        },
    );
    Ok(if disabled {
        disabled_by_user(outcome)
    } else {
        outcome
    })
}

pub(crate) fn disabled_by_user(outcome: StatusOutcome) -> StatusOutcome {
    match outcome {
        StatusOutcome::Installed { path }
        | StatusOutcome::NeedsRepair {
            path: Some(path), ..
        } => StatusOutcome::DisabledByUser { path },
        other => other,
    }
}

pub(crate) fn has_fields(entry: &serde_json::Value, managed: &serde_json::Value) -> bool {
    match (entry.as_object(), managed.as_object()) {
        (Some(entry), Some(managed)) => managed
            .iter()
            .all(|(key, value)| entry.get(key) == Some(value)),
        _ => false,
    }
}

pub(crate) fn string_command(entry: &serde_json::Value) -> Option<String> {
    entry.get("command")?.as_str().map(str::to_string)
}

pub(crate) fn array_command(entry: &serde_json::Value) -> Option<String> {
    entry
        .get("command")?
        .as_array()?
        .first()?
        .as_str()
        .map(str::to_string)
}

pub(crate) fn classify_entry(
    found: Option<String>,
    expected: Option<&Path>,
    shape_ok: bool,
    repair_reason: &str,
) -> StatusOutcome {
    let Some(found) = found.filter(|p| !p.is_empty()) else {
        return StatusOutcome::NeedsRepair {
            path: None,
            reason: "MCP entry is missing a command path".to_string(),
        };
    };

    if let Some(expected) = expected {
        let expected = expected.to_string_lossy();
        if found != expected {
            return StatusOutcome::StalePath {
                found,
                expected: expected.into_owned(),
            };
        }
    }

    if !shape_ok {
        return StatusOutcome::NeedsRepair {
            path: Some(found),
            reason: repair_reason.to_string(),
        };
    }

    StatusOutcome::Installed { path: found }
}

pub(crate) fn config_presence(cli: &str, config: Option<&Path>) -> crate::detect::Presence {
    let mut paths: Vec<PathBuf> = Vec::new();
    if let Some(config) = config {
        paths.push(config.to_path_buf());
        if let Some(parent) = config.parent() {
            paths.push(parent.to_path_buf());
        }
    }
    crate::detect::detect(Some(cli), &paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn validate_string_entry(entry: &serde_json::Value, expected: Option<&Path>) -> StatusOutcome {
        classify_entry(string_command(entry), expected, true, "shape mismatch")
    }

    #[test]
    fn codex_config_honors_codex_home() {
        let codex_home = std::env::temp_dir().join("codex-home");
        assert_eq!(
            codex_config_from(
                Some(PathBuf::from("/home/alice")),
                Some(codex_home.clone().into_os_string())
            )
            .unwrap(),
            codex_home.join("config.toml")
        );
        assert_eq!(
            codex_config_from(
                Some(PathBuf::from("/home/alice")),
                Some(OsString::from("relative"))
            )
            .unwrap(),
            PathBuf::from("/home/alice")
                .join(".codex")
                .join("config.toml")
        );
    }

    #[test]
    fn opencode_config_candidates_prefer_custom_path() {
        assert_eq!(
            opencode_configs_from(
                Some(PathBuf::from("/home/alice")),
                None,
                None,
                Some(OsString::from("/tmp/opencode.jsonc")),
                None,
            ),
            vec![PathBuf::from("/tmp/opencode.jsonc")]
        );
    }

    #[test]
    fn opencode_config_dir_holds_the_config_files_directly() {
        let dir = std::env::temp_dir().join("opencode-config");
        assert_eq!(
            opencode_configs_from(
                Some(PathBuf::from("/home/alice")),
                None,
                None,
                None,
                Some(dir.clone().into_os_string()),
            ),
            vec![dir.join("opencode.jsonc"), dir.join("opencode.json")]
        );
    }

    #[test]
    fn a_relative_opencode_config_dir_is_ignored() {
        let home = std::env::temp_dir().join("alice");
        let defaults = opencode_configs_from(Some(home.clone()), None, None, None, None);
        assert_eq!(
            opencode_configs_from(
                Some(home),
                None,
                None,
                None,
                Some(OsString::from("relative/opencode")),
            ),
            defaults
        );
    }

    #[test]
    fn json_install_then_idempotent() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("settings.json");
        let entry = json!({ "command": "/p", "args": [] });

        assert_eq!(
            json_install(&p, "mcpServers", &entry.clone(), &json!({}), false).unwrap(),
            InstallOutcome::Installed
        );
        assert_eq!(
            json_install(&p, "mcpServers", &entry, &json!({}), false).unwrap(),
            InstallOutcome::AlreadyCurrent
        );
        assert_eq!(
            json_install(
                &p,
                "mcpServers",
                &json!({ "command": "/q", "args": [] }),
                &json!({}),
                false
            )
            .unwrap(),
            InstallOutcome::Updated
        );
    }

    #[test]
    fn json_install_preserves_siblings() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("settings.json");
        std::fs::write(
            &p,
            serde_json::to_vec(&json!({
                "mcpServers": { "other": { "command": "x" } },
                "theme": "dark"
            }))
            .unwrap(),
        )
        .unwrap();

        json_install(
            &p,
            "mcpServers",
            &json!({ "command": "/p" }),
            &json!({}),
            false,
        )
        .unwrap();
        let after: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(after["mcpServers"]["other"]["command"], json!("x"));
        assert_eq!(after["theme"], json!("dark"));
        assert_eq!(after["mcpServers"]["paneflow"]["command"], json!("/p"));
    }

    #[test]
    fn json_install_refuses_invalid_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("settings.json");
        std::fs::write(&p, b"{ broken").unwrap();
        assert!(json_install(&p, "mcpServers", &json!({}), &json!({}), false).is_err());
        assert_eq!(std::fs::read(&p).unwrap(), b"{ broken");
    }

    #[test]
    fn json_uninstall_removes_only_target() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("settings.json");
        std::fs::write(
            &p,
            serde_json::to_vec(&json!({
                "mcpServers": { "paneflow": { "command": "/p" }, "other": { "command": "x" } }
            }))
            .unwrap(),
        )
        .unwrap();

        assert_eq!(
            json_uninstall(&p, "mcpServers", false).unwrap(),
            UninstallOutcome::Removed
        );
        let after: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert!(after["mcpServers"].get("paneflow").is_none());
        assert_eq!(after["mcpServers"]["other"]["command"], json!("x"));
        assert_eq!(
            json_uninstall(&p, "mcpServers", false).unwrap(),
            UninstallOutcome::NothingToRemove
        );
    }

    #[test]
    fn json_uninstall_absent_file_does_not_create_parent_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("missing-parent").join("settings.json");

        assert_eq!(
            json_uninstall(&p, "mcpServers", false).unwrap(),
            UninstallOutcome::NothingToRemove
        );
        assert!(!p.parent().unwrap().exists());
    }

    #[test]
    fn json_status_reports_installed_and_stale() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("settings.json");
        std::fs::write(
            &p,
            serde_json::to_vec(&json!({ "mcpServers": { "paneflow": { "command": "/cur" } } }))
                .unwrap(),
        )
        .unwrap();

        assert_eq!(
            json_status(
                &p,
                "mcpServers",
                false,
                Some(Path::new("/cur")),
                validate_string_entry,
            )
            .unwrap(),
            StatusOutcome::Installed {
                path: "/cur".into()
            }
        );
        assert_eq!(
            json_status(
                &p,
                "mcpServers",
                false,
                Some(Path::new("/new")),
                validate_string_entry,
            )
            .unwrap(),
            StatusOutcome::StalePath {
                found: "/cur".into(),
                expected: "/new".into()
            }
        );
    }

    #[test]
    fn json_status_not_installed_when_absent() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("missing.json");
        assert_eq!(
            json_status(
                &p,
                "mcpServers",
                false,
                Some(Path::new("/x")),
                validate_string_entry,
            )
            .unwrap(),
            StatusOutcome::NotInstalled
        );
    }

    #[test]
    fn json_status_without_expected_path_requires_command() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("settings.json");
        std::fs::write(
            &p,
            serde_json::to_vec(&json!({ "mcpServers": { "paneflow": { "args": [] } } })).unwrap(),
        )
        .unwrap();

        assert!(matches!(
            json_status(&p, "mcpServers", false, None, validate_string_entry).unwrap(),
            StatusOutcome::NeedsRepair { .. }
        ));
    }

    #[test]
    fn toml_install_idempotent_and_preserves_comments() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, b"# my codex config\nmodel = \"gpt-5\"\n").unwrap();

        assert_eq!(toml_install(&p, "/p").unwrap(), InstallOutcome::Installed);
        let txt = std::fs::read_to_string(&p).unwrap();
        assert!(txt.contains("# my codex config"));
        assert!(txt.contains("model = \"gpt-5\""));
        assert!(txt.contains("paneflow"));
        assert_eq!(
            toml_install(&p, "/p").unwrap(),
            InstallOutcome::AlreadyCurrent
        );
        assert_eq!(toml_install(&p, "/q").unwrap(), InstallOutcome::Updated);
    }

    #[test]
    fn toml_uninstall_and_status() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("config.toml");
        toml_install(&p, "/cur").unwrap();

        assert_eq!(
            toml_status(&p, Some(Path::new("/cur"))).unwrap(),
            StatusOutcome::Installed {
                path: "/cur".into()
            }
        );
        assert_eq!(
            toml_status(&p, Some(Path::new("/new"))).unwrap(),
            StatusOutcome::StalePath {
                found: "/cur".into(),
                expected: "/new".into()
            }
        );
        assert_eq!(toml_uninstall(&p).unwrap(), UninstallOutcome::Removed);
        assert_eq!(
            toml_uninstall(&p).unwrap(),
            UninstallOutcome::NothingToRemove
        );
    }

    #[test]
    fn toml_uninstall_absent_file_does_not_create_parent_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("missing-parent").join("config.toml");

        assert_eq!(
            toml_uninstall(&p).unwrap(),
            UninstallOutcome::NothingToRemove
        );
        assert!(!p.parent().unwrap().exists());
    }

    #[test]
    fn array_command_extracts_first_element() {
        let entry = json!({ "type": "local", "command": ["/bin/paneflow-mcp"], "enabled": true });
        assert_eq!(array_command(&entry), Some("/bin/paneflow-mcp".to_string()));
    }
}
