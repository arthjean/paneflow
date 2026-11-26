use std::io::Write;

use crate::legacy_mcp::PurgeReport;

pub const MCP_REMOVED: &str = "\
paneflow mcp: the Paneflow MCP bridge was removed.
Agents now drive Paneflow through the `paneflow` CLI and the paneflow-conductor
skill, which `paneflow integrations install <agent>` installs with the hooks.
Run `paneflow mcp uninstall` to remove the MCP entries an older Paneflow wrote.";

pub const NOTHING_TO_PURGE: &str = "no Paneflow MCP entry found";

#[must_use]
pub fn run_cli(args: &[String]) -> i32 {
    run_with(
        args,
        crate::integrations::purge_default_home_legacy_mcp,
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    )
}

pub(crate) fn run_with(
    args: &[String],
    purge: impl FnOnce() -> Result<PurgeReport, String>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    if !matches!(args, [only] if only == "uninstall") {
        let _ = writeln!(err, "{MCP_REMOVED}");
        return 1;
    }
    let report = match purge() {
        Ok(report) => report,
        Err(error) => {
            let _ = writeln!(err, "paneflow mcp uninstall: {error}");
            return 1;
        }
    };
    for warning in &report.warnings {
        let _ = writeln!(err, "warning: {warning}");
    }
    let removed = report.lines();
    if removed.is_empty() {
        let _ = writeln!(out, "{NOTHING_TO_PURGE}");
    }
    for line in removed {
        let _ = writeln!(out, "{line}");
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::legacy_mcp::{LegacyAgent, RemovedEntry};
    use std::path::PathBuf;

    fn run(words: &[&str], purge: Result<PurgeReport, String>) -> (i32, String, String) {
        let args: Vec<String> = words.iter().map(|word| word.to_string()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut purged = false;
        let code = run_with(
            &args,
            || {
                purged = true;
                purge
            },
            &mut out,
            &mut err,
        );
        assert_eq!(purged, words == ["uninstall"], "{words:?}");
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn every_verb_but_uninstall_names_the_replacement_and_exits_1() {
        for words in [
            &["install"][..],
            &["status"],
            &["install", "--force"],
            &["uninstall", "--force"],
            &[],
            &["--help"],
        ] {
            let (code, out, err) = run(words, Ok(PurgeReport::default()));
            assert_eq!(code, 1, "{words:?}");
            assert!(out.is_empty(), "{words:?}: {out}");
            assert!(err.contains("bridge was removed"), "{err}");
            assert!(
                err.contains("paneflow integrations install <agent>"),
                "{err}"
            );
            assert!(err.contains("paneflow-conductor"), "{err}");
        }
    }

    #[test]
    fn uninstall_prints_each_removed_entry_with_its_file_and_exits_0() {
        let file = PathBuf::from("/home/alice/.claude.json");
        let report = PurgeReport {
            removed: vec![
                RemovedEntry {
                    agent: LegacyAgent::ClaudeCode,
                    file: file.clone(),
                    key: "paneflow".to_string(),
                },
                RemovedEntry {
                    agent: LegacyAgent::ClaudeCode,
                    file: file.clone(),
                    key: "paneflow-dev".to_string(),
                },
            ],
            warnings: vec!["left /home/alice/.fx/mcp.json untouched".to_string()],
        };
        let (code, out, err) = run(&["uninstall"], Ok(report));
        assert_eq!(code, 0);
        assert_eq!(out.lines().count(), 2, "{out}");
        assert!(out
            .lines()
            .all(|line| line.contains(&file.display().to_string())));
        assert!(out.contains("`paneflow-dev`"), "{out}");
        assert!(
            err.contains("left /home/alice/.fx/mcp.json untouched"),
            "{err}"
        );
    }

    #[test]
    fn uninstall_with_nothing_to_remove_says_so_and_exits_0() {
        let (code, out, _) = run(&["uninstall"], Ok(PurgeReport::default()));
        assert_eq!(code, 0);
        assert_eq!(out.trim_end(), NOTHING_TO_PURGE);
    }

    #[test]
    fn an_unresolvable_home_fails_the_uninstall() {
        let (code, _, err) = run(&["uninstall"], Err("cannot resolve PANEFLOW_HOME".into()));
        assert_eq!(code, 1);
        assert!(err.contains("cannot resolve PANEFLOW_HOME"), "{err}");
    }
}
