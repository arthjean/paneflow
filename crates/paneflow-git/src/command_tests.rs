use super::*;
use crate::fixture;
use std::time::SystemTime;

fn commit(cwd: &Path, message: &str) -> paneflow_process::BoundedOutput {
    fixture::output(cwd, &["commit", "-q", "-m", message])
}

fn committed_repo(root: &Path) {
    fixture::committed_repo(root, &[("tracked.txt", "one\ntwo\n")]);
}

fn sh_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn make_tracked_files_stat_dirty(root: &Path, names: &[String]) {
    let later = SystemTime::now() + Duration::from_secs(120);
    for name in names {
        std::fs::File::options()
            .write(true)
            .open(root.join(name))
            .and_then(|file| file.set_modified(later))
            .expect("touch without changing content");
    }
}

#[test]
fn both_profiles_strip_the_inherited_repository_env_and_never_prompt() {
    for profile in [GitProfile::Probe, GitProfile::UserAction] {
        let command = git(profile, ["status"]);
        let envs: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        for key in INHERITED_REPOSITORY_ENV {
            assert!(
                envs.contains(&(key.to_string(), None)),
                "{profile:?} must remove {key}"
            );
        }
        assert!(envs.contains(&("GIT_TERMINAL_PROMPT".to_string(), Some("0".to_string()))));
    }
}

fn args_of(command: &Command) -> Vec<String> {
    command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn only_the_probe_profile_isolates_config_and_disables_diff_programs() {
    let probe = args_of(&git(GitProfile::Probe, ["diff", "--shortstat", "HEAD"]));
    for setting in PROBE_CONFIG {
        assert!(probe.iter().any(|arg| arg == setting), "missing {setting}");
    }
    let hooks = format!("core.hooksPath={}", empty_hooks_dir().display());
    assert!(probe.contains(&hooks));
    let diff_at = probe.iter().position(|arg| arg == "diff").expect("diff");
    assert_eq!(
        &probe[diff_at + 1..diff_at + 3],
        &["--no-ext-diff".to_string(), "--no-textconv".to_string()]
    );

    let user = args_of(&git(
        GitProfile::UserAction,
        ["-c", "user.name=Paneflow", "commit-tree", "HEAD^{tree}"],
    ));
    assert_eq!(
        user,
        vec!["-c", "user.name=Paneflow", "commit-tree", "HEAD^{tree}"],
        "a user action keeps the repository's hooks, filters and diff programs"
    );

    let probe_with_options = args_of(&git(GitProfile::Probe, ["-C", "diff", "status"]));
    assert!(
        !probe_with_options.contains(&"--no-ext-diff".to_string()),
        "the value of -C is never mistaken for the subcommand"
    );
}

#[test]
fn the_empty_hooks_dir_is_a_real_directory_that_tests_keep_out_of_the_paneflow_home() {
    let dir = empty_hooks_dir();
    assert!(dir.is_dir(), "{} must exist", dir.display());
    assert_eq!(dir, empty_hooks_dir_in(&empty_hooks_parent()));
    if let Some(home) = paneflow_home::paneflow_home() {
        assert!(
            !dir.starts_with(home),
            "tests never write under the Paneflow home"
        );
    }
    let cache = Path::new("cache-root");
    assert_eq!(empty_hooks_dir_in(cache), cache.join("git-empty-hooks"));
}

fn write_filter_script(script: &Path, marker: &Path) {
    std::fs::write(
        script,
        format!("#!/bin/sh\necho hit >> \"{}\"\ncat\n", sh_path(marker)),
    )
    .expect("filter script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(script, std::fs::Permissions::from_mode(0o755))
            .expect("script mode");
    }
}

fn repo_with_a_filter_driver(tmp: &Path) -> (PathBuf, PathBuf) {
    let root = tmp.join("repo");
    committed_repo(&root);
    let marker = tmp.join("filter-marker");
    let script = tmp.join("filter.sh");
    write_filter_script(&script, &marker);
    let command = format!("sh {}", sh_path(&script));
    assert!(fixture::git_succeeds(
        &root,
        &["config", "filter.evil.clean", &command]
    ));
    assert!(fixture::git_succeeds(
        &root,
        &["config", "filter.evil.required", "true"]
    ));
    std::fs::write(root.join(".gitattributes"), "*.txt filter=evil\n").expect("attributes");
    let info = root.join(".git").join("info");
    std::fs::create_dir_all(&info).expect("info dir");
    std::fs::write(info.join("attributes"), "*.md filter=evil\n").expect("info attributes");
    std::fs::write(root.join("notes.md"), "notes\n").expect("notes");
    assert!(fixture::git_succeeds(
        &root,
        &["add", ".gitattributes", "notes.md"]
    ));
    assert!(commit(&root, "attributes").status.success());
    make_tracked_files_stat_dirty(&root, &["tracked.txt".to_string(), "notes.md".to_string()]);
    let _ = std::fs::remove_file(&marker);
    (root, marker)
}

#[test]
fn a_user_action_keeps_the_repository_filter_driver() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (root, marker) = repo_with_a_filter_driver(tmp.path());
    let mut command = git(GitProfile::UserAction, ["status", "--porcelain"]);
    command.current_dir(&root);
    assert!(run(command, Duration::from_secs(30), 1 << 20).is_ok());
    assert!(
        marker.exists(),
        "a user action keeps filters so that Git LFS keeps working"
    );
}

