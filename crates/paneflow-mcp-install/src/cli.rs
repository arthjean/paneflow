//! `paneflow mcp <subcommand>` CLI front-end (EP-002 US-005).
//!
//! [`run_cli`] is the single entry point `main.rs` calls before the GUI
//! starts. It parses the subcommand, delegates orchestration to
//! [`crate::api`], formats a scriptable per-agent report, and returns a
//! process exit code:
//!
//! - `0` - success (including "no agents detected", which writes nothing).
//! - `1` - install refused (bridge binary missing) or ≥1 agent errored.
//! - `2` - usage error (missing / unknown subcommand).
//!
//! Output is line-oriented `<agent-id>: <message>` so the command is
//! scriptable; diagnostics go to stderr, the report to stdout. The GUI
//! (EP-004) bypasses this formatter and consumes [`crate::api`] directly.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::agents::{self, AgentConfigWriter};
use crate::api::{self, InstallKind, StatusKind, UninstallKind};

const USAGE: &str = "\
paneflow mcp - register the Paneflow MCP bridge with your CLI agents

Usage:
  paneflow mcp install      Register the bridge with every detected agent
  paneflow mcp uninstall    Remove the Paneflow entry from every agent
  paneflow mcp status       Report the bridge registration state per agent";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Install,
    Uninstall,
    Status,
}

impl Command {
    fn parse(arg: Option<&str>) -> Option<Self> {
        match arg {
            Some("install") => Some(Self::Install),
            Some("uninstall") => Some(Self::Uninstall),
            Some("status") => Some(Self::Status),
            _ => None,
        }
    }
}

/// Entry point. `args` is everything *after* `paneflow mcp` (i.e. the
/// subcommand and its flags). `bridge_path` is the stable bridge location
/// resolved by the caller (`runtime_paths::bridge_binary_path()`), or
/// `None` when `data_dir()` is unresolvable.
#[must_use]
pub fn run_cli(args: &[String], bridge_path: Option<PathBuf>) -> i32 {
    let writers = agents::default_writers();
    run_with(
        args,
        bridge_path.as_deref(),
        &writers,
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    )
}

/// Testable core: output sinks and the writer set are injected so unit
/// tests drive it with mock agents and capture buffers (no real configs,
/// no real PATH).
pub(crate) fn run_with(
    args: &[String],
    bridge_path: Option<&Path>,
    writers: &[Box<dyn AgentConfigWriter>],
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let Some(command) = Command::parse(args.first().map(String::as_str)) else {
        let _ = writeln!(err, "{USAGE}");
        return 2;
    };
    if args.len() != 1 {
        let _ = writeln!(err, "unexpected argument after `{}`\n\n{USAGE}", args[0]);
        return 2;
    }

    match command {
        Command::Install => run_install(bridge_path, writers, out, err),
        Command::Uninstall => run_uninstall(writers, out),
        Command::Status => run_status(bridge_path, writers, out),
    }
}

fn run_install(
    bridge_path: Option<&Path>,
    writers: &[Box<dyn AgentConfigWriter>],
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let results = match api::install_with(bridge_path, writers) {
        Ok(r) => r,
        Err(msg) => {
            let _ = writeln!(err, "error: {msg}");
            return 1;
        }
    };

    if results.is_empty() {
        let _ = writeln!(
            out,
            "No supported MCP agents detected - nothing to install."
        );
        return 0;
    }

    let path_disp = bridge_path
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let mut had_error = false;
    for r in &results {
        match &r.kind {
            InstallKind::Installed => {
                let _ = writeln!(out, "{}: installed ({path_disp})", r.id);
            }
            InstallKind::Updated => {
                let _ = writeln!(out, "{}: updated ({path_disp})", r.id);
            }
            InstallKind::AlreadyCurrent => {
                let _ = writeln!(out, "{}: already up to date", r.id);
            }
            InstallKind::SkippedAbsent => {
                let _ = writeln!(out, "{}: skipped (not detected)", r.id);
            }
            InstallKind::Error(e) => {
                had_error = true;
                let _ = writeln!(out, "{}: error - {e}", r.id);
            }
        }
    }
    if results
        .iter()
        .all(|r| matches!(r.kind, InstallKind::SkippedAbsent))
    {
        let _ = writeln!(
            out,
            "No supported MCP agents detected - nothing to install."
        );
    }
    i32::from(had_error)
}

fn run_uninstall(writers: &[Box<dyn AgentConfigWriter>], out: &mut dyn Write) -> i32 {
    let results = api::uninstall_with(writers);
    if results.is_empty() {
        let _ = writeln!(out, "No supported MCP agents detected.");
        return 0;
    }
    let mut had_error = false;
    for r in &results {
        match &r.kind {
            UninstallKind::Removed => {
                let _ = writeln!(out, "{}: removed", r.id);
            }
            UninstallKind::NothingToRemove => {
                let _ = writeln!(out, "{}: no Paneflow entry (nothing to remove)", r.id);
            }
            UninstallKind::NotDetected => {
                let _ = writeln!(out, "{}: not detected (nothing to remove)", r.id);
            }
            UninstallKind::Error(e) => {
                had_error = true;
                let _ = writeln!(out, "{}: error - {e}", r.id);
            }
        }
    }
    i32::from(had_error)
}

