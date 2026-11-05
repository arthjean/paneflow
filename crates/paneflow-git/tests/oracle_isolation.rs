#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod oracle;
mod support;

use std::path::Path;

use paneflow_git::porcelain::Record;

use oracle::{cli_command, gix_records, parse_z, Probe};
use support::{backdate, git, write};

const POLLUTING_CONFIG: &str = "[core]\n\tautocrlf = true\n";

const CRLF_FILE: &str = "crlf.txt";

fn cli_without(root: &Path, removed: &str) -> Vec<Record> {
    let out = cli_command(root, Probe::Files.cli_args())
        .env_remove(removed)
        .output()
        .expect("git status");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    parse_z(&out.stdout)
}

fn gix_without(root: &Path, removed: &str) -> Vec<Record> {
    let kept = std::env::var_os(removed).expect("the isolation sets the variable");
    std::env::remove_var(removed);
    let records = gix_records(root, Probe::Files);
    std::env::set_var(removed, kept);
    records.expect("gix status")
}

#[test]
fn the_fixture_isolation_hides_a_global_and_a_system_autocrlf_from_both_engines() {
    let polluted = tempfile::tempdir().expect("polluted dir");
    let home = polluted.path().join("home");
    let xdg = polluted.path().join("xdg");
    let system = polluted.path().join("system-gitconfig");
    write(&home, ".gitconfig", POLLUTING_CONFIG.as_bytes());
    write(&xdg, "git/config", POLLUTING_CONFIG.as_bytes());
    std::fs::write(&system, POLLUTING_CONFIG).expect("system config");
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_CONFIG_HOME", &xdg);
    std::env::set_var("GIT_CONFIG_SYSTEM", &system);
    support::isolate_process();

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).expect("repo dir");
    git(&root, &["init", "-q"]);
    write(&root, CRLF_FILE, b"one\r\ntwo\r\n");
    git(&root, &["-c", "core.autocrlf=true", "add", CRLF_FILE]);
    git(&root, &["commit", "-q", "--no-verify", "-m", "normalized"]);
    assert_eq!(
        git(&root, &["cat-file", "blob", &format!("HEAD:{CRLF_FILE}")]),
        "one\ntwo\n"
    );
    backdate(&root.join(CRLF_FILE));

    let modified = vec![Record {
        code: *b" M",
        path: CRLF_FILE.as_bytes().to_vec(),
    }];
    assert_eq!(
        oracle::cli_records(&root, Probe::Files).expect("cli"),
        modified,
        "the isolated CLI ignores the polluting core.autocrlf"
    );
    assert_eq!(
        gix_records(&root, Probe::Files).expect("gix"),
        modified,
        "isolated gix ignores the polluting core.autocrlf"
    );

    for (variable, source) in [
        ("GIT_CONFIG_GLOBAL", "global"),
        ("GIT_CONFIG_NOSYSTEM", "system"),
    ] {
        assert_eq!(
            cli_without(&root, variable),
            Vec::new(),
            "without {variable} the CLI reads the polluting {source} config, so the fixture discriminates"
        );
        assert_eq!(
            gix_without(&root, variable),
            Vec::new(),
            "without {variable} gix reads the polluting {source} config, so the isolation is what hides it"
        );
    }
}