#[test]
fn only_filter_drivers_from_the_repository_config_are_neutralized() {
    let listing: &[u8] = b"system\0file:/etc/gitconfig\0filter.lfs.process\ngit-lfs\0\
        global\0file:/home/u/.gitconfig\0filter.lfs.clean\ngit-lfs\0\
        local\0file:.git/config\0filter.evil.clean\nevil\0\
        worktree\0file:.git/config.worktree\0filter.my.driver.process\nmine\0\
        command\0command line:\0filter.cli.clean\ncli\0\
        local\0file:.git/config\0filter.evil.required\0";
    let drivers = repository_filter_listing(listing, None)
        .expect("utf-8 listing")
        .drivers;
    assert_eq!(
        drivers.into_iter().collect::<Vec<_>>(),
        vec!["evil".to_string(), "my.driver".to_string()]
    );
    assert!(
        repository_filter_listing(b"local\0file:.git/config\0filter.\xff.clean\nx\0", None)
            .is_none(),
        "a driver name that cannot be neutralized exactly refuses the probe"
    );
}

#[test]
fn only_worktree_reading_probes_query_the_filter_drivers() {
    let mut show = git(GitProfile::Probe, ["show", "HEAD:tracked.txt"]);
    assert!(neutralize_repository_filters(&mut show).is_ok());
    assert!(
        !show
            .get_envs()
            .any(|(key, _)| key == OsStr::new("GIT_CONFIG_COUNT"))
    );
    assert!(reads_the_worktree(&git(
        GitProfile::Probe,
        ["-C", "diff", "status"]
    )));
    assert!(!reads_the_worktree(&git(
        GitProfile::Probe,
        ["rev-parse", "HEAD"]
    )));
    assert!(!is_probe(&git(GitProfile::UserAction, ["status"])));
}

#[test]
fn a_probe_killed_at_its_deadline_leaves_no_index_lock() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    committed_repo(&root);
    let names: Vec<String> = (0..2000).map(|i| format!("f{i}.txt")).collect();
    for name in &names {
        std::fs::write(root.join(name), "x\n").expect("file");
    }
    assert!(fixture::git_succeeds(&root, &["add", "."]));
    assert!(commit(&root, "many").status.success());
    make_tracked_files_stat_dirty(&root, &names);

    let lock = root.join(".git").join("index.lock");
    let mut timeouts = 0;
    for millis in 1..=40u64 {
        let mut command = git(GitProfile::Probe, ["status", "--porcelain"]);
        command.current_dir(&root);
        if matches!(
            run(command, Duration::from_millis(millis), 1 << 20),
            Err(paneflow_process::ProcError::Timeout)
        ) {
            timeouts += 1;
        }
        assert!(
            !lock.exists(),
            "a probe killed after {millis} ms left index.lock"
        );
    }
    assert!(
        timeouts > 0,
        "at least one probe must be killed at its deadline"
    );
}

#[test]
fn a_user_action_keeps_the_stderr_tail_instead_of_failing_past_the_cap() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut command = git(
        GitProfile::UserAction,
        [
            "-c",
            "alias.loud=!head -c 1048576 /dev/zero | tr '\\0' x >&2; echo tail-marker >&2",
            "loud",
        ],
    );
    command.current_dir(tmp.path());
    let Ok(out) = run(command, Duration::from_secs(30), 1 << 20) else {
        panic!("a chatty git command must not fail on its stderr volume");
    };
    assert!(out.status.success());
    assert!(out.stderr.len() <= 64 * 1024);
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .trim_end()
            .ends_with("tail-marker")
    );
}

