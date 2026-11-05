#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::path::{Path, PathBuf};

use paneflow_git::{snapshot, Options};
use support::{backdate, commit_all, git, init, marker_program, write};

const TRACKED: &str = "tracked.txt";

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    marker: PathBuf,
    program: String,
}

fn fixture() -> Fixture {
    support::isolate_process();
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    let tools = tmp.path().join("tools");
    std::fs::create_dir_all(&tools).expect("tools dir");
    let marker = tmp.path().join("marker-ran");
    let program = marker_program(&tools, &marker);
    init(&root);
    write(&root, TRACKED, b"tracked content\n");
    write(&root, "second.txt", b"second content\n");
    commit_all(&root, "init");
    Fixture {
        _tmp: tmp,
        root,
        marker,
        program,
    }
}

fn marker_settings(program: &str) -> Vec<(String, String)> {
    [
        ("filter.marker.clean", program),
        ("filter.marker.smudge", program),
        ("filter.marker.process", program),
        ("core.fsmonitor", program),
        ("diff.marker.textconv", program),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect()
}

fn write_config_file(path: &Path, settings: &[(String, String)]) {
    let mut text = String::new();
    for (key, value) in settings {
        let (section, rest) = key.split_once('.').expect("section");
        let (subsection, name) = match rest.rsplit_once('.') {
            Some((subsection, name)) => (Some(subsection), name),
            None => (None, rest),
        };
        match subsection {
            Some(subsection) => text.push_str(&format!("[{section} \"{subsection}\"]\n")),
            None => text.push_str(&format!("[{section}]\n")),
        }
        text.push_str(&format!("\t{name} = {}\n", value.replace('\\', "\\\\")));
    }
    std::fs::write(path, text).expect("config file");
}

fn assign_marker_attributes_and_dirty_the_stat(root: &Path) {
    write(
        root,
        ".git/info/attributes",
        b"* filter=marker diff=marker\n",
    );
    write(root, ".git/hooks/post-index-change", b"#!/bin/sh\nexit 0\n");
    backdate(&root.join(TRACKED));
}

fn assert_clean_snapshot_without_marker(fixture: &Fixture) {
    let snapshot = snapshot(&fixture.root, &Options::default());
    assert!(
        !fixture.marker.exists(),
        "a repository-configured program ran: {:?}",
        std::fs::read_to_string(&fixture.marker)
    );
    assert_eq!(
        snapshot.expect("snapshot").entries,
        Vec::new(),
        "the stat-dirty file hashes raw, like the CLI probe after neutralization"
    );
}

#[test]
fn a_status_runs_no_filter_fsmonitor_or_textconv_declared_in_the_repository_config() {
    let fixture = fixture();
    for (key, value) in marker_settings(&fixture.program) {
        git(&fixture.root, &["config", &key, &value]);
    }
    assign_marker_attributes_and_dirty_the_stat(&fixture.root);
    assert_clean_snapshot_without_marker(&fixture);
}

#[test]
fn a_status_runs_no_driver_declared_through_an_include_path() {
    let fixture = fixture();
    let included = fixture.root.join(".git").join("marker.inc");
    write_config_file(&included, &marker_settings(&fixture.program));
    git(&fixture.root, &["config", "include.path", "marker.inc"]);
    assign_marker_attributes_and_dirty_the_stat(&fixture.root);
    assert_clean_snapshot_without_marker(&fixture);
}

#[test]
fn a_status_runs_no_driver_declared_through_an_include_if() {
    let fixture = fixture();
    let included = fixture.root.join(".git").join("marker-if.inc");
    write_config_file(&included, &marker_settings(&fixture.program));
    git(
        &fixture.root,
        &[
            "config",
            "includeIf.gitdir:**/repo/.git.path",
            "marker-if.inc",
        ],
    );
    git(
        &fixture.root,
        &["config", "includeIf.onbranch:main.path", "marker-if.inc"],
    );
    assign_marker_attributes_and_dirty_the_stat(&fixture.root);
    assert_clean_snapshot_without_marker(&fixture);
}

#[test]
fn a_required_repository_filter_is_skipped_and_the_raw_file_is_hashed() {
    let fixture = fixture();
    for (key, value) in marker_settings(&fixture.program) {
        git(&fixture.root, &["config", &key, &value]);
    }
    git(&fixture.root, &["config", "filter.marker.required", "true"]);
    assign_marker_attributes_and_dirty_the_stat(&fixture.root);
    assert_clean_snapshot_without_marker(&fixture);
}

#[test]
fn a_repository_filter_does_not_hide_a_real_modification() {
    let fixture = fixture();
    for (key, value) in marker_settings(&fixture.program) {
        git(&fixture.root, &["config", &key, &value]);
    }
    git(&fixture.root, &["config", "filter.marker.required", "true"]);
    assign_marker_attributes_and_dirty_the_stat(&fixture.root);
    write(&fixture.root, TRACKED, b"Tracked content\n");
    backdate(&fixture.root.join(TRACKED));
    let snapshot = snapshot(&fixture.root, &Options::default());
    assert!(
        !fixture.marker.exists(),
        "a repository-configured program ran"
    );
    let snapshot = snapshot.expect("snapshot");
    let paths: Vec<&[u8]> = snapshot
        .entries
        .iter()
        .map(|entry| entry.path.as_slice())
        .collect();
    assert_eq!(paths, vec![TRACKED.as_bytes()]);
}
