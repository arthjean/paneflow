use std::io::Write;
use std::path::{Path, PathBuf};

use crate::agents::{self, AgentConfigWriter};
use crate::api::{self, InstallKind, StatusKind, UninstallKind};
use crate::integrations::{InstallMode, FORCE_FLAG};

const USAGE: &str = "\
paneflow mcp - register the Paneflow MCP bridge with your CLI agents

Usage:
  paneflow mcp install      Register the bridge with every detected agent
                            (--force takes over entries another Paneflow
                            home owns; a debug build needs it for any
                            existing entry)
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

#[must_use]
pub fn run_cli(args: &[String], bridge_path: Option<PathBuf>) -> i32 {
    let writers = agents::default_writers();
    run_with(
        args,
        bridge_path.as_deref(),
        &writers,
        &mut std::io::stdout(),
        &mut std::io::stderr(),
        cfg!(debug_assertions),
    )
}

pub(crate) fn run_with(
    args: &[String],
    bridge_path: Option<&Path>,
    writers: &[Box<dyn AgentConfigWriter>],
    out: &mut dyn Write,
    err: &mut dyn Write,
    debug_build: bool,
) -> i32 {
    let Some(command) = Command::parse(args.first().map(String::as_str)) else {
        let _ = writeln!(err, "{USAGE}");
        return 2;
    };
    let force = command == Command::Install && args.get(1).map(String::as_str) == Some(FORCE_FLAG);
    if args.len() != 1 + usize::from(force) {
        let _ = writeln!(err, "unexpected argument after `{}`\n\n{USAGE}", args[0]);
        return 2;
    }
    let mode = InstallMode { force, debug_build };

    match command {
        Command::Install => run_install(bridge_path, writers, mode, out, err),
        Command::Uninstall => run_uninstall(writers, out),
        Command::Status => run_status(bridge_path, writers, out),
    }
}

fn run_install(
    bridge_path: Option<&Path>,
    writers: &[Box<dyn AgentConfigWriter>],
    mode: InstallMode,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let results = match api::install_with(bridge_path, writers, mode) {
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
            StatusKind::DisabledByUser { path } => {
                let _ = writeln!(
                    out,
                    "{}: installed ({path}) but disabled in the agent config by the user",
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
        run_as(args, bridge, writers, false)
    }

    fn run_as(
        args: &[&str],
        bridge: Option<&Path>,
        writers: &[Box<dyn AgentConfigWriter>],
        debug_build: bool,
    ) -> (i32, String, String) {
        let args: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_with(&args, bridge, writers, &mut out, &mut err, debug_build);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn staged_home(root: &Path, name: &str) -> PathBuf {
        let bin = root.join(name).join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let bridge = bin.join(format!(
            "paneflow-mcp{}",
            if cfg!(windows) { ".exe" } else { "" }
        ));
        std::fs::write(&bridge, b"bridge").unwrap();
        bridge
    }

    fn owned_by(found: &Path, expected: &Path) -> Mock {
        Mock::present("claude-code").with_status(Ok(crate::agents::StatusOutcome::StalePath {
            found: found.display().to_string(),
            expected: expected.display().to_string(),
        }))
    }

    #[test]
    fn an_entry_owned_by_another_live_home_is_kept_without_force_naming_both_homes() {
        let dir = tempfile::TempDir::new().unwrap();
        let release = staged_home(dir.path(), "release-home");
        let dev = staged_home(dir.path(), "dev-home");

        let writers = vec![boxed(owned_by(&release, &dev))];
        let (code, out, _) = run_as(&["install"], Some(&dev), &writers, false);
        assert_eq!(code, 1, "{out}");
        assert!(
            out.contains(&dir.path().join("release-home").display().to_string()),
            "{out}"
        );
        assert!(
            out.contains(&dir.path().join("dev-home").display().to_string()),
            "{out}"
        );
        assert!(out.contains("--force"), "{out}");
        assert!(
            matches!(
                writers[0].install(Path::new("/x")),
                Ok(crate::agents::InstallOutcome::Installed)
            ),
            "the refused run never reached the writer"
        );

        let writers = vec![boxed(owned_by(&release, &dev))];
        let (code, out, err) = run_as(&["install", "--force"], Some(&dev), &writers, false);
        assert_eq!(code, 0, "{err}");
        assert!(out.contains("claude-code: installed"), "{out}");

        let (code, _, err) = run_as(&["status", "--force"], Some(&dev), &writers, false);
        assert_eq!(code, 2, "--force only belongs to install: {err}");
    }

    #[test]
    fn a_debug_build_overwrites_no_existing_entry_without_force_but_installs_a_fresh_one() {
        let dir = tempfile::TempDir::new().unwrap();
        let dev = staged_home(dir.path(), "dev-home");
        let gone = dir
            .path()
            .join("deleted-home")
            .join("bin")
            .join("paneflow-mcp");

        let writers = vec![boxed(owned_by(&gone, &dev))];
        let (code, out, _) = run_as(&["install"], Some(&dev), &writers, true);
        assert_eq!(code, 1, "{out}");
        assert!(
            out.contains(&dir.path().join("deleted-home").display().to_string()),
            "{out}"
        );

        let writers = vec![boxed(owned_by(&gone, &dev))];
        let (code, out, _) = run_as(&["install"], Some(&dev), &writers, false);
        assert_eq!(
            code, 0,
            "a release build replaces an entry whose home is gone: {out}"
        );

        let writers = vec![boxed(
            Mock::present("claude-code")
                .with_status(Ok(crate::agents::StatusOutcome::NotInstalled)),
        )];
        let (code, out, err) = run_as(&["install"], Some(&dev), &writers, true);
        assert_eq!(code, 0, "{err}");
        assert!(out.contains("claude-code: installed"), "{out}");
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
