use crate::git_command::*;
use crate::git_fixture as fixture;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
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
    let builder = manifest
        .join("..")
        .join("crates")
        .join("paneflow-git")
        .join("src")
        .join("command.rs");
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
        "spawn git through paneflow_git::command::git, not directly: {offenders:?}"
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
    let builder = manifest
        .join("..")
        .join("crates")
        .join("paneflow-git")
        .join("src")
        .join("command.rs");
    let mut sources = Vec::new();
    all_sources(&manifest.join("src"), &mut sources);
    all_sources(&manifest.join("tests"), &mut sources);
    let git_crate = manifest.join("..").join("crates").join("paneflow-git");
    all_sources(&git_crate.join("src"), &mut sources);
    all_sources(&git_crate.join("tests"), &mut sources);
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
        "spawn test git through paneflow_git::fixture, which isolates it from the user's git config: {offenders:?}"
    );
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
    let staging = tmp.path().join("staging");
    committed_repo(&root);
    std::fs::create_dir(&staging).expect("staging dir outside the repository");
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
        let staged_edit = staging.join("tracked.txt");
        std::fs::write(&staged_edit, format!("{iteration}\n")).expect("staged edit");
        std::fs::rename(&staged_edit, root.join("tracked.txt"))
            .expect("an atomic replace never truncates a file a git probe has mapped");
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

fn backdate(path: &Path, seconds: u64) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .expect("config file")
        .set_modified(SystemTime::now() - Duration::from_secs(seconds))
        .expect("backdated mtime");
}

fn git_spawns_on_this_thread() -> u64 {
    use paneflow_git::counters::{counted_on_this_thread, profile_counter};
    counted_on_this_thread(profile_counter(true)) + counted_on_this_thread(profile_counter(false))
}

fn filter_queries_on_this_thread() -> u64 {
    paneflow_git::counters::counted_on_this_thread(paneflow_git::counters::subcommand_counter(
        Some("config"),
    ))
}

#[test]
fn the_second_diff_stat_probe_of_an_unchanged_repository_spawns_three_git_processes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    committed_repo(tmp.path());
    std::fs::write(tmp.path().join("tracked.txt"), "one\ntwo\nthree\n").expect("edit");
    backdate(&tmp.path().join(".git").join("config"), 60);
    let cwd = tmp.path().to_str().expect("utf-8 test path");

    let before = git_spawns_on_this_thread();
    let first = crate::workspace::GitDiffStats::from_cwd(cwd);
    let after_first = git_spawns_on_this_thread();
    let second = crate::workspace::GitDiffStats::from_cwd(cwd);
    let after_second = git_spawns_on_this_thread();

    assert_eq!(
        after_first - before,
        4,
        "diff-index, one blob batch and ls-files, plus one filter query reused by the second probe"
    );
    assert_eq!(
        after_second - after_first,
        3,
        "the second probe reuses the filter query"
    );
    assert_eq!(first, second);
    assert!(!second.is_empty());
}

#[test]
fn a_repository_created_just_before_its_first_probe_queries_its_filters_once_more_then_never_again()
{
    let tmp = tempfile::tempdir().expect("tempdir");
    committed_repo(tmp.path());
    std::fs::write(tmp.path().join("tracked.txt"), "one\ntwo\nthree\n").expect("edit");
    let cwd = tmp.path().to_str().expect("utf-8 test path");

    let created = filter_queries_on_this_thread();
    let _ = crate::workspace::GitDiffStats::from_cwd(cwd);
    assert!(
        cached_filter_drivers(tmp.path()).is_none(),
        "a config written by git init less than RACY_CONFIG_WINDOW ago is not cached"
    );
    assert!(filter_queries_on_this_thread() > created);

    backdate(&tmp.path().join(".git").join("config"), 300);
    let next_cycle = filter_queries_on_this_thread();
    let _ = crate::workspace::GitDiffStats::from_cwd(cwd);
    assert_eq!(
        filter_queries_on_this_thread() - next_cycle,
        1,
        "the next cycle queries once and caches the answer"
    );

    let settled = filter_queries_on_this_thread();
    for _ in 0..3 {
        let _ = crate::workspace::GitDiffStats::from_cwd(cwd);
    }
    assert_eq!(
        filter_queries_on_this_thread(),
        settled,
        "every later cycle is served by the cache"
    );
}

