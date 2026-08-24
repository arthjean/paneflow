//! Persistent user-scope agent notification hooks.
//!
//! Claude hook shape, command rendering, detection, and reconciliation live in
//! `paneflow-agent-config`, shared with the project-local shim. This module owns
//! only user-scope path resolution, safe persistence, and CLI presentation.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use paneflow_agent_config::claude_hooks::{self, HookStatus};

use crate::agents::{InstallOutcome, StatusOutcome, UninstallOutcome};
use crate::{io, merge};

fn claude_settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".claude").join("settings.json"))
}

fn claude_detected() -> bool {
    which::which("claude").is_ok()
        || dirs::home_dir()
            .map(|home| home.join(".claude").exists())
            .unwrap_or(false)
}

fn install(hook_path: &Path) -> Result<(PathBuf, InstallOutcome)> {
    let settings =
        claude_settings_path().ok_or_else(|| anyhow!("cannot resolve ~/.claude/settings.json"))?;
    let outcome = install_at(&settings, hook_path)?;
    Ok((settings, outcome))
}

fn install_at(settings: &Path, hook_path: &Path) -> Result<InstallOutcome> {
    io::with_config_lock(settings, || {
        let mut root = merge::read_json_or_default(settings)?;
        let reconciled = claude_hooks::reconcile_hooks(&mut root, |event| {
            claude_hooks::render_hook_command(hook_path, event)
        })?;
        if !reconciled.changed {
            return Ok(InstallOutcome::AlreadyCurrent);
        }
        io::write_if_changed_unlocked(settings, &merge::json_to_bytes(&root)?)?;
        Ok(if reconciled.had_prior {
            InstallOutcome::Updated
        } else {
            InstallOutcome::Installed
        })
    })
}

fn uninstall() -> Result<UninstallOutcome> {
    let settings =
        claude_settings_path().ok_or_else(|| anyhow!("cannot resolve ~/.claude/settings.json"))?;
    uninstall_at(&settings)
}

fn uninstall_at(settings: &Path) -> Result<UninstallOutcome> {
    if !settings.exists() {
        return Ok(UninstallOutcome::NothingToRemove);
    }
    io::with_config_lock(settings, || {
        if !settings.exists() {
            return Ok(UninstallOutcome::NothingToRemove);
        }
        let mut root = merge::read_json_or_default(settings)?;
        if !claude_hooks::remove_hooks(&mut root)? {
            return Ok(UninstallOutcome::NothingToRemove);
        }
        io::write_if_changed_unlocked(settings, &merge::json_to_bytes(&root)?)?;
        Ok(UninstallOutcome::Removed)
    })
}

fn status(expected_hook_path: Option<&Path>) -> Result<StatusOutcome> {
    let settings =
        claude_settings_path().ok_or_else(|| anyhow!("cannot resolve ~/.claude/settings.json"))?;
    status_at(&settings, expected_hook_path)
}

fn status_at(settings: &Path, expected_hook_path: Option<&Path>) -> Result<StatusOutcome> {
    if !settings.exists() {
        return Ok(StatusOutcome::NotInstalled);
    }
    let root = merge::read_json_or_default(settings)?;
    Ok(map_hook_status(claude_hooks::inspect_hooks(
        &root,
        expected_hook_path,
    )))
}

fn map_hook_status(status: HookStatus) -> StatusOutcome {
    match status {
        HookStatus::NotInstalled => StatusOutcome::NotInstalled,
        HookStatus::Installed { path } => StatusOutcome::Installed { path },
        HookStatus::Stale { found, expected } => StatusOutcome::StalePath { found, expected },
        HookStatus::NeedsRepair { path, reason } => StatusOutcome::NeedsRepair { path, reason },
    }
}

const HOOKS_USAGE: &str = "\
paneflow hooks - register the Paneflow agent-notification hooks with your agents

Usage:
  paneflow hooks setup       Install persistent hooks for every supported agent
  paneflow hooks uninstall   Remove the Paneflow hooks
  paneflow hooks status      Report the hook installation state per agent";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HooksCommand {
    Setup,
    Uninstall,
    Status,
}

impl HooksCommand {
    fn parse(argument: Option<&str>) -> Option<Self> {
        match argument {
            Some("setup") => Some(Self::Setup),
            Some("uninstall") => Some(Self::Uninstall),
            Some("status") => Some(Self::Status),
            _ => None,
        }
    }
}

#[must_use]
pub fn run_hooks_cli(args: &[String], hook_path: Option<PathBuf>) -> i32 {
    run_hooks_with(
        args,
        hook_path.as_deref(),
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    )
}

pub(crate) fn run_hooks_with(
    args: &[String],
    hook_path: Option<&Path>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let Some(command) = HooksCommand::parse(args.first().map(String::as_str)) else {
        let _ = writeln!(err, "{HOOKS_USAGE}");
        return 2;
    };
    if args.len() != 1 {
        let _ = writeln!(
            err,
            "unexpected argument after `{}`\n\n{HOOKS_USAGE}",
            args[0]
        );
        return 2;
    }

    match command {
        HooksCommand::Setup => run_setup(hook_path, out, err),
        HooksCommand::Uninstall => run_uninstall(out, err),
        HooksCommand::Status => run_status(hook_path, out, err),
    }
}

