#![allow(
    clippy::panic,
    reason = "integration test setup failures need contextual diagnostics"
)]

use std::path::Path;
use std::process::Command;

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

    for source_line in lock
        .lines()
        .filter(|line| line.starts_with("source = \"git+"))
    {
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
}

#[test]
fn paneflow_links_the_native_ghostty_engine_unconditionally() {
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
    let engine = package["dependencies"]
        .as_array()
        .and_then(|dependencies| {
            dependencies
                .iter()
                .find(|dependency| dependency["name"] == "paneflow-terminal-ghostty")
        })
        .unwrap_or_else(|| panic!("paneflow-app no longer depends on the Ghostty engine"));

    // Ghostty is the only terminal engine, so nothing may make it optional or
    // leave it on the stub: a build that resolves without `native` would link a
    // terminal that cannot run a shell.
    assert_eq!(
        engine["optional"],
        serde_json::Value::Bool(false),
        "the Ghostty engine must not be an optional dependency"
    );
    assert!(
        engine["target"].is_null(),
        "the Ghostty engine must not be target-gated"
    );
    assert!(
        engine["features"]
            .as_array()
            .is_some_and(|features| features.iter().any(|feature| feature == "native")),
        "the Ghostty engine must be linked with the native feature"
    );
}
