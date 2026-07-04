//! Shared plumbing for the per-agent writers (EP-003).
//!
//! - Config-path resolution (cross-platform, `dirs`-based).
//! - `shell_out` - run an agent's own CLI and surface a clean error on
//!   non-zero exit (preferred path for Claude Code / Codex per PRD D4).
//! - Format-generic install / uninstall / status built on the tested
//!   [`crate::merge`] + [`crate::io`] primitives, so every writer is
//!   idempotent and no-clobber without repeating the logic.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Result};

use crate::agents::{InstallOutcome, StatusOutcome, UninstallOutcome};
use crate::{io, merge};

/// The entry name every writer registers under its container key.
pub(crate) const ENTRY: &str = "paneflow";

// ---------------------------------------------------------------------------
// Config paths (resolved against the real home / XDG dirs)
// ---------------------------------------------------------------------------

/// `~/.claude.json` - where `claude mcp add -s user` stores user-scope MCP
/// servers (verified 2026: NOT `~/.claude/settings.json`).
pub(crate) fn claude_config() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude.json"))
}

/// `$CODEX_HOME/config.toml`, falling back to `~/.codex/config.toml`.
pub(crate) fn codex_config() -> Option<PathBuf> {
    codex_config_from(dirs::home_dir(), std::env::var_os("CODEX_HOME"))
}

fn codex_config_from(home: Option<PathBuf>, codex_home: Option<OsString>) -> Option<PathBuf> {
    codex_home
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| home.map(|h| h.join(".codex")))
        .map(|h| h.join("config.toml"))
}

/// `~/.gemini/settings.json`.
pub(crate) fn gemini_config() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".gemini").join("settings.json"))
}

