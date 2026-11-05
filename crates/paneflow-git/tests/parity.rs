#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod oracle;
mod support;

use paneflow_git::engine::{self, Engine, Fallback, Selector};
use paneflow_git::porcelain::{self, Record};
use paneflow_git::{snapshot, Options};

use oracle::{base, divergences, layouts, linked, parse_z, submodules, Probe};

const UNKNOWN_MANDATORY_EXTENSION: &[u8; 4] = b"zzzz";

#[test]
fn the_cli_and_gix_report_equal_records_on_the_base_matrix() {
    oracle::run(&base::matrix(), Probe::Files).assert_parity(base::MAY_SKIP);
}

#[test]
fn sparse_checkouts_and_index_layouts_match_the_cli() {
    oracle::run(&layouts::matrix(), Probe::Files).assert_parity(&[]);
}

#[test]
fn sparse_checkouts_and_index_layouts_match_the_cli_with_the_is_clean_arguments() {
    oracle::run(&layouts::matrix(), Probe::Clean).assert_parity(&[]);
}

fn insert_index_extension(root: &std::path::Path, signature: &[u8; 4], payload: &[u8]) {
    let path = root.join(".git").join("index");
    let index = std::fs::read(&path).expect("index");
    let trailer = layouts::SKIP_HASH_TRAILER.len();
    assert_eq!(index[index.len() - trailer..], layouts::SKIP_HASH_TRAILER);
    let mut patched = index[..index.len() - trailer].to_vec();
    patched.extend_from_slice(signature);
    patched.extend_from_slice(&u32::try_from(payload.len()).expect("size").to_be_bytes());
    patched.extend_from_slice(payload);
    patched.extend_from_slice(&layouts::SKIP_HASH_TRAILER);
    std::fs::write(&path, patched).expect("patched index");
}

#[test]
fn an_unknown_mandatory_index_extension_fails_gix_and_the_selector_answers_with_the_cli() {
    support::isolate_process();
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    support::committed_repo(&root, &[("tracked.txt", "tracked\n")]);
    support::git(&root, &["config", "index.skipHash", "true"]);
    support::write(&root, "staged.txt", b"staged\n");
    support::git(&root, &["add", "staged.txt"]);
    insert_index_extension(&root, UNKNOWN_MANDATORY_EXTENSION, b"data");

    let gix_error =
        snapshot(&root, &Options::default()).expect_err("gix refuses the unknown extension");
    let cli_error =
        oracle::cli_records(&root, Probe::Files).expect_err("git refuses the unknown extension");
    assert!(cli_error.contains("zzzz"), "{cli_error}");

    let selector = Selector::new(Engine::Gix, None);
    let answer = selector.run(
        engine::Probe::FilesStatus,
        &root,
        || snapshot(&root, &Options::default()).map(|snapshot| Ok(porcelain::records(&snapshot))),
        || oracle::cli_records(&root, Probe::Files),
    );
    assert_eq!(
        answer,
        Err(cli_error),
        "the selector answers with the CLI after gix failed with {gix_error}"
    );
    let counters = selector.counters();
    assert_eq!(counters.fallbacks(Fallback::Error), 1);
    assert_eq!(counters.probes(engine::Probe::FilesStatus, Engine::Cli), 1);
    assert_eq!(counters.probes(engine::Probe::FilesStatus, Engine::Gix), 0);
}

#[test]
fn an_unknown_optional_index_extension_is_skipped_by_both_engines() {
    support::isolate_process();
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    support::committed_repo(&root, &[("tracked.txt", "tracked\n")]);
    support::git(&root, &["config", "index.skipHash", "true"]);
    support::write(&root, "staged.txt", b"staged\n");
    support::git(&root, &["add", "staged.txt"]);
    insert_index_extension(&root, b"ZZZZ", b"data");
    let cli = oracle::cli_records(&root, Probe::Files).expect("git skips it");
    let gix = oracle::gix_records(&root, Probe::Files).expect("gix skips it");
    assert_eq!(
        divergences("optional extension", &cli, &gix),
        Vec::<String>::new()
    );
    assert_eq!(
        cli,
        vec![Record {
            code: *b"A ",
            path: b"staged.txt".to_vec(),
        }]
    );
}

#[test]
fn a_linked_worktree_reports_its_own_index_with_the_files_probe_arguments() {
    oracle::run(&linked::matrix(), Probe::Files).assert_parity(&[]);
}

#[test]
fn a_linked_worktree_reports_its_own_index_with_the_is_clean_arguments() {
    oracle::run(&linked::matrix(), Probe::Clean).assert_parity(&[]);
}

#[test]
fn submodules_match_the_cli_with_the_files_probe_arguments() {
    oracle::run(&submodules::matrix(Probe::Files), Probe::Files)
        .assert_parity_with_failures(&[], submodules::MUST_FAIL);
}

#[test]
fn submodules_match_the_cli_with_the_is_clean_arguments() {
    oracle::run(&submodules::matrix(Probe::Clean), Probe::Clean)
        .assert_parity_with_failures(&[], submodules::MUST_FAIL);
}

#[test]
fn a_snapshot_opens_each_of_ten_submodules_once() {
    support::isolate_process();
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    let names: Vec<String> = (0..10).map(|index| format!("sub{index}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let subs = submodules::superproject(&root, &names);
    support::write(&subs[3], "inner.txt", b"edited\n");
    for probe in [Probe::Files, Probe::Clean] {
        let snapshot = snapshot(&root, &probe.options()).expect("snapshot");
        assert_eq!(
            snapshot.submodules_opened,
            names.len(),
            "{} probe",
            probe.name()
        );
        assert_eq!(
            porcelain::records(&snapshot),
            vec![Record {
                code: *b" M",
                path: b"sub3".to_vec(),
            }]
        );
    }
    let ignored = snapshot(
        &root,
        &Options {
            submodules: paneflow_git::Submodules::Ignore,
            ..Options::default()
        },
    )
    .expect("snapshot");
    assert_eq!(ignored.submodules_opened, 0);
}

#[test]
fn a_divergence_names_the_fixture_the_path_and_both_codes() {
    let cli = parse_z(b" M shared.txt\0?? only-cli.txt\0");
    let gix = vec![Record {
        code: *b"M ",
        path: b"shared.txt".to_vec(),
    }];
    let found = divergences("demo", &cli, &gix);
    assert_eq!(
        found,
        vec![
            "fixture demo: path \"only-cli.txt\": cli [\"??\"] gix []".to_string(),
            "fixture demo: path \"shared.txt\": cli [\" M\"] gix [\"M \"]".to_string(),
        ]
    );
}

#[test]
fn a_rename_record_consumes_its_source_path() {
    let records = parse_z(b"R  new.txt\0old.txt\0 M other.txt\0");
    assert_eq!(
        records,
        vec![
            Record {
                code: *b"R ",
                path: b"new.txt".to_vec(),
            },
            Record {
                code: *b" M",
                path: b"other.txt".to_vec(),
            },
        ]
    );
}

#[test]
fn a_fixture_skipped_outside_the_allowed_list_fails_by_name() {
    let report = oracle::Report {
        compared: Vec::new(),
        skipped: vec![("conflict_kinds", "no symbolic links".to_string())],
        both_failed: Vec::new(),
        failures: Vec::new(),
    };
    let failure = std::panic::catch_unwind(|| report.assert_parity(base::MAY_SKIP))
        .expect_err("an unexpected skip fails");
    let message = failure
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_default();
    assert!(
        message.contains("fixture conflict_kinds was skipped"),
        "{message}"
    );
}