fn run_status(
    bridge_path: Option<&Path>,
    writers: &[Box<dyn AgentConfigWriter>],
    out: &mut dyn Write,
) -> i32 {
    let results = api::status_with(bridge_path, writers);
    if results.is_empty() {
        let _ = writeln!(out, "No supported MCP agents detected.");
        return 0;
    }
    let mut had_error = false;
    for r in &results {
        match &r.kind {
            StatusKind::NotDetected => {
                let _ = writeln!(out, "{}: not detected", r.id);
            }
            StatusKind::Installed { path } => {
                let _ = writeln!(out, "{}: installed ({path})", r.id);
            }
            StatusKind::Stale { found, expected } => {
                let _ = writeln!(
                    out,
                    "{}: stale path (config points at {found}, expected {expected}) - re-run `paneflow mcp install`",
                    r.id
                );
            }
            StatusKind::NeedsRepair { path, reason } => {
                let suffix = path
                    .as_deref()
                    .map(|p| format!(" at {p}"))
                    .unwrap_or_default();
                let _ = writeln!(
                    out,
                    "{}: needs repair{suffix} ({reason}) - re-run `paneflow mcp install`",
                    r.id
                );
            }
            StatusKind::NotInstalled => {
                let _ = writeln!(out, "{}: detected but not installed", r.id);
            }
            StatusKind::Error(e) => {
                had_error = true;
                let _ = writeln!(out, "{}: error - {e}", r.id);
            }
        }
    }
    i32::from(had_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::testutil::Mock;
    use anyhow::anyhow;

    fn boxed(m: Mock) -> Box<dyn AgentConfigWriter> {
        Box::new(m)
    }

    fn run(
        args: &[&str],
        bridge: Option<&Path>,
        writers: &[Box<dyn AgentConfigWriter>],
    ) -> (i32, String, String) {
        let args: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_with(&args, bridge, writers, &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn missing_subcommand_is_usage_error() {
        let (code, _out, err) = run(&[], None, &[]);
        assert_eq!(code, 2);
        assert!(err.contains("Usage:"));
    }

    #[test]
    fn unknown_subcommand_is_usage_error() {
        let (code, _out, err) = run(&["bogus"], None, &[]);
        assert_eq!(code, 2);
        assert!(err.contains("Usage:"));
    }

    #[test]
    fn trailing_args_are_usage_error() {
        let (code, out, err) = run(&["install", "--help"], None, &[]);
        assert_eq!(code, 2);
        assert!(out.is_empty());
        assert!(err.contains("unexpected argument"));
    }

    #[test]
    fn install_refuses_when_bridge_missing() {
        let writers = vec![boxed(Mock::present("claude"))];
        let missing = Path::new("/definitely/not/here/paneflow-mcp");
        let (code, out, err) = run(&["install"], Some(missing), &writers);
        assert_eq!(code, 1, "must refuse with non-zero exit");
        assert!(err.contains("missing"), "stderr explains the refusal");
        assert!(out.is_empty(), "no agent lines written when bridge missing");
    }

    #[test]
    fn install_refuses_when_data_dir_unresolved() {
        let writers = vec![boxed(Mock::present("claude"))];
        let (code, _out, err) = run(&["install"], None, &writers);
        assert_eq!(code, 1);
        assert!(err.contains("data directory"));
    }

    #[test]
    fn install_no_agents_is_success() {
        let (code, out, _err) = run(&["install"], None, &[]);
        assert_eq!(code, 0);
        assert!(out.contains("No supported MCP agents"));
    }

    #[test]
    fn install_writes_present_skips_absent() {
        let dir = tempfile::TempDir::new().unwrap();
        let bridge = dir.path().join("paneflow-mcp");
        std::fs::write(&bridge, b"bin").unwrap();
        let writers = vec![boxed(Mock::present("claude")), boxed(Mock::absent("codex"))];
        let (code, out, _err) = run(&["install"], Some(&bridge), &writers);
        assert_eq!(code, 0);
        assert!(out.contains("claude: installed"));
        assert!(out.contains("codex: skipped (not detected)"));
    }

    #[test]
    fn install_reports_per_agent_error_without_aborting_others() {
        let dir = tempfile::TempDir::new().unwrap();
        let bridge = dir.path().join("paneflow-mcp");
        std::fs::write(&bridge, b"bin").unwrap();
        let writers = vec![
            boxed(Mock::present("claude").with_install(Err(anyhow!("boom")))),
            boxed(Mock::present("codex")),
        ];
        let (code, out, _err) = run(&["install"], Some(&bridge), &writers);
        assert_eq!(code, 1, "an agent error yields non-zero exit");
        assert!(out.contains("claude: error"));
        assert!(out.contains("codex: installed"), "other agents still run");
    }

    #[test]
    fn uninstall_skips_absent_and_removes_present() {
        let writers = vec![boxed(Mock::present("claude")), boxed(Mock::absent("codex"))];
        let (code, out, _err) = run(&["uninstall"], None, &writers);
        assert_eq!(code, 0);
        assert!(out.contains("claude: removed"));
        assert!(out.contains("codex: not detected"));
    }

    #[test]
    fn status_is_read_only_and_reports_states() {
        let writers = vec![boxed(Mock::present("claude")), boxed(Mock::absent("codex"))];
        let (code, out, _err) = run(&["status"], Some(Path::new("/p")), &writers);
        assert_eq!(code, 0);
        assert!(out.contains("claude: installed (/p)"));
        assert!(out.contains("codex: not detected"));
    }
}