#[test]
fn a_config_written_in_one_workspace_reruns_the_filter_query_for_that_workspace_only() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let changed = tmp.path().join("changed");
    let untouched = tmp.path().join("untouched");
    for root in [&changed, &untouched] {
        committed_repo(root);
        std::fs::write(root.join("tracked.txt"), "one\ntwo\nthree\n").expect("edit");
        backdate(&root.join(".git").join("config"), 300);
    }
    let changed_cwd = changed.to_str().expect("utf-8 test path");
    let untouched_cwd = untouched.to_str().expect("utf-8 test path");
    let _ = crate::workspace::GitDiffStats::from_cwd(changed_cwd);
    let _ = crate::workspace::GitDiffStats::from_cwd(untouched_cwd);
    assert!(cached_filter_drivers(&changed).is_some());
    assert!(cached_filter_drivers(&untouched).is_some());

    assert!(fixture::git_succeeds(
        &changed,
        &["config", "user.name", "someone else"]
    ));
    backdate(&changed.join(".git").join("config"), 30);

    let queries = filter_queries_on_this_thread();
    let _ = crate::workspace::GitDiffStats::from_cwd(untouched_cwd);
    assert_eq!(
        filter_queries_on_this_thread(),
        queries,
        "the untouched workspace is still served by the cache"
    );
    let _ = crate::workspace::GitDiffStats::from_cwd(changed_cwd);
    assert_eq!(
        filter_queries_on_this_thread() - queries,
        1,
        "the written config reruns the query once"
    );
    let _ = crate::workspace::GitDiffStats::from_cwd(changed_cwd);
    assert_eq!(
        filter_queries_on_this_thread() - queries,
        1,
        "the new answer is served by the cache"
    );
}

fn same_size_body(generation: usize) -> String {
    format!(
        "{generation:08}\n{}",
        format!("{}\n", "x".repeat(1023)).repeat(480)
    )
}

#[test]
fn a_truncating_agent_write_never_fails_while_the_diff_stats_probe_runs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    fixture::committed_repo(&root, &[("tracked.txt", &same_size_body(99_999_999))]);
    let tracked = root.join("tracked.txt");
    backdate(&tracked, 3600);
    assert!(fixture::git_succeeds(&root, &["update-index", "--refresh"]));
    let rounds = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    const PROBE_ROUNDS: usize = 40;
    let probes = {
        let root = root.clone();
        let rounds = rounds.clone();
        std::thread::spawn(move || {
            let cwd = root.to_str().expect("utf-8 test path").to_string();
            let mut stats = Vec::new();
            for _ in 0..PROBE_ROUNDS {
                stats.push(crate::workspace::GitDiffStats::from_cwd(&cwd));
                rounds.fetch_add(1, Ordering::Relaxed);
            }
            stats
        })
    };

    let mut writes = 0usize;
    let mut failures: std::collections::BTreeMap<Option<i32>, usize> = Default::default();
    while rounds.load(Ordering::Relaxed) < PROBE_ROUNDS {
        if let Err(error) = std::fs::write(&tracked, same_size_body(writes)) {
            *failures.entry(error.raw_os_error()).or_default() += 1;
        }
        writes += 1;
    }
    let stats = probes.join().expect("probe thread");

    assert!(writes > PROBE_ROUNDS, "the writer must overlap the probes");
    assert!(
        failures.is_empty(),
        "{failures:?} failed writes out of {writes} (raw OS error 1224 is a mapped section)"
    );
    assert!(
        stats
            .iter()
            .any(|stat| stat.files_changed == 1 && !stat.unavailable),
        "a probe still sees the one rewritten file: {stats:?}"
    );
}
