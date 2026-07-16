use std::path::Path;
use std::process::Command;

const MERMAN_SOURCE: &str = "source = \"git+https://github.com/zed-industries/merman?tag=v0.6.2-with-patches#9acc3960f04a7deeb08079d60fa8183f15e8bde1\"";
const MERMAN_PACKAGES: [(&str, &str); 7] = [
    ("dugong", "0.6.2"),
    ("dugong-graphlib", "0.6.2"),
    ("manatee", "0.6.2"),
    ("merman", "0.6.2"),
    ("merman-core", "0.6.2"),
    ("merman-render", "0.6.2"),
    ("roughr-merman", "0.12.0"),
];

#[test]
fn cargo_deny_merman_exclusions_stay_locked_to_the_reviewed_commit() {
    let lock_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../Cargo.lock");
    let lock = std::fs::read_to_string(&lock_path);
    assert!(
        lock.is_ok(),
        "failed to read {}: {:?}",
        lock_path.display(),
        lock.as_ref().err()
    );
    let lock = lock.unwrap_or_default();

    for (name, version) in MERMAN_PACKAGES {
        let package_header = format!("name = \"{name}\"\nversion = \"{version}\"");
        let section = lock
            .split("[[package]]")
            .find(|section| section.contains(&package_header));
        assert!(
            section.is_some(),
            "cargo-deny exclusion {name}@{version} no longer matches Cargo.lock"
        );
        assert!(
            section.is_some_and(|section| section.contains(MERMAN_SOURCE)),
            "{name}@{version} must stay on the reviewed Merman tag and commit"
        );
    }
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