#[test]
fn each_git_spawn_counts_once_under_its_profile_and_subcommand_with_the_filter_query() {
    use crate::counters::{counted_on_this_thread, profile_counter, subcommand_counter};
    let tmp = tempfile::tempdir().expect("tempdir");
    committed_repo(tmp.path());
    let counters = [
        profile_counter(true),
        profile_counter(false),
        subcommand_counter(Some("status")),
        subcommand_counter(Some("config")),
        subcommand_counter(Some("rev-parse")),
    ];
    let read = || counters.map(counted_on_this_thread);
    let before = read();

    let mut status = git(GitProfile::Probe, ["status", "--porcelain"]);
    status.current_dir(tmp.path());
    assert!(run(status, Duration::from_secs(10), 4096).is_ok());
    let after_probe = read();
    assert_eq!(
        std::array::from_fn::<u64, 5, _>(|i| after_probe[i] - before[i]),
        [2, 0, 1, 1, 0],
        "a worktree probe spawns itself and one filter query"
    );

    let mut head = git(GitProfile::UserAction, ["rev-parse", "HEAD"]);
    head.current_dir(tmp.path());
    assert!(run(head, Duration::from_secs(10), 4096).is_ok());
    let after_action = read();
    assert_eq!(
        std::array::from_fn::<u64, 5, _>(|i| after_action[i] - after_probe[i]),
        [0, 1, 0, 0, 1]
    );
}

fn backdate(path: &Path, seconds: u64) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .expect("config file")
        .set_modified(SystemTime::now() - Duration::from_secs(seconds))
        .expect("backdated mtime");
}

fn filter_queries_on_this_thread() -> u64 {
    crate::counters::counted_on_this_thread(crate::counters::subcommand_counter(Some("config")))
}

fn probe_status(
    root: &Path,
) -> Result<paneflow_process::BoundedOutput, paneflow_process::ProcError> {
    let mut status = git(GitProfile::Probe, ["status", "--porcelain"]);
    status.current_dir(root);
    run(status, Duration::from_secs(30), 1 << 20)
}

#[test]
fn a_filter_added_through_an_included_file_is_neutralized_by_the_next_probe() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    committed_repo(&root);
    let marker = tmp.path().join("filter-marker");
    let script = tmp.path().join("filter.sh");
    write_filter_script(&script, &marker);
    std::fs::write(root.join(".gitattributes"), "*.txt filter=evil\n").expect("attributes");
    assert!(fixture::git_succeeds(&root, &["add", ".gitattributes"]));
    assert!(commit(&root, "attributes").status.success());
    let included = root.join(".git").join("included.cfg");
    std::fs::write(&included, "").expect("empty include");
    assert!(fixture::git_succeeds(
        &root,
        &["config", "include.path", "included.cfg"]
    ));
    backdate(&root.join(".git").join("config"), 60);
    backdate(&included, 60);
    make_tracked_files_stat_dirty(&root, &["tracked.txt".to_string()]);

    assert!(probe_status(&root).is_ok());
    assert!(cached_filter_drivers(&root).is_some_and(|drivers| drivers.is_empty()));

    std::fs::write(
        &included,
        format!(
            "[filter \"evil\"]\n\tclean = sh {}\n\trequired = true\n",
            sh_path(&script)
        ),
    )
    .expect("included filter");
    backdate(&included, 30);
    let _ = fixture::output(&root, &["status", "--porcelain"]);
    assert!(
        marker.exists(),
        "the included filter is live for an unisolated git"
    );
    std::fs::remove_file(&marker).expect("reset marker");
    make_tracked_files_stat_dirty(&root, &["tracked.txt".to_string()]);

    let queries = filter_queries_on_this_thread();
    assert!(probe_status(&root).is_ok());
    assert_eq!(
        filter_queries_on_this_thread() - queries,
        1,
        "the query ran again"
    );
    assert!(
        !marker.exists(),
        "a probe ran a filter driver added through include.path"
    );
    assert_eq!(
        cached_filter_drivers(&root).map(|drivers| drivers.into_iter().collect::<Vec<_>>()),
        Some(vec!["evil".to_string()])
    );
}

#[test]
fn a_probe_from_a_subdirectory_watches_the_included_file_git_actually_read() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    committed_repo(&root);
    let marker = tmp.path().join("filter-marker");
    let script = tmp.path().join("filter.sh");
    write_filter_script(&script, &marker);
    std::fs::write(root.join(".gitattributes"), "*.txt filter=evil\n").expect("attributes");
    assert!(fixture::git_succeeds(&root, &["add", ".gitattributes"]));
    assert!(commit(&root, "attributes").status.success());
    let included = root.join(".git").join("included.cfg");
    std::fs::write(&included, "").expect("empty include");
    assert!(fixture::git_succeeds(
        &root,
        &["config", "include.path", "included.cfg"]
    ));
    let nested = root.join("nested").join("deeper");
    std::fs::create_dir_all(&nested).expect("nested directory");
    backdate(&root.join(".git").join("config"), 60);
    backdate(&included, 60);
    make_tracked_files_stat_dirty(&root, &["tracked.txt".to_string()]);

    assert!(probe_status(&nested).is_ok());
    assert!(
        cached_filter_drivers(&nested).is_some_and(|drivers| drivers.is_empty()),
        "a probe from a subdirectory still caches its filter query"
    );

    std::fs::write(
        &included,
        format!(
            "[filter \"evil\"]\n\tclean = sh {}\n\trequired = true\n",
            sh_path(&script)
        ),
    )
    .expect("included filter");
    backdate(&included, 30);
    make_tracked_files_stat_dirty(&root, &["tracked.txt".to_string()]);

    let queries = filter_queries_on_this_thread();
    assert!(probe_status(&nested).is_ok());
    assert_eq!(
        filter_queries_on_this_thread() - queries,
        1,
        "the included file git reported relative to the worktree root invalidates the cache"
    );
    assert!(
        !marker.exists(),
        "a subdirectory probe ran a filter driver added through include.path"
    );
}

