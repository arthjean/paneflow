#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::cell::Cell;
use std::path::{Path, PathBuf};

use paneflow_git::engine::{
    parse_engine_setting, Engine, Fallback, Probe, Selector, ACCEPTED_ENGINE_VALUES,
};
use paneflow_git::{snapshot, Error, Options};
use support::{committed_repo, git, write};

fn repo() -> (tempfile::TempDir, PathBuf) {
    support::isolate_process();
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    committed_repo(&root, &[("file.txt", "f\n")]);
    (tmp, root)
}

fn failing_gix(calls: &Cell<u32>) -> impl FnOnce() -> Result<&'static str, Error> + '_ {
    move || {
        calls.set(calls.get() + 1);
        Err(Error::Git {
            context: "compute the status",
            message: "simulated gix failure".to_string(),
        })
    }
}

fn interrupted_gix(calls: &Cell<u32>) -> impl FnOnce() -> Result<&'static str, Error> + '_ {
    move || {
        calls.set(calls.get() + 1);
        Err(Error::Interrupted)
    }
}

fn answering_gix(calls: &Cell<u32>) -> impl FnOnce() -> Result<&'static str, Error> + '_ {
    move || {
        calls.set(calls.get() + 1);
        Ok("gix")
    }
}

fn run(
    selector: &Selector,
    root: &Path,
    gix: impl FnOnce() -> Result<&'static str, Error>,
) -> &'static str {
    selector.run(Probe::FilesStatus, root, gix, || "cli")
}

#[test]
fn a_reftable_repository_is_answered_by_the_cli_without_opening_gix() {
    let (_tmp, root) = repo();
    std::fs::write(root.join(".git").join("HEAD"), "ref: refs/heads/.invalid\n").expect("HEAD");
    let selector = Selector::new(Engine::Gix);
    let calls = Cell::new(0);
    assert_eq!(run(&selector, &root, answering_gix(&calls)), "cli");
    assert_eq!(calls.get(), 0, "no gix open for a reftable repository");
    let counters = selector.counters();
    assert_eq!(counters.fallbacks(Fallback::Reftable), 1);
    assert_eq!(counters.probes(Probe::FilesStatus, Engine::Cli), 1);
    assert_eq!(counters.probes(Probe::FilesStatus, Engine::Gix), 0);
}

#[test]
fn a_ref_storage_extension_routes_the_repository_to_the_cli() {
    let (_tmp, root) = repo();
    git(&root, &["config", "core.repositoryformatversion", "1"]);
    git(&root, &["config", "extensions.refStorage", "reftable"]);
    let selector = Selector::new(Engine::Gix);
    let calls = Cell::new(0);
    assert_eq!(run(&selector, &root, answering_gix(&calls)), "cli");
    assert_eq!(calls.get(), 0);
    assert_eq!(selector.counters().fallbacks(Fallback::Reftable), 1);
}

#[test]
fn a_sha256_repository_is_answered_by_gix() {
    support::isolate_process();
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).expect("dir");
    git(&root, &["init", "-q", "--object-format=sha256"]);
    write(&root, "file.txt", b"f\n");
    let selector = Selector::new(Engine::Gix);
    let calls = Cell::new(0);
    assert_eq!(run(&selector, &root, answering_gix(&calls)), "gix");
    assert_eq!(calls.get(), 1);
    assert_eq!(selector.counters().fallbacks(Fallback::ObjectFormat), 0);
    assert!(snapshot(&root, &Options::files_status()).is_ok());
}

#[test]
fn an_object_format_gix_cannot_read_is_answered_by_the_cli_without_opening_gix() {
    let (_tmp, root) = repo();
    git(&root, &["config", "core.repositoryFormatVersion", "1"]);
    git(&root, &["config", "extensions.objectFormat", "sha3"]);
    let selector = Selector::new(Engine::Gix);
    let calls = Cell::new(0);
    assert_eq!(run(&selector, &root, answering_gix(&calls)), "cli");
    assert_eq!(calls.get(), 0, "no gix open for an unknown object format");
    assert_eq!(selector.counters().fallbacks(Fallback::ObjectFormat), 1);
}

#[test]
fn a_failed_gix_probe_is_answered_by_the_cli_in_the_same_call() {
    let (_tmp, root) = repo();
    let selector = Selector::new(Engine::Gix);
    let calls = Cell::new(0);
    assert_eq!(run(&selector, &root, failing_gix(&calls)), "cli");
    assert_eq!(calls.get(), 1);
    let counters = selector.counters();
    assert_eq!(counters.fallbacks(Fallback::Error), 1);
    assert_eq!(counters.probes(Probe::FilesStatus, Engine::Cli), 1);
    assert_eq!(counters.pinned_directories, 0);
}

#[test]
fn a_real_snapshot_error_falls_back_to_the_cli() {
    let (_tmp, root) = repo();
    std::fs::write(root.join(".git").join("index"), b"DIRC\0\0\0\x09garbage").expect("index");
    let selector = Selector::new(Engine::Gix);
    let answer = selector.run(
        Probe::IsClean,
        &root,
        || snapshot(&root, &Options::default()).map(|_| "gix"),
        || "cli",
    );
    assert_eq!(answer, "cli");
    assert_eq!(selector.counters().fallbacks(Fallback::Error), 1);
}

#[test]
fn two_consecutive_failures_pin_the_git_directory_to_the_cli() {
    let (_tmp, root) = repo();
    let selector = Selector::new(Engine::Gix);
    let calls = Cell::new(0);
    run(&selector, &root, failing_gix(&calls));
    run(&selector, &root, failing_gix(&calls));
    assert_eq!(selector.counters().pinned_directories, 1);
    let later = Cell::new(0);
    for _ in 0..3 {
        assert_eq!(
            run(&selector, &root.join("."), answering_gix(&later)),
            "cli"
        );
    }
    assert_eq!(later.get(), 0, "a pinned directory never reaches gix again");
    let counters = selector.counters();
    assert_eq!(counters.fallbacks(Fallback::Error), 5);
    assert_eq!(counters.pinned_directories, 1, "one warning per directory");
}

