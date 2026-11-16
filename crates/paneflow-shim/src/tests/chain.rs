use crate::chain::{agents_above, fingerprint, Chain, Launch, MAX_DEPTH, RELAUNCH_WINDOW_MS};
use crate::detect::{candidate_names, find_real_binary_in};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

const LAUNCHED_MS: u64 = 1_800_000_000_000;
const PANE: &str = "1fa27fad-d319-43a0-a3fa-59f2d710b5b2";

fn arguments(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

fn extended(
    inherited: Option<&Chain>,
    tool: &str,
    target: &str,
    values: &[&str],
    sent_back: bool,
) -> Chain {
    Chain::extended(
        inherited,
        &Launch {
            tool,
            session: PANE,
            target: Path::new(target),
            arguments: &arguments(values),
            now_ms: LAUNCHED_MS,
            sent_back,
        },
    )
}

fn launched(tool: &str, target: &str, values: &[&str]) -> Chain {
    extended(None, tool, target, values, false)
}

fn inherited_in(chain: &Chain, session: &str) -> Option<Chain> {
    let targets = chain.targets_env_value().unwrap();
    Chain::from_env(
        Some(OsStr::new(&chain.env_value())),
        Some(&targets),
        chain.targets.last().map(|target| target.as_os_str()),
        session,
    )
}

#[test]
fn a_chain_survives_the_environment_round_trip() {
    let first = launched("copilot", "launcher-a", &["--version"]);
    let second = extended(Some(&first), "copilot", "launcher-b", &["--version"], true);

    let inherited = inherited_in(&second, PANE).unwrap();

    assert_eq!(inherited, second);
    assert_eq!(inherited.depth, 2);
    assert_eq!(
        inherited.targets,
        vec![PathBuf::from("launcher-a"), PathBuf::from("launcher-b")]
    );
    assert_eq!(inherited.launcher(), Some(Path::new("launcher-b")));
}

#[test]
fn a_chain_from_another_pane_or_from_outside_a_pane_is_not_inherited() {
    let chain = launched("claude", "real-claude", &[]);

    assert_eq!(inherited_in(&chain, "another-pane"), None);
    assert_eq!(inherited_in(&chain, ""), None);
    assert!(inherited_in(&chain, PANE).is_some());
}

#[test]
fn the_last_target_is_excluded_even_when_the_target_list_is_lost() {
    let chain = launched("copilot", "launcher", &[]);

    let inherited = Chain::from_env(
        Some(OsStr::new(&chain.env_value())),
        None,
        Some(OsStr::new("launcher")),
        PANE,
    )
    .unwrap();

    assert_eq!(inherited.targets, vec![PathBuf::from("launcher")]);
}

#[test]
fn a_missing_or_malformed_chain_is_no_chain() {
    assert_eq!(Chain::from_env(None, None, None, ""), None);
    for malformed in [
        "",
        "copilot",
        "x:1:1:00::copilot",
        "1:x:1:00::copilot",
        "1:1:x:00::copilot",
        "1:1:1:zz::copilot",
        "1:1:1:00:copilot",
    ] {
        assert_eq!(
            Chain::from_env(Some(OsStr::new(malformed)), None, None, ""),
            None,
            "{malformed}"
        );
    }
}

#[test]
fn the_same_tool_and_arguments_right_after_the_launch_were_sent_back() {
    let chain = launched("copilot", "launcher", &["--flag", "two words"]);

    assert!(chain.was_sent_back(
        "copilot",
        &arguments(&["--flag", "two words"]),
        LAUNCHED_MS + 20
    ));
    assert!(chain.was_sent_back(
        "copilot",
        &arguments(&["--flag", "two words"]),
        LAUNCHED_MS - 20
    ));
}

#[test]
fn a_tool_that_runs_itself_again_was_not_sent_back() {
    let chain = launched("claude", "real-claude", &[]);

    assert!(!chain.was_sent_back("claude", &arguments(&["-p", "task"]), LAUNCHED_MS + 20));
    assert!(!chain.was_sent_back("codex", &arguments(&[]), LAUNCHED_MS + 20));
    assert!(!chain.was_sent_back(
        "claude",
        &arguments(&[]),
        LAUNCHED_MS + RELAUNCH_WINDOW_MS + 1
    ));
}

#[test]
fn the_fingerprint_separates_argument_boundaries() {
    assert_eq!(
        fingerprint(&arguments(&["a", "b"])),
        fingerprint(&arguments(&["a", "b"]))
    );
    assert_ne!(
        fingerprint(&arguments(&["ab"])),
        fingerprint(&arguments(&["a", "b"]))
    );
    assert_ne!(fingerprint(&arguments(&[])), fingerprint(&arguments(&[""])));
}

#[test]
fn depth_counts_every_nested_shim_and_caps_the_chain() {
    let mut chain = launched("claude", "real-claude", &[]);
    for level in 1..MAX_DEPTH {
        assert!(!chain.is_too_deep(), "depth {level}");
        let tool = if level % 2 == 0 { "claude" } else { "codex" };
        chain = extended(Some(&chain), tool, "real", &[], false);
    }

    assert_eq!(chain.depth, MAX_DEPTH);
    assert!(chain.is_too_deep());
}

#[test]
fn another_tool_starts_a_fresh_target_list() {
    let claude = launched("claude", "real-claude", &[]);

    let codex = extended(Some(&claude), "codex", "real-codex", &[], false);

    assert_eq!(codex.targets, vec![PathBuf::from("real-codex")]);
    assert_eq!(codex.depth, 2);
}

#[test]
fn a_shim_sent_back_by_a_launcher_is_not_one_more_agent() {
    let failed_launch = launched("copilot", "launcher", &[]);
    assert_eq!(agents_above(None, false), 0);
    assert_eq!(
        agents_above(Some(&failed_launch), true),
        0,
        "the launcher above a sent-back shim is no agent"
    );
    assert_eq!(agents_above(Some(&failed_launch), false), 1);

    let sent_back = extended(Some(&failed_launch), "copilot", "real-copilot", &[], true);
    assert_eq!((sent_back.depth, sent_back.launches), (2, 1));

    let claude = launched("claude", "real-claude", &[]);
    let probe = extended(Some(&claude), "copilot", "launcher", &[], false);
    assert_eq!(probe.launches, 2);
    assert_eq!(
        agents_above(Some(&probe), true),
        1,
        "the agent that ran the probe still owns the pane"
    );
}

#[test]
fn an_excluded_target_is_skipped_for_the_next_one_on_path() {
    let launcher_dir = tempfile::TempDir::new().unwrap();
    let real_dir = tempfile::TempDir::new().unwrap();
    let name = &candidate_names("copilot")[0];
    let launcher = launcher_dir.path().join(name);
    let real = real_dir.path().join(name);
    for path in [&launcher, &real] {
        std::fs::write(path, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    let path_entries = || vec![launcher_dir.path().to_owned(), real_dir.path().to_owned()];

    let found = find_real_binary_in(
        "copilot",
        path_entries(),
        None,
        None,
        std::slice::from_ref(&launcher),
    );
    assert_eq!(found, Some(real.clone()));

    let found = find_real_binary_in("copilot", path_entries(), None, None, &[launcher, real]);
    assert_eq!(found, None);
}