#[test]
fn an_origin_that_does_not_resolve_to_a_file_is_never_cached() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let listing: &[u8] = b"local\0file:.git/missing.cfg\0filter.evil.clean\nevil\0";
    let parsed = repository_filter_listing(listing, Some(tmp.path())).expect("utf-8 listing");
    assert_eq!(
        parsed.drivers.into_iter().collect::<Vec<_>>(),
        vec!["evil".to_string()]
    );
    assert!(parsed.watched.is_none());
}

#[cfg(unix)]
#[test]
fn a_config_replaced_with_the_same_size_and_mtime_but_a_new_inode_invalidates_the_cache() {
    let tmp = tempfile::tempdir().expect("tempdir");
    committed_repo(tmp.path());
    let config = tmp.path().join(".git").join("config");
    backdate(&config, 60);
    assert!(probe_status(tmp.path()).is_ok());
    assert!(cached_filter_drivers(tmp.path()).is_some());

    let held = std::fs::metadata(&config).expect("config metadata");
    let replacement = tmp.path().join(".git").join("config.replacement");
    std::fs::write(&replacement, std::fs::read(&config).expect("config bytes")).expect("copy");
    std::fs::File::options()
        .write(true)
        .open(&replacement)
        .expect("replacement")
        .set_modified(held.modified().expect("mtime"))
        .expect("same mtime");
    std::fs::rename(&replacement, &config).expect("replace config");
    let replaced = std::fs::metadata(&config).expect("replaced metadata");
    assert_eq!(replaced.len(), held.len());
    assert_eq!(replaced.modified().ok(), held.modified().ok());

    assert!(cached_filter_drivers(tmp.path()).is_none());
    let queries = filter_queries_on_this_thread();
    assert!(probe_status(tmp.path()).is_ok());
    assert_eq!(filter_queries_on_this_thread() - queries, 1);
}

#[test]
fn a_failed_filter_query_fails_the_probe_and_writes_no_cache_entry() {
    let tmp = tempfile::tempdir().expect("tempdir");
    committed_repo(tmp.path());
    let config = tmp.path().join(".git").join("config");
    let valid = std::fs::read(&config).expect("config bytes");
    std::fs::write(&config, "[[[ not a config\n").expect("corrupt config");
    backdate(&config, 60);

    let failed = probe_status(tmp.path());
    assert!(failed.is_err() || failed.is_ok_and(|output| !output.status.success()));
    assert!(
        filter_cache()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(tmp.path())
            .is_none()
    );

    std::fs::write(&config, valid).expect("restore config");
    backdate(&config, 30);
    assert!(probe_status(tmp.path()).is_ok_and(|output| output.status.success()));
    assert!(cached_filter_drivers(tmp.path()).is_some());
}

#[test]
fn a_repository_without_a_local_config_caches_its_absence_until_the_file_appears() {
    let tmp = tempfile::tempdir().expect("tempdir");
    committed_repo(tmp.path());
    let config = tmp.path().join(".git").join("config");
    let valid = std::fs::read(&config).expect("config bytes");
    std::fs::remove_file(&config).expect("remove local config");

    assert!(probe_status(tmp.path()).is_ok_and(|output| output.status.success()));
    assert!(cached_filter_drivers(tmp.path()).is_some());
    let queries = filter_queries_on_this_thread();
    assert!(probe_status(tmp.path()).is_ok());
    assert_eq!(
        filter_queries_on_this_thread(),
        queries,
        "the absence is cached"
    );

    std::fs::write(&config, valid).expect("create local config");
    backdate(&config, 60);
    assert!(cached_filter_drivers(tmp.path()).is_none());
    assert!(probe_status(tmp.path()).is_ok());
    assert_eq!(
        filter_queries_on_this_thread() - queries,
        1,
        "creation invalidates"
    );
}
