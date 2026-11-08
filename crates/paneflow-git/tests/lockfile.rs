#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const VENDORED_CRATES: &[(&str, &str)] = &[
    ("gix-status", "0.37.0"),
    ("gix-dir", "0.32.0"),
    ("gix-index", "0.58.0"),
];

const SIZE_CAPPED_HELPERS: &[&str] = &["paneflow-shim", "paneflow-ai-hook", "paneflow-mcp"];

#[derive(Debug, Default)]
struct Package {
    name: String,
    version: String,
    source: Option<String>,
    dependencies: Vec<String>,
}

struct Lockfile {
    packages: Vec<Package>,
    unused_patches: Vec<String>,
}

fn quoted(value: &str) -> String {
    value
        .trim()
        .trim_end_matches(',')
        .trim_matches('"')
        .to_string()
}

fn lockfile() -> Lockfile {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock");
    let text = std::fs::read_to_string(&path).expect("workspace Cargo.lock");
    let mut packages = Vec::new();
    let mut unused_patches = Vec::new();
    let mut current: Option<Package> = None;
    let mut in_unused_patches = false;
    let mut in_dependencies = false;
    for line in text.lines() {
        let line = line.trim_end();
        if line.starts_with('[') {
            packages.extend(current.take());
            in_dependencies = false;
            in_unused_patches = line == "[[patch.unused]]";
            if line == "[[package]]" {
                current = Some(Package::default());
            }
            continue;
        }
        if in_unused_patches {
            if let Some(name) = line.strip_prefix("name = ") {
                unused_patches.push(quoted(name));
            }
            continue;
        }
        let Some(package) = current.as_mut() else {
            continue;
        };
        if in_dependencies {
            if line == "]" {
                in_dependencies = false;
            } else {
                let dependency = quoted(line);
                let name = dependency.split(' ').next().unwrap_or_default();
                package.dependencies.push(name.to_string());
            }
            continue;
        }
        if let Some(name) = line.strip_prefix("name = ") {
            package.name = quoted(name);
        } else if let Some(version) = line.strip_prefix("version = ") {
            package.version = quoted(version);
        } else if let Some(source) = line.strip_prefix("source = ") {
            package.source = Some(quoted(source));
        } else if line == "dependencies = [" {
            in_dependencies = true;
        }
    }
    packages.extend(current);
    Lockfile {
        packages,
        unused_patches,
    }
}

fn dependency_closure(lockfile: &Lockfile, root: &str) -> BTreeSet<String> {
    let by_name: BTreeMap<&str, Vec<&Package>> =
        lockfile
            .packages
            .iter()
            .fold(BTreeMap::new(), |mut by_name, package| {
                by_name
                    .entry(package.name.as_str())
                    .or_default()
                    .push(package);
                by_name
            });
    let mut seen = BTreeSet::new();
    let mut pending = vec![root.to_string()];
    while let Some(name) = pending.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        for package in by_name.get(name.as_str()).into_iter().flatten() {
            pending.extend(package.dependencies.iter().cloned());
        }
    }
    seen
}

#[test]
fn every_vendored_gix_crate_is_the_only_one_in_the_lockfile() {
    let lockfile = lockfile();
    for (name, version) in VENDORED_CRATES {
        let resolved: Vec<&Package> = lockfile
            .packages
            .iter()
            .filter(|package| package.name == *name)
            .collect();
        assert_eq!(
            resolved.len(),
            1,
            "exactly one {name} resolves: {resolved:?}"
        );
        assert_eq!(resolved[0].version, *version, "{name}");
        assert_eq!(
            resolved[0].source, None,
            "{name} must resolve to vendor/{name}, not to a registry"
        );
        assert!(
            !lockfile.unused_patches.iter().any(|unused| unused == name),
            "the {name} patch is unused: the gix bump no longer depends on {name} {version}, so drop or re-vendor vendor/{name}"
        );
    }
}

#[test]
fn no_size_capped_helper_depends_on_gix() {
    let lockfile = lockfile();
    for helper in SIZE_CAPPED_HELPERS {
        let closure = dependency_closure(&lockfile, helper);
        assert!(closure.contains(*helper), "{helper} is in the lockfile");
        let gix: Vec<&String> = closure
            .iter()
            .filter(|name| name.starts_with("gix") || name.as_str() == "paneflow-git")
            .collect();
        assert_eq!(gix, Vec::<&String>::new(), "{helper} pulls gix");
    }
}

#[test]
fn the_desktop_app_is_the_only_dependent_of_the_engine() {
    let lockfile = lockfile();
    let dependents: Vec<&str> = lockfile
        .packages
        .iter()
        .filter(|package| {
            package
                .dependencies
                .iter()
                .any(|name| name == "paneflow-git")
        })
        .map(|package| package.name.as_str())
        .collect();
    assert_eq!(dependents, vec!["paneflow-app"]);
}
