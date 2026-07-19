#![allow(
    clippy::panic,
    reason = "integration test setup failures need contextual diagnostics"
)]

use std::path::Path;
use std::process::Command;

const MERMAN_SOURCE: &str = "source = \"git+https://github.com/zed-industries/merman?tag=v0.6.2-with-patches#9acc3960f04a7deeb08079d60fa8183f15e8bde1\"";

#[test]
fn cargo_lock_git_sources_are_immutable() {
    let lock_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../Cargo.lock");
    let lock = std::fs::read_to_string(&lock_path);
    assert!(
        lock.is_ok(),
        "failed to read {}: {:?}",
        lock_path.display(),
        lock.as_ref().err()
    );
    let lock = lock.unwrap_or_default();

    let mut saw_merman = false;
    for source_line in lock
        .lines()
        .filter(|line| line.starts_with("source = \"git+"))
    {
        if source_line == MERMAN_SOURCE {
            saw_merman = true;
            continue;
        }

        let source = source_line
            .strip_prefix("source = \"git+")
            .and_then(|source| source.strip_suffix('"'))
            .unwrap_or_default();
        let (spec, resolved) = source.rsplit_once('#').unwrap_or_default();
        let revision = spec
            .rsplit_once("?rev=")
            .map(|(_, revision)| revision)
            .unwrap_or_default();
        assert!(
            revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "git source must use a full immutable revision: {source_line}"
        );
        assert_eq!(
            revision, resolved,
            "git source revision and resolved commit differ: {source_line}"
        );
    }

    assert!(
        saw_merman,
        "the reviewed Merman tag disappeared; tighten cargo-deny back to revision-only sources"
    );
}

#[test]
fn paneflow_default_features_select_the_linux_ghostty_backend() {
    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(&manifest_path)
        .output()
        .unwrap_or_else(|error| panic!("failed to inspect Cargo metadata: {error}"));
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("cargo metadata returned invalid JSON: {error}"));
    let package = metadata["packages"]
        .as_array()
        .and_then(|packages| {
            packages
                .iter()
                .find(|package| package["name"] == "paneflow-app")
        })
        .unwrap_or_else(|| panic!("cargo metadata omitted paneflow-app"));
    let defaults = package["features"]["default"]
        .as_array()
        .unwrap_or_else(|| panic!("paneflow-app default features are not an array"));

    assert!(
        defaults.iter().any(|feature| feature == "libghostty-linux"),
        "cargo run must activate libghostty-linux by default"
    );
}