#[test]
fn interrupted_probes_never_pin_the_git_directory() {
    let (_tmp, root) = repo();
    let selector = Selector::new(Engine::Gix);
    let calls = Cell::new(0);
    for _ in 0..3 {
        assert_eq!(run(&selector, &root, interrupted_gix(&calls)), "cli");
    }
    assert_eq!(selector.counters().pinned_directories, 0);
    assert_eq!(run(&selector, &root, failing_gix(&calls)), "cli");
    assert_eq!(
        selector.counters().pinned_directories,
        0,
        "an interrupt does not count toward two consecutive failures"
    );
    assert_eq!(run(&selector, &root, answering_gix(&calls)), "gix");
}

#[test]
fn a_status_over_the_entry_cap_is_an_entry_cap_fallback_that_never_pins_the_git_directory() {
    let (_tmp, root) = repo();
    let selector = Selector::new(Engine::Gix);
    for _ in 0..3 {
        let answer = run(&selector, &root, || Err(Error::TooManyEntries { cap: 1 }));
        assert_eq!(answer, "cli");
    }
    let counters = selector.counters();
    assert_eq!(counters.fallbacks(Fallback::EntryCap), 3);
    assert_eq!(counters.fallbacks(Fallback::Error), 0);
    assert_eq!(counters.pinned_directories, 0);
    let calls = Cell::new(0);
    assert_eq!(run(&selector, &root, failing_gix(&calls)), "cli");
    assert_eq!(
        selector.counters().pinned_directories,
        0,
        "a capped status does not count toward two consecutive failures"
    );
    assert_eq!(run(&selector, &root, answering_gix(&calls)), "gix");
}

#[test]
fn a_success_between_failures_resets_the_count() {
    let (_tmp, root) = repo();
    let selector = Selector::new(Engine::Gix);
    let calls = Cell::new(0);
    run(&selector, &root, failing_gix(&calls));
    assert_eq!(run(&selector, &root, answering_gix(&calls)), "gix");
    run(&selector, &root, failing_gix(&calls));
    assert_eq!(run(&selector, &root, answering_gix(&calls)), "gix");
    assert_eq!(selector.counters().pinned_directories, 0);
}

#[test]
fn failures_are_counted_per_git_directory() {
    let (_tmp, first) = repo();
    let (_other_tmp, second) = repo();
    let selector = Selector::new(Engine::Gix);
    let calls = Cell::new(0);
    run(&selector, &first, failing_gix(&calls));
    run(&selector, &second, failing_gix(&calls));
    assert_eq!(run(&selector, &first, answering_gix(&calls)), "gix");
    assert_eq!(selector.counters().pinned_directories, 0);
}

#[test]
fn a_linked_worktree_is_pinned_apart_from_its_main_worktree() {
    let (tmp, root) = repo();
    let linked = tmp.path().join("linked");
    git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "linked",
            &support::forward_slashes(&linked),
        ],
    );
    let selector = Selector::new(Engine::Gix);
    let calls = Cell::new(0);
    run(&selector, &linked, failing_gix(&calls));
    run(&selector, &linked, failing_gix(&calls));
    assert_eq!(run(&selector, &linked, answering_gix(&calls)), "cli");
    assert_eq!(run(&selector, &root, answering_gix(&calls)), "gix");
}

#[test]
fn the_forced_cli_never_reaches_gix() {
    let (_tmp, root) = repo();
    let selector = Selector::new(Engine::Cli);
    let calls = Cell::new(0);
    for probe in Probe::ALL {
        assert_eq!(
            selector.run(probe, &root, answering_gix(&calls), || "cli"),
            "cli"
        );
    }
    assert_eq!(calls.get(), 0);
    let counters = selector.counters();
    assert_eq!(
        counters.fallbacks(Fallback::Forced),
        Probe::ALL.len() as u64
    );
    for probe in Probe::ALL {
        assert_eq!(counters.probes(probe, Engine::Cli), 1, "{probe:?}");
    }
}

#[test]
fn a_path_outside_any_repository_still_asks_gix_first() {
    support::isolate_process();
    let tmp = tempfile::tempdir().expect("tempdir");
    let selector = Selector::new(Engine::Gix);
    let answer = selector.run(
        Probe::FilesStatus,
        tmp.path(),
        || snapshot(tmp.path(), &Options::default()).map(|_| "gix"),
        || "cli",
    );
    assert_eq!(answer, "cli");
    assert_eq!(selector.counters().pinned_directories, 0);
}

#[test]
fn the_engine_variable_accepts_gix_and_cli_and_warns_on_anything_else() {
    assert_eq!(parse_engine_setting(None).preferred, Engine::Gix);
    assert_eq!(parse_engine_setting(None).warning, None);
    assert_eq!(parse_engine_setting(Some("gix")).preferred, Engine::Gix);
    assert_eq!(parse_engine_setting(Some("cli")).preferred, Engine::Cli);
    assert_eq!(parse_engine_setting(Some(" CLI ")).preferred, Engine::Cli);
    let unknown = parse_engine_setting(Some("libgit2"));
    assert_eq!(unknown.preferred, Engine::Gix);
    let warning = unknown.warning.expect("a warning");
    for accepted in ACCEPTED_ENGINE_VALUES {
        assert!(warning.contains(accepted), "{warning}");
    }
    assert!(warning.contains("libgit2"), "{warning}");
}