fn run_setup(hook_path: Option<&Path>, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let Some(hook_path) = hook_path else {
        let _ = writeln!(
            err,
            "hooks: the paneflow-ai-hook binary is unavailable (data dir unresolvable); cannot install"
        );
        return 1;
    };
    let code = if !claude_detected() {
        let _ = writeln!(out, "claude-code: not detected (skipped)");
        0
    } else {
        match install(hook_path) {
            Ok((path, outcome)) => {
                let verb = match outcome {
                    InstallOutcome::Installed => "installed",
                    InstallOutcome::Updated => "updated",
                    InstallOutcome::AlreadyCurrent => "already current",
                };
                let _ = writeln!(out, "claude-code: hooks {verb} ({})", path.display());
                0
            }
            Err(error) => {
                let _ = writeln!(err, "claude-code: error: {error:#}");
                1
            }
        }
    };
    report_other_agents(out);
    code
}

fn run_uninstall(out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    match uninstall() {
        Ok(UninstallOutcome::Removed) => {
            let _ = writeln!(out, "claude-code: hooks removed");
            0
        }
        Ok(UninstallOutcome::NothingToRemove) => {
            let _ = writeln!(out, "claude-code: no Paneflow hooks present");
            0
        }
        Err(error) => {
            let _ = writeln!(err, "claude-code: error: {error:#}");
            1
        }
    }
}

fn run_status(expected_hook_path: Option<&Path>, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let code = match status(expected_hook_path) {
        Ok(StatusOutcome::Installed { path }) => {
            let _ = writeln!(out, "claude-code: installed ({path})");
            0
        }
        Ok(StatusOutcome::StalePath { found, expected }) => {
            let _ = writeln!(
                out,
                "claude-code: stale (found {found}, expected {expected})"
            );
            0
        }
        Ok(StatusOutcome::NeedsRepair { path, reason }) => {
            let suffix = path
                .as_deref()
                .map(|path| format!(" at {path}"))
                .unwrap_or_default();
            let _ = writeln!(out, "claude-code: needs repair{suffix} ({reason})");
            0
        }
        Ok(StatusOutcome::NotInstalled) => {
            let _ = writeln!(out, "claude-code: not installed");
            0
        }
        Err(error) => {
            let _ = writeln!(err, "claude-code: error: {error:#}");
            1
        }
    };
    report_other_agents(out);
    code
}

fn report_other_agents(out: &mut dyn Write) {
    if which::which("codex").is_ok() {
        let _ = writeln!(
            out,
            "codex: hooks injected per-launch by the shim (no user-scope install)"
        );
    }
    if which::which("gemini").is_ok() {
        let _ = writeln!(out, "gemini: no notification-hook mechanism (unsupported)");
    }
    if which::which("opencode").is_ok() {
        let _ = writeln!(
            out,
            "opencode: no notification-hook mechanism (unsupported)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn read(path: &Path) -> Value {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn install_status_uninstall_round_trip() {
        let dir = tempfile::TempDir::new().unwrap();
        let settings = dir.path().join("settings.json");
        let hook = Path::new("/opt/Pane Flow/paneflow-ai-hook");
        std::fs::write(
            &settings,
            serde_json::to_vec(&json!({
                "theme": "dark",
                "hooks": {
                    "Stop": [{ "hooks": [{ "type": "command", "command": "my-hook" }] }]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        assert_eq!(
            install_at(&settings, hook).unwrap(),
            InstallOutcome::Installed
        );
        assert_eq!(
            install_at(&settings, hook).unwrap(),
            InstallOutcome::AlreadyCurrent
        );
        assert_eq!(
            status_at(&settings, Some(hook)).unwrap(),
            StatusOutcome::Installed {
                path: claude_hooks::display_hook_program(hook),
            }
        );
        assert_eq!(read(&settings)["theme"], json!("dark"));
        assert_eq!(uninstall_at(&settings).unwrap(), UninstallOutcome::Removed);
        assert_eq!(
            read(&settings)["hooks"]["Stop"].as_array().unwrap().len(),
            1
        );
    }

    #[test]
    fn install_refuses_invalid_hook_boundaries_without_clobbering() {
        let dir = tempfile::TempDir::new().unwrap();
        let settings = dir.path().join("settings.json");
        let original = br#"{"hooks":{"Stop":"broken"}}"#;
        std::fs::write(&settings, original).unwrap();

        assert!(install_at(&settings, Path::new("/bin/paneflow-ai-hook")).is_err());
        assert_eq!(std::fs::read(&settings).unwrap(), original);
    }

    #[test]
    fn status_rejects_partial_hook_set() {
        let dir = tempfile::TempDir::new().unwrap();
        let settings = dir.path().join("settings.json");
        let hook = Path::new("/bin/paneflow-ai-hook");
        install_at(&settings, hook).unwrap();
        let mut root = read(&settings);
        root["hooks"].as_object_mut().unwrap().remove("Stop");
        std::fs::write(&settings, serde_json::to_vec(&root).unwrap()).unwrap();

        assert!(matches!(
            status_at(&settings, Some(hook)).unwrap(),
            StatusOutcome::NeedsRepair { .. }
        ));
    }

    #[test]
    fn cli_rejects_bad_or_trailing_arguments() {
        for args in [
            vec!["bogus".to_string()],
            vec!["status".to_string(), "extra".to_string()],
        ] {
            let mut out = Vec::new();
            let mut err = Vec::new();
            assert_eq!(run_hooks_with(&args, None, &mut out, &mut err), 2);
            assert!(String::from_utf8_lossy(&err).contains("Usage"));
        }
    }

    #[test]
    fn setup_without_hook_path_errors() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_hooks_with(&["setup".to_string()], None, &mut out, &mut err);
        assert_eq!(code, 1);
        assert!(String::from_utf8_lossy(&err).contains("unavailable"));
    }
}