/// opencode global config candidates. Current opencode supports JSONC and
/// custom config env vars; the first existing candidate wins, otherwise the
/// first candidate is used for a new install.
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
    platform_config_dir: Option<PathBuf>,
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
    if let Some(dir) = opencode_config_dir
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
    {
        push_opencode_names(&mut out, dir);
        return out;
    }

    #[cfg(windows)]
    {
        if let Some(home) = home.clone() {
            push_opencode_names(&mut out, home.join(".config"));
        }
        if let Some(dir) = platform_config_dir {
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
    let dir = config_base.join("opencode");
    out.push(dir.join("opencode.jsonc"));
    out.push(dir.join("opencode.json"));
}

// ---------------------------------------------------------------------------
// CLI shell-out
// ---------------------------------------------------------------------------

/// Wall-clock deadline for an agent CLI shell-out (U-032). `mcp add` is a quick
/// local config edit; 30 s is generous for a cold CLI start yet bounds a hung
/// invocation (network stall, auth prompt) so install can't block.
const CLI_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// stdout cap for an agent CLI shell-out - `mcp add` prints a short
/// confirmation, so 1 MiB is plenty while bounding a runaway CLI.
const CLI_STDOUT_CAP: u64 = 1024 * 1024;

/// Is `cli` resolvable on `PATH`?
pub(crate) fn cli_on_path(cli: &str) -> bool {
    which::which(cli).is_ok()
}

/// Run `program args...`, capturing output. `Ok(())` iff it exits 0;
/// otherwise an error carrying the trimmed stderr (for `log`/report).
pub(crate) fn shell_out(program: &str, args: &[&str]) -> Result<()> {
    // US-042 (Windows): a bare `Command::new("claude")` goes through
    // `CreateProcessW`, which ignores `PATHEXT` and so cannot launch the
    // `claude.cmd` shim that npm/bun install - even though `cli_on_path`
    // (via `which::which`) resolved it, so the "preferred CLI path" was
    // entered and then died with `NotFound`. Resolve the full `.cmd`/`.exe`
    // path first; Rust std ≥1.77 wraps `.cmd`/`.bat` through `cmd.exe`
    // automatically. On Unix `execvp` honors PATH for a bare name, so the
    // original behavior is kept there.
    #[cfg(windows)]
    let resolved = which::which(program).unwrap_or_else(|_| PathBuf::from(program));
    #[cfg(windows)]
    let mut command = Command::new(resolved);
    #[cfg(not(windows))]
    let mut command = Command::new(program);
    command.args(args);

    // U-032: bound the CLI with a wall-clock deadline so a hung `claude`/`codex
    // mcp add` (network stall, auth prompt) can't block the installer.
    // run_with_timeout nulls stdin and caps stdout/stderr for us.
    let output = paneflow_process::run_with_timeout(command, CLI_DEADLINE, CLI_STDOUT_CAP)
        .map_err(|e| anyhow!("failed to run `{program}`: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    Err(anyhow!(
        "`{program} {}` exited with {}: {}",
        args.join(" "),
        output.status,
        // Some CLIs report errors on stdout; include both, trimmed.
        format!("{} {}", stderr.trim(), stdout.trim()).trim()
    ))
}

// ---------------------------------------------------------------------------
// JSON install / uninstall / status (Claude Code, Gemini, opencode)
// ---------------------------------------------------------------------------

/// Upsert `root[container][paneflow] = entry` at `path`, idempotently and
/// no-clobber. Returns `Installed` (new), `Updated` (entry changed), or
/// `AlreadyCurrent` (no-op). A present-but-invalid file is an error (never
/// overwritten).
pub(crate) fn json_install(
    path: &Path,
    container: &str,
    entry: serde_json::Value,
) -> Result<InstallOutcome> {
    io::with_config_lock(path, || {
        let mut root = merge::read_json_or_default(path)?;
        let had_prior = root.get(container).and_then(|c| c.get(ENTRY)).is_some();
        let changed = merge::merge_json_entry(&mut root, container, ENTRY, entry)?;
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

/// Remove `root[container][paneflow]` at `path`. No-op when the file or
/// entry is absent.
pub(crate) fn json_uninstall(path: &Path, container: &str) -> Result<UninstallOutcome> {
    if !path.exists() {
        return Ok(UninstallOutcome::NothingToRemove);
    }
    io::with_config_lock(path, || {
        if !path.exists() {
            return Ok(UninstallOutcome::NothingToRemove);
        }
        let mut root = merge::read_json_or_default(path)?;
        if !merge::remove_json_entry(&mut root, container, ENTRY) {
            return Ok(UninstallOutcome::NothingToRemove);
        }
        io::write_if_changed_unlocked(path, &merge::json_to_bytes(&root)?)?;
        Ok(UninstallOutcome::Removed)
    })
}

/// Read-only state of the `paneflow` JSON entry at `path`. `extract`
/// pulls the command path out of the entry (string for most agents, first
/// array element for opencode). `expected` is the current bridge path used
/// to flag staleness when it is available.
pub(crate) fn json_status(
    path: &Path,
    container: &str,
    expected: Option<&Path>,
    validate: impl Fn(&serde_json::Value, Option<&Path>) -> StatusOutcome,
) -> Result<StatusOutcome> {
    if !path.exists() {
        return Ok(StatusOutcome::NotInstalled);
    }
    let root = merge::read_json_or_default(path)?;
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

// ---------------------------------------------------------------------------
// TOML install / uninstall / status (Codex)
// ---------------------------------------------------------------------------

/// Codex's parent table for MCP servers.
pub(crate) const CODEX_TABLE: &str = "mcp_servers";

pub(crate) fn toml_install(path: &Path, command: &str) -> Result<InstallOutcome> {
    io::with_config_lock(path, || {
        let mut doc = merge::read_toml_or_default(path)?;
        let had_prior = doc.get(CODEX_TABLE).and_then(|t| t.get(ENTRY)).is_some();
        let changed = merge::upsert_toml_entry(&mut doc, CODEX_TABLE, ENTRY, command, &[])?;
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
    let enabled_ok = entry
        .get("enabled")
        .and_then(|e| e.as_bool())
        .unwrap_or(true);
    let shape_ok = args_ok && enabled_ok;
    Ok(classify_entry(
        found,
        expected,
        shape_ok,
        "Codex MCP entry must have empty args and must not be disabled",
    ))
}

// ---------------------------------------------------------------------------
// Command-path extractors
// ---------------------------------------------------------------------------

/// `command` as a plain string (Claude Code, Gemini).
pub(crate) fn string_command(entry: &serde_json::Value) -> Option<String> {
    entry.get("command")?.as_str().map(str::to_string)
}

/// `command` as an array whose first element is the binary path (opencode).
pub(crate) fn array_command(entry: &serde_json::Value) -> Option<String> {
    entry
        .get("command")?
        .as_array()?
        .first()?
        .as_str()
        .map(str::to_string)
}

pub(crate) fn json_entry_present(path: &Path, container: &str) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let root = merge::read_json_or_default(path)?;
    let Some(container_value) = root.get(container) else {
        return Ok(false);
    };
    let Some(container_object) = container_value.as_object() else {
        bail!("config key `{container}` is not an object - refusing to overwrite");
    };
    Ok(container_object.contains_key(ENTRY))
}

pub(crate) fn toml_entry_present(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let doc = merge::read_toml_or_default(path)?;
    let Some(parent) = doc.get(CODEX_TABLE) else {
        return Ok(false);
    };
    let Some(parent) = parent.as_table() else {
        bail!("`{CODEX_TABLE}` is not a TOML table - refusing to overwrite");
    };
    Ok(parent.contains_key(ENTRY))
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Compare a found command path and entry shape against the expected bridge
/// path, when that path is available.
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn validate_string_entry(entry: &serde_json::Value, expected: Option<&Path>) -> StatusOutcome {
        classify_entry(string_command(entry), expected, true, "shape mismatch")
    }

    #[test]
    fn codex_config_honors_codex_home() {
        assert_eq!(
            codex_config_from(
                Some(PathBuf::from("/home/alice")),
                Some(OsString::from("/tmp/codex-home"))
            )
            .unwrap(),
            PathBuf::from("/tmp/codex-home").join("config.toml")
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
    fn opencode_config_candidates_prefer_jsonc_in_custom_dir() {
        assert_eq!(
            opencode_configs_from(
                Some(PathBuf::from("/home/alice")),
                None,
                None,
                None,
                Some(OsString::from("/tmp/opencode-config")),
            ),
            vec![
                PathBuf::from("/tmp/opencode-config")
                    .join("opencode")
                    .join("opencode.jsonc"),
                PathBuf::from("/tmp/opencode-config")
                    .join("opencode")
                    .join("opencode.json"),
            ]
        );
    }

    #[test]
    fn json_install_then_idempotent() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("settings.json");
        let entry = json!({ "command": "/p", "args": [] });

        assert_eq!(
            json_install(&p, "mcpServers", entry.clone()).unwrap(),
            InstallOutcome::Installed
        );
        // Re-run with identical entry → no-op.
        assert_eq!(
            json_install(&p, "mcpServers", entry).unwrap(),
            InstallOutcome::AlreadyCurrent
        );
        // Different path → Updated.
        assert_eq!(
            json_install(&p, "mcpServers", json!({ "command": "/q", "args": [] })).unwrap(),
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

        json_install(&p, "mcpServers", json!({ "command": "/p" })).unwrap();
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
        assert!(json_install(&p, "mcpServers", json!({})).is_err());
        // The invalid file was NOT overwritten.
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
            json_uninstall(&p, "mcpServers").unwrap(),
            UninstallOutcome::Removed
        );
        let after: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert!(after["mcpServers"].get("paneflow").is_none());
        assert_eq!(after["mcpServers"]["other"]["command"], json!("x"));
        // Second uninstall → nothing to remove.
        assert_eq!(
            json_uninstall(&p, "mcpServers").unwrap(),
            UninstallOutcome::NothingToRemove
        );
    }

    #[test]
    fn json_uninstall_absent_file_does_not_create_parent_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("missing-parent").join("settings.json");

        assert_eq!(
            json_uninstall(&p, "mcpServers").unwrap(),
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
            json_status(&p, "mcpServers", None, validate_string_entry).unwrap(),
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
        // Idempotent.
        assert_eq!(
            toml_install(&p, "/p").unwrap(),
            InstallOutcome::AlreadyCurrent
        );
        // Updated path.
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
