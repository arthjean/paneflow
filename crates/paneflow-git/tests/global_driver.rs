#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::path::PathBuf;
use std::sync::OnceLock;

use paneflow_git::{snapshot, Options};
use support::{backdate, commit_all, init, marker_program, porcelain, write};

const TRACKED: &str = "tracked.txt";

struct Tools {
    marker: PathBuf,
}

fn tools() -> &'static Tools {
    static TOOLS: OnceLock<Tools> = OnceLock::new();
    TOOLS.get_or_init(|| {
        let dir = tempfile::Builder::new()
            .prefix("paneflow-git-driver")
            .tempdir()
            .expect("tools dir")
            .keep();
        let marker = dir.join("marker-ran");
        let program = marker_program(&dir, &marker);
        let escaped = program.replace('\\', "\\\\").replace('"', "\\\"");
        support::isolate_process_with(&format!("[filter \"global\"]\n\tclean = \"{escaped}\"\n"));
        Tools { marker }
    })
}

fn marker_runs(tools: &Tools) -> usize {
    std::fs::read_to_string(&tools.marker)
        .map(|text| text.lines().count())
        .unwrap_or(0)
}

#[test]
fn a_global_filter_driver_runs_for_both_engines_and_both_report_the_same_status() {
    let tools = tools();
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    init(&root);
    write(&root, TRACKED, b"tracked content\n");
    write(&root, ".gitattributes", b"tracked.txt filter=global\n");
    commit_all(&root, "init");

    let path = root.join(TRACKED);
    backdate(&path);
    let before = marker_runs(tools);
    let cli = porcelain(&root);
    let after_cli = marker_runs(tools);

    backdate(&path);
    let gix = snapshot(&root, &Options::default()).expect("snapshot");
    let after_gix = marker_runs(tools);

    assert!(after_cli > before, "the CLI probe ran the global driver");
    assert!(
        after_gix > after_cli,
        "the gix status ran the global driver"
    );
    assert_eq!(
        cli, "",
        "the CLI reports the passthrough-filtered file clean"
    );
    assert_eq!(gix.entries, Vec::new(), "gix reports it clean too");
}
