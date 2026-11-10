#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::cell::Cell;
use std::path::{Path, PathBuf};

use paneflow_git::engine::{
    parse_engine_setting, Engine, Fallback, Probe, Selector, ACCEPTED_ENGINE_VALUES,
};
use paneflow_git::{snapshot, Error, Options};
use support::{commit_all, committed_repo, git, write};

const CORRUPT_INDEX: &[u8] = b"DIRC\0\0\0\x09garbage";

fn repo() -> (tempfile::TempDir, PathBuf) {
    support::isolate_process();
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    committed_repo(&root, &[("file.txt", "f\n")]);
    (tmp, root)
}

fn reftable_repo() -> (tempfile::TempDir, PathBuf) {
    support::isolate_process();
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).expect("dir");
    git(&root, &["init", "-q", "--ref-format=reftable"]);
    git(&root, &["config", "core.autocrlf", "false"]);
    write(&root, "file.txt", b"f\n");
    commit_all(&root, "init");
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
    selector.run(Probe::FilesStatus, root, gix, || "cli", |_| "unavailable")
}

fn real_snapshot(selector: &Selector, probe: Probe, root: &Path) -> &'static str {
    selector.run(
        probe,
        root,
        || snapshot(root, &Options::default()).map(|_| "gix"),
        || "cli",
        |_| "unavailable",
    )
}

#[test]
fn no_probe_reaches_the_cli_when_gix_cannot_answer() {
    let (_reftable_tmp, reftable) = reftable_repo();
    let (_corrupt_tmp, corrupt) = repo();
    std::fs::write(corrupt.join(".git").join("index"), CORRUPT_INDEX).expect("index");
    let (_capped_tmp, capped) = repo();
    let selector = Selector::new(Engine::Gix);
    let cli_calls = Cell::new(0);
    for probe in Probe::ALL {
        for _ in 0..3 {
            for (root, gix) in [
                (&reftable, None),
                (&corrupt, None),
                (&capped, Some(Error::TooManyEntries { cap: 1 })),
            ] {
                let answer = selector.run(
                    probe,
                    root,
                    || match gix {
                        Some(error) => Err(error),
                        None => snapshot(root, &Options::default()).map(|_| "gix"),
                    },
                    || {
                        cli_calls.set(cli_calls.get() + 1);
                        "cli"
                    },
                    |_| "unavailable",
                );
                assert_eq!(answer, "unavailable", "{probe:?} on {}", root.display());
            }
        }
    }
    assert_eq!(
        cli_calls.get(),
        0,
        "a background probe never answers with the CLI"
    );
    let counters = selector.counters();
    let unanswered = 3 * Probe::ALL.len() as u64;
    assert_eq!(counters.fallbacks(Fallback::Reftable), unanswered);
    assert_eq!(counters.fallbacks(Fallback::Error), unanswered);
    assert_eq!(counters.fallbacks(Fallback::EntryCap), unanswered);
    for probe in Probe::ALL {
        assert_eq!(counters.probes(probe, Engine::Cli), 0, "{probe:?}");
    }
    assert_eq!(
        counters.unavailable_directories, 2,
        "the reftable and the corrupt repositories, not the capped one"
    );
}

#[test]
fn a_reftable_repository_is_unavailable_without_opening_gix() {
    let (_tmp, root) = reftable_repo();
    let selector = Selector::new(Engine::Gix);
    let calls = Cell::new(0);
    let mut detail = String::new();
    let answer = selector.run(
        Probe::FilesStatus,
        &root,
        answering_gix(&calls),
        || "cli",
        |unavailable| {
            detail = unavailable.detail.clone();
            "unavailable"
        },
    );
    assert_eq!(answer, "unavailable");
    assert_eq!(calls.get(), 0, "no gix open for a reftable repository");
    assert!(detail.contains("reftable"), "{detail}");
    let counters = selector.counters();
    assert_eq!(counters.fallbacks(Fallback::Reftable), 1);
    assert_eq!(counters.probes(Probe::FilesStatus, Engine::Cli), 0);
    assert_eq!(counters.probes(Probe::FilesStatus, Engine::Gix), 0);
    assert_eq!(counters.unavailable_directories, 1);
}

