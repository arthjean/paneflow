use super::*;
use crate::git_fixture as fixture;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime};

const RAW_GIT_SPAWN: &str = "Command::new(\"git\")";

fn commit(cwd: &Path, message: &str) -> paneflow_process::BoundedOutput {
    fixture::output(cwd, &["commit", "-q", "-m", message])
}

fn committed_repo(root: &Path) {
    fixture::committed_repo(root, &[("tracked.txt", "one\ntwo\n")]);
}

fn sh_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn write_marker_script(script: &Path, marker: &Path) {
    std::fs::write(
        script,
        format!("#!/bin/sh\necho hit >> \"{}\"\nexit 1\n", sh_path(marker)),
    )
    .expect("marker script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(script, std::fs::Permissions::from_mode(0o755))
            .expect("script mode");
    }
}

fn run_every_probe(root: &Path) {
    let cwd = root.to_str().expect("utf-8 test path");
    let _ = crate::workspace::GitDiffStats::from_cwd(cwd);
    let _ = crate::diff::compute_head_diff(root, crate::diff::DiffOptions::default());
    let _ = crate::app::files_git::read(root);
    let _ = crate::workspace::worktree::is_clean(root);
}

fn index_state(root: &Path) -> (Vec<u8>, SystemTime) {
    let index = root.join(".git").join("index");
    let bytes = std::fs::read(&index).expect("index bytes");
    let modified = std::fs::metadata(&index)
        .and_then(|meta| meta.modified())
        .expect("index mtime");
    (bytes, modified)
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

fn production_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "tests" && name != "target" {
                production_sources(&path, out);
            }
        } else if name.ends_with(".rs") && !name.ends_with("tests.rs") {
            out.push(path);
        }
    }
}

fn production_part(source: &str) -> &str {
    let test_module_start = source
        .match_indices("#[cfg(test)]")
        .find(|(at, _)| {
            source[*at..]
                .lines()
                .skip(1)
                .find(|line| !line.trim().is_empty() && !line.trim().starts_with("#["))
                .is_some_and(|line| line.trim_start().starts_with("mod tests"))
        })
        .map_or(source.len(), |(at, _)| at);
    &source[..test_module_start]
}

#[test]
fn every_production_git_spawn_goes_through_the_builder() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let builder = manifest.join("src").join("git_command.rs");
    let mut sources = Vec::new();
    production_sources(&manifest.join("src"), &mut sources);
    if let Ok(crates) = std::fs::read_dir(manifest.join("..").join("crates")) {
        for krate in crates.flatten() {
            production_sources(&krate.path().join("src"), &mut sources);
        }
    }
    assert!(
        sources.len() > 100,
        "the guard must actually scan the workspace sources"
    );
    let offenders: Vec<String> = sources
        .iter()
        .filter(|path| **path != builder)
        .filter(|path| {
            let source = std::fs::read_to_string(path).unwrap_or_default();
            let compact: String = production_part(&source)
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            compact.contains(RAW_GIT_SPAWN)
        })
        .map(|path| path.display().to_string())
        .collect();
    assert!(
        offenders.is_empty(),
        "spawn git through crate::git_command::git, not directly: {offenders:?}"
    );
    let builder_source = std::fs::read_to_string(&builder).expect("builder source");
    assert!(production_part(&builder_source).contains(RAW_GIT_SPAWN));
}

fn all_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            all_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn every_test_git_fixture_goes_through_the_isolating_helper() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let builder = manifest.join("src").join("git_command.rs");
    let mut sources = Vec::new();
    all_sources(&manifest.join("src"), &mut sources);
    all_sources(&manifest.join("tests"), &mut sources);
    assert!(sources.len() > 100);
    let offenders: Vec<String> = sources
        .iter()
        .filter(|path| **path != builder)
        .filter(|path| {
            let compact: String = std::fs::read_to_string(path)
                .unwrap_or_default()
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            compact.contains(RAW_GIT_SPAWN)
        })
        .map(|path| path.display().to_string())
        .collect();
    assert!(
        offenders.is_empty(),
        "spawn test git through crate::git_fixture, which isolates it from the user's git config: {offenders:?}"
    );
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

