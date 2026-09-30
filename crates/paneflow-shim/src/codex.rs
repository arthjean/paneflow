use std::ffi::{OsStr, OsString};

const NO_DAEMON: &str = "--no-daemon";

const SESSION_SUBCOMMANDS: &[&str] = &["resume", "fork"];

const OTHER_SUBCOMMANDS: &[&str] = &[
    "agents",
    "exec",
    "e",
    "review",
    "login",
    "logout",
    "mcp",
    "mcp-server",
    "plugin",
    "app-server",
    "remote-control",
    "completion",
    "update",
    "doctor",
    "sandbox",
    "debug",
    "apply",
    "a",
    "queue",
    "archive",
    "delete",
    "migrate-rollouts",
    "unarchive",
    "cloud",
    "exec-server",
    "features",
    "help",
];

const VALUE_OPTIONS: &[&str] = &[
    "-c",
    "--config",
    "--enable",
    "--disable",
    "--remote",
    "--remote-auth-token-env",
    "-m",
    "--model",
    "--local-provider",
    "-p",
    "--profile",
    "-s",
    "--sandbox",
    "-a",
    "--ask-for-approval",
    "-C",
    "--cd",
    "--add-dir",
];

const MULTI_VALUE_OPTIONS: &[&str] = &["-i", "--image"];

const NON_SESSION_FLAGS: &[&str] = &["-h", "--help", "-V", "--version"];

#[derive(Debug, PartialEq, Eq)]
enum Invocation {
    Interactive,
    Session(usize),
    Other,
}

pub(crate) fn pane_session_args(args: Vec<OsString>, in_pane: bool) -> Vec<OsString> {
    if !in_pane {
        return args;
    }
    let insert_at = match classify(&args) {
        Invocation::Interactive => 0,
        Invocation::Session(index) => index + 1,
        Invocation::Other => return args,
    };
    let mut args = args;
    args.insert(insert_at, OsString::from(NO_DAEMON));
    args
}

fn classify(args: &[OsString]) -> Invocation {
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        let Some(text) = arg.to_str() else {
            return Invocation::Interactive;
        };
        if text == "--" {
            return Invocation::Interactive;
        }
        if text == NO_DAEMON || text.starts_with("--remote") || NON_SESSION_FLAGS.contains(&text) {
            return Invocation::Other;
        }
        if MULTI_VALUE_OPTIONS.contains(&text) {
            index += 1;
            while args.get(index).is_some_and(|value| !is_flag(value)) {
                index += 1;
            }
            continue;
        }
        if VALUE_OPTIONS.contains(&text) {
            index += 2;
            continue;
        }
        if is_flag(arg) {
            index += 1;
            continue;
        }
        if SESSION_SUBCOMMANDS.contains(&text) {
            return if session_rest_is_plain(&args[index + 1..]) {
                Invocation::Session(index)
            } else {
                Invocation::Other
            };
        }
        if OTHER_SUBCOMMANDS.contains(&text) {
            return Invocation::Other;
        }
        return Invocation::Interactive;
    }
    Invocation::Interactive
}

fn session_rest_is_plain(rest: &[OsString]) -> bool {
    !rest.iter().take_while(|arg| *arg != "--").any(|arg| {
        arg.to_str().is_some_and(|text| {
            text == NO_DAEMON || text.starts_with("--remote") || NON_SESSION_FLAGS.contains(&text)
        })
    })
}

fn is_flag(arg: &OsStr) -> bool {
    arg.to_str()
        .is_some_and(|text| text.len() > 1 && text.starts_with('-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str], in_pane: bool) -> Vec<String> {
        pane_session_args(args.iter().map(OsString::from).collect(), in_pane)
            .into_iter()
            .map(|arg| arg.into_string().unwrap())
            .collect()
    }

    #[test]
    fn interactive_sessions_in_a_pane_skip_the_shared_daemon() {
        assert_eq!(run(&[], true), ["--no-daemon"]);
        assert_eq!(
            run(&["fix the build"], true),
            ["--no-daemon", "fix the build"]
        );
        assert_eq!(
            run(&["-m", "gpt-6", "-c", "model=\"x\"", "hi"], true),
            ["--no-daemon", "-m", "gpt-6", "-c", "model=\"x\"", "hi"]
        );
        assert_eq!(
            run(&["-i", "a.png", "b.png", "--search"], true),
            ["--no-daemon", "-i", "a.png", "b.png", "--search"]
        );
        assert_eq!(
            run(&["--", "exec"], true),
            ["--no-daemon", "--", "exec"],
            "after -- every word is the prompt"
        );
        assert_eq!(
            run(&["-C", "exec", "review this"], true),
            ["--no-daemon", "-C", "exec", "review this"],
            "an option value is never a subcommand"
        );
    }

    #[test]
    fn resume_and_fork_get_the_flag_after_their_subcommand() {
        assert_eq!(
            run(&["resume", "--last"], true),
            ["resume", "--no-daemon", "--last"]
        );
        assert_eq!(
            run(&["-p", "work", "fork", "0199"], true),
            ["-p", "work", "fork", "--no-daemon", "0199"]
        );
    }

    #[test]
    fn other_subcommands_and_explicit_choices_pass_unchanged() {
        for args in [
            &["exec", "hi"][..],
            &["e", "hi"],
            &["mcp", "list"],
            &["login"],
            &["app-server", "daemon", "stop"],
            &["features", "list"],
            &["--version"],
            &["-h"],
            &["--no-daemon", "hi"],
            &["resume", "--no-daemon"],
            &["--remote", "unix://", "hi"],
            &["--remote=ws://h:1"],
            &["resume", "--remote", "ws://h:1"],
            &["-m", "gpt-6", "review"],
        ] {
            assert_eq!(run(args, true), args, "{args:?}");
        }
    }

    #[test]
    fn launches_outside_a_pane_pass_unchanged() {
        for args in [&[][..], &["hi"], &["resume", "--last"]] {
            assert_eq!(run(args, false), args, "{args:?}");
        }
    }
}