#[test]
fn a_ref_storage_extension_makes_the_repository_unavailable() {
    let (_tmp, root) = repo();
    git(&root, &["config", "core.repositoryformatversion", "1"]);
    git(&root, &["config", "extensions.refStorage", "reftable"]);
    let selector = Selector::new(Engine::Gix);
    let calls = Cell::new(0);
    assert_eq!(run(&selector, &root, answering_gix(&calls)), "unavailable");
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
fn an_object_format_gix_cannot_read_is_unavailable_without_opening_gix() {
    let (_tmp, root) = repo();
    git(&root, &["config", "core.repositoryFormatVersion", "1"]);
    git(&root, &["config", "extensions.objectFormat", "sha3"]);
    let selector = Selector::new(Engine::Gix);
    let calls = Cell::new(0);
    let mut detail = String::new();
    let answer = selector.run(
        Probe::IsClean,
        &root,
        answering_gix(&calls),
        || "cli",
        |unavailable| {
            detail = unavailable.detail.clone();
            "unavailable"
        },
    );
    assert_eq!(answer, "unavailable");
    assert_eq!(calls.get(), 0, "no gix open for an unknown object format");
    assert!(detail.contains("sha3"), "{detail}");
    assert_eq!(selector.counters().fallbacks(Fallback::ObjectFormat), 1);
}

#[test]
fn a_failed_gix_probe_is_unavailable_in_the_same_call() {
    let (_tmp, root) = repo();
    let selector = Selector::new(Engine::Gix);
    let calls = Cell::new(0);
    assert_eq!(run(&selector, &root, failing_gix(&calls)), "unavailable");
    assert_eq!(calls.get(), 1);
    let counters = selector.counters();
    assert_eq!(counters.fallbacks(Fallback::Error), 1);
    assert_eq!(counters.probes(Probe::FilesStatus, Engine::Cli), 0);
    assert_eq!(counters.probes(Probe::FilesStatus, Engine::Gix), 0);
    assert_eq!(counters.unavailable_directories, 1);
}

#[test]
fn a_real_snapshot_error_is_unavailable() {
    let (_tmp, root) = repo();
    std::fs::write(root.join(".git").join("index"), CORRUPT_INDEX).expect("index");
    let selector = Selector::new(Engine::Gix);
    assert_eq!(
        real_snapshot(&selector, Probe::IsClean, &root),
        "unavailable"
    );
    assert_eq!(selector.counters().fallbacks(Fallback::Error), 1);
}

#[test]
fn a_repaired_repository_is_answered_by_gix_again() {
    let (_tmp, root) = repo();
    let index = root.join(".git").join("index");
    std::fs::write(&index, CORRUPT_INDEX).expect("index");
    let selector = Selector::new(Engine::Gix);
    for _ in 0..3 {
        assert_eq!(
            real_snapshot(&selector, Probe::FilesStatus, &root),
            "unavailable"
        );
    }
    assert_eq!(selector.counters().unavailable_directories, 1);
    std::fs::remove_file(&index).expect("drop the corrupt index");
    git(&root, &["reset", "-q"]);
    assert_eq!(real_snapshot(&selector, Probe::FilesStatus, &root), "gix");
    let counters = selector.counters();
    assert_eq!(counters.fallbacks(Fallback::Error), 3);
    assert_eq!(counters.probes(Probe::FilesStatus, Engine::Gix), 1);
    assert_eq!(counters.unavailable_directories, 0);
}

#[test]
fn an_interrupted_probe_is_unavailable_without_marking_the_directory() {
    let (_tmp, root) = repo();
    let selector = Selector::new(Engine::Gix);
    let calls = Cell::new(0);
    for _ in 0..3 {
        assert_eq!(
            run(&selector, &root, interrupted_gix(&calls)),
            "unavailable"
        );
    }
    assert_eq!(calls.get(), 3, "every probe asks gix again");
    assert_eq!(selector.counters().unavailable_directories, 0);
    assert_eq!(run(&selector, &root, answering_gix(&calls)), "gix");
}

#[test]
fn a_status_over_the_entry_cap_is_unavailable_without_marking_the_directory() {
    let (_tmp, root) = repo();
    let selector = Selector::new(Engine::Gix);
    for _ in 0..3 {
        let answer = run(&selector, &root, || Err(Error::TooManyEntries { cap: 1 }));
        assert_eq!(answer, "unavailable");
    }
    let counters = selector.counters();
    assert_eq!(counters.fallbacks(Fallback::EntryCap), 3);
    assert_eq!(counters.fallbacks(Fallback::Error), 0);
    assert_eq!(
        counters.unavailable_directories, 0,
        "a large status is not a fault of the repository"
    );
    let calls = Cell::new(0);
    assert_eq!(run(&selector, &root, answering_gix(&calls)), "gix");
}

#[test]
fn unavailable_directories_are_counted_per_git_directory() {
    let (_tmp, first) = repo();
    let (_other_tmp, second) = repo();
    let selector = Selector::new(Engine::Gix);
    let calls = Cell::new(0);
    run(&selector, &first, failing_gix(&calls));
    run(&selector, &first, failing_gix(&calls));
    run(&selector, &second, failing_gix(&calls));
    assert_eq!(selector.counters().unavailable_directories, 2);
    assert_eq!(run(&selector, &first, answering_gix(&calls)), "gix");
    assert_eq!(selector.counters().unavailable_directories, 1);
}

#[test]
fn a_linked_worktree_is_tracked_apart_from_its_main_worktree() {
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
    assert_eq!(run(&selector, &root, answering_gix(&calls)), "gix");
    assert_eq!(
        selector.counters().unavailable_directories,
        1,
        "an answer from the main worktree leaves the linked one unavailable"
    );
    assert_eq!(run(&selector, &linked, answering_gix(&calls)), "gix");
    assert_eq!(selector.counters().unavailable_directories, 0);
}

#[test]
fn the_forced_cli_never_reaches_gix() {
    let (_tmp, root) = repo();
    let selector = Selector::new(Engine::Cli);
    let calls = Cell::new(0);
    for probe in Probe::ALL {
        assert_eq!(
            selector.run(
                probe,
                &root,
                answering_gix(&calls),
                || "cli",
                |_| { "unavailable" }
            ),
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
    let asked = Cell::new(false);
    let answer = selector.run(
        Probe::FilesStatus,
        tmp.path(),
        || {
            asked.set(true);
            snapshot(tmp.path(), &Options::default()).map(|_| "gix")
        },
        || "cli",
        |_| "unavailable",
    );
    assert!(asked.get());
    assert_eq!(answer, "unavailable");
    assert_eq!(selector.counters().unavailable_directories, 0);
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
