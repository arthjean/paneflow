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

#[test]
fn embedded_helpers_never_link_the_textdiff_crate() {
    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--manifest-path"])
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

    let names: std::collections::HashMap<&str, &str> = metadata["packages"]
        .as_array()
        .map(|packages| {
            packages
                .iter()
                .filter_map(|package| Some((package["id"].as_str()?, package["name"].as_str()?)))
                .collect()
        })
        .unwrap_or_default();
    let edges: std::collections::HashMap<&str, Vec<&str>> = metadata["resolve"]["nodes"]
        .as_array()
        .map(|nodes| {
            nodes
                .iter()
                .filter_map(|node| {
                    let id = node["id"].as_str()?;
                    let deps = node["deps"]
                        .as_array()?
                        .iter()
                        .filter_map(|dep| dep["pkg"].as_str())
                        .collect();
                    Some((id, deps))
                })
                .collect()
        })
        .unwrap_or_default();
    assert!(
        names.values().any(|name| *name == "paneflow-textdiff"),
        "the workspace no longer resolves paneflow-textdiff"
    );

    let reaches_textdiff = |root: &str| {
        let root_id = names
            .iter()
            .find(|(_, name)| **name == root)
            .map(|(id, _)| *id)
            .unwrap_or_else(|| panic!("cargo metadata omitted {root}"));
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![root_id];
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            if names.get(id) == Some(&"paneflow-textdiff") {
                return true;
            }
            if let Some(deps) = edges.get(id) {
                stack.extend(deps.iter().copied());
            }
        }
        false
    };

    for helper in ["paneflow-shim", "paneflow-ai-hook", "paneflow-mcp"] {
        assert!(
            !reaches_textdiff(helper),
            "{helper} must not depend on paneflow-textdiff, transitively or otherwise"
        );
    }
    assert!(
        reaches_textdiff("paneflow-app"),
        "paneflow-app is expected to link paneflow-textdiff"
    );
}
