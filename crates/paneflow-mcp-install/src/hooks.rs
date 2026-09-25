use std::io::Write;

use crate::integrations::{self, IntegrationBinaries, IntegrationState, IntegrationStatus};

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
pub fn run_hooks_cli(args: &[String], binaries: Option<IntegrationBinaries>) -> i32 {
    run_hooks_with(
        args,
        binaries.as_ref(),
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    )
}

pub(crate) fn run_hooks_with(
    args: &[String],
    binaries: Option<&IntegrationBinaries>,
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
        HooksCommand::Setup => run_setup(binaries, out, err),
        HooksCommand::Uninstall => run_uninstall(out, err),
        HooksCommand::Status => run_status(out),
    }
}

fn installable_integrations() -> impl Iterator<Item = IntegrationStatus> {
    integrations::list_integrations()
        .into_iter()
        .filter(|status| {
            matches!(
                status.state,
                IntegrationState::Installed | IntegrationState::NotInstalled
            )
        })
}

fn run_setup(
    binaries: Option<&IntegrationBinaries>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let Some(binaries) = binaries else {
        let _ = writeln!(
            err,
            "hooks: the embedded integration binaries are unavailable (data dir unresolvable); cannot install"
        );
        return 1;
    };
    let mut code = 0;
    for status in installable_integrations() {
        if !integrations::runtime_detected(status.slug) {
            let _ = writeln!(out, "{}: not detected (skipped)", status.slug);
            continue;
        }
        match integrations::install_integration(status.slug, binaries) {
            Ok(installed) => {
                let _ = writeln!(out, "{}: hooks installed", installed.slug);
                if let Some(step) = installed.post_install_step {
                    let _ = writeln!(out, "{step}");
                }
            }
            Err(error) => {
                let _ = writeln!(err, "{}: error: {error}", status.slug);
                code = 1;
            }
        }
    }
    code
}

fn run_uninstall(out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let mut code = 0;
    for status in installable_integrations() {
        if status.state != IntegrationState::Installed
            && !integrations::has_owned_hooks(status.slug)
        {
            let _ = writeln!(out, "{}: no Paneflow hooks present", status.slug);
            continue;
        }
        match integrations::remove_integration(status.slug) {
            Ok(_) => {
                let _ = writeln!(out, "{}: hooks removed", status.slug);
            }
            Err(error) => {
                let _ = writeln!(err, "{}: error: {error}", status.slug);
                code = 1;
            }
        }
    }
    code
}

fn run_status(out: &mut dyn Write) -> i32 {
    for status in integrations::list_integrations() {
        let _ = writeln!(
            out,
            "{}: {}",
            status.slug,
            integrations::state_label(status.state)
        );
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn setup_without_binaries_errors() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_hooks_with(&["setup".to_string()], None, &mut out, &mut err);
        assert_eq!(code, 1);
        assert!(String::from_utf8_lossy(&err).contains("unavailable"));
    }
}
