#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const ENGINE_CRATES: &[&str] = &["cef", "cef-dll-sys"];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

fn read_toml(path: &Path) -> toml::Value {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    text.parse::<toml::Value>()
        .unwrap_or_else(|error| panic!("cannot parse {}: {error}", path.display()))
}

fn workspace_member_manifests() -> Vec<PathBuf> {
    let root = repository_root();
    let manifest = read_toml(&root.join("Cargo.toml"));
    manifest["workspace"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|member| root.join(member.as_str().unwrap()).join("Cargo.toml"))
        .collect()
}

fn dependency_names(table: &toml::Value, names: &mut BTreeSet<String>) {
    let Some(table) = table.as_table() else {
        return;
    };
    for (key, value) in table {
        let is_dependency_table =
            key == "dependencies" || key == "dev-dependencies" || key == "build-dependencies";
        if is_dependency_table {
            if let Some(dependencies) = value.as_table() {
                for (name, spec) in dependencies {
                    names.insert(name.clone());
                    if let Some(package) = spec.get("package").and_then(toml::Value::as_str) {
                        names.insert(package.to_string());
                    }
                }
            }
        } else if value.is_table() {
            dependency_names(value, names);
        }
    }
}

#[test]
fn no_workspace_manifest_names_the_engine_crate() {
    let manifests = workspace_member_manifests();
    assert!(
        manifests
            .iter()
            .any(|path| path.ends_with("src-app/Cargo.toml"))
    );
    assert!(
        manifests
            .iter()
            .any(|path| path.ends_with("crates/paneflow-browser-proto/Cargo.toml"))
    );
    for manifest in manifests
        .iter()
        .chain(std::iter::once(&repository_root().join("Cargo.toml")))
    {
        let mut names = BTreeSet::new();
        dependency_names(&read_toml(manifest), &mut names);
        for engine in ENGINE_CRATES {
            assert!(
                !names.contains(*engine),
                "{} names the engine crate {engine}",
                manifest.display()
            );
        }
    }
}

#[test]
fn the_browser_host_is_not_a_workspace_member() {
    let manifests = workspace_member_manifests();
    assert!(
        !manifests
            .iter()
            .any(|path| path.ends_with("crates/paneflow-browser-host/Cargo.toml"))
    );
}

#[test]
fn the_protocol_crate_reaches_no_engine_and_no_gpui_crate() {
    let lock = read_toml(&repository_root().join("Cargo.lock"));
    let mut graph: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for package in lock["package"].as_array().unwrap() {
        let name = package["name"].as_str().unwrap().to_string();
        let dependencies: Vec<String> = package
            .get("dependencies")
            .and_then(toml::Value::as_array)
            .map(|list| {
                list.iter()
                    .map(|entry| {
                        entry
                            .as_str()
                            .unwrap()
                            .split(' ')
                            .next()
                            .unwrap()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default();
        graph.entry(name).or_default().extend(dependencies);
    }
    assert!(
        !graph
            .keys()
            .any(|name| ENGINE_CRATES.contains(&name.as_str()))
    );
    let mut reached = BTreeSet::new();
    let mut stack = vec!["paneflow-browser-proto".to_string()];
    while let Some(name) = stack.pop() {
        if reached.insert(name.clone()) {
            stack.extend(graph.get(&name).cloned().unwrap_or_default());
        }
    }
    assert!(reached.contains("serde_json"));
    for name in &reached {
        assert!(
            !ENGINE_CRATES.contains(&name.as_str()) && !name.starts_with("gpui"),
            "paneflow-browser-proto reaches {name}"
        );
    }
}