#[test]
fn a_probe_never_runs_the_repository_fsmonitor_textconv_or_external_diff() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    committed_repo(&root);
    let marker = tmp.path().join("marker");
    let script = tmp.path().join("hook.sh");
    write_marker_script(&script, &marker);
    let script = sh_path(&script);
    assert!(fixture::git_succeeds(
        &root,
        &["config", "core.fsmonitor", &script]
    ));
    assert!(fixture::git_succeeds(
        &root,
        &["config", "diff.external", &script]
    ));
    assert!(fixture::git_succeeds(
        &root,
        &["config", "diff.evil.textconv", &script]
    ));
    std::fs::write(root.join(".gitattributes"), "*.txt diff=evil\n").expect("attributes");
    std::fs::write(root.join("tracked.txt"), "one\nchanged\n").expect("edit");
    std::fs::write(root.join("untracked.txt"), "new\n").expect("untracked");

    let _ = fixture::output(&root, &["status", "--porcelain"]);
    assert!(
        marker.exists(),
        "an unisolated git status runs the repository fsmonitor, so this test can detect it"
    );
    std::fs::remove_file(&marker).expect("reset marker");

    run_every_probe(&root);

    assert!(
        !marker.exists(),
        "a probe ran code configured by the repository: {:?}",
        std::fs::read_to_string(&marker)
    );
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
fn a_probe_never_runs_a_repository_filter_driver() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (root, marker) = repo_with_a_filter_driver(tmp.path());

    let _ = fixture::output(&root, &["status", "--porcelain"]);
    assert!(
        marker.exists(),
        "an unisolated git status runs the repository filter, so this test can detect it"
    );
    std::fs::remove_file(&marker).expect("reset marker");

    run_every_probe(&root);

    assert!(
        !marker.exists(),
        "a probe ran a filter driver configured by the repository: {:?}",
        std::fs::read_to_string(&marker)
    );
    assert!(
        crate::workspace::worktree::is_clean(&root).is_ok(),
        "a required filter that was neutralized must not fail the probe"
    );
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
    let listing: &[u8] = b"system\0filter.lfs.process\0global\0filter.lfs.clean\0\
        local\0filter.evil.clean\0worktree\0filter.my.driver.process\0\
        command\0filter.cli.clean\0local\0filter.evil.required\0";
    let drivers = repository_filter_drivers(listing).expect("utf-8 listing");
    assert_eq!(
        drivers.into_iter().collect::<Vec<_>>(),
        vec!["evil".to_string(), "my.driver".to_string()]
    );
    assert_eq!(
        repository_filter_drivers(b"local\0filter.\xff.clean\0"),
        None,
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
fn probes_work_in_a_worktree_linked_to_a_bare_repository() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let origin = tmp.path().join("origin");
    committed_repo(&origin);
    let bare = tmp.path().join("bare.git");
    assert!(fixture::git_succeeds(
        tmp.path(),
        &["clone", "-q", "--bare", &sh_path(&origin), &sh_path(&bare)]
    ));
    let linked = tmp.path().join("linked");
    assert!(fixture::git_succeeds(
        &bare,
        &["worktree", "add", "-q", &sh_path(&linked), "HEAD"]
    ));
    std::fs::write(linked.join("tracked.txt"), "one\ntwo\nthree\n").expect("edit");

    let stats = crate::workspace::GitDiffStats::from_cwd(linked.to_str().expect("utf-8"));
    assert_eq!(stats.files_changed, 1);
    assert_eq!(stats.insertions, 1);
    let diff = crate::diff::compute_head_diff(&linked, crate::diff::DiffOptions::default());
    assert_eq!(diff.error, None);
    assert_eq!(diff.files.len(), 1);
    assert_eq!(crate::workspace::worktree::is_clean(&linked), Ok(false));
}

#[test]
fn probes_never_rewrite_the_index_of_stat_dirty_files() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    committed_repo(&root);
    make_tracked_files_stat_dirty(&root, &["tracked.txt".to_string()]);
    let before = index_state(&root);

    run_every_probe(&root);

    assert!(index_state(&root) == before, "a probe rewrote .git/index");
    let _ = fixture::output(&root, &["status", "--porcelain"]);
    assert!(
        index_state(&root) != before,
        "an unisolated git status refreshes this index, so the test detects a rewrite"
    );
}

#[test]
fn agent_commits_never_hit_index_lock_while_probes_run() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    committed_repo(&root);
    let stop = Arc::new(AtomicBool::new(false));
    let probes = {
        let root = root.clone();
        let stop = stop.clone();
        std::thread::spawn(move || {
            let mut rounds = 0usize;
            while !stop.load(Ordering::Relaxed) {
                run_every_probe(&root);
                rounds += 1;
            }
            rounds
        })
    };

    let mut failures = Vec::new();
    let started = Instant::now();
    for iteration in 0..500 {
        std::fs::write(root.join("tracked.txt"), format!("{iteration}\n")).expect("edit");
        let added = fixture::output(&root, &["add", "tracked.txt"]);
        let committed = commit(&root, &format!("agent {iteration}"));
        for out in [&added, &committed] {
            if !out.status.success() {
                failures.push(format!(
                    "{iteration}: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
            }
        }
    }
    stop.store(true, Ordering::Relaxed);
    let rounds = probes.join().expect("probe thread");
    let lock_errors = failures
        .iter()
        .filter(|failure| failure.contains("index.lock"))
        .count();

    assert!(rounds > 0, "the probes must run during the commits");
    assert!(
        failures.is_empty(),
        "{lock_errors} index.lock errors, {} failures in 500 iterations ({rounds} probe rounds, {:?}): {:?}",
        failures.len(),
        started.elapsed(),
        failures.first()
    );
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
