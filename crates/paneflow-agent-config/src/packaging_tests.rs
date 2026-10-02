use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::build_support;
use crate::RUNTIMES;

const AI_HOOK_EXE: &str = "paneflow-ai-hook.exe";

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn wix_shims(wix: &str) -> BTreeSet<String> {
    let name = regex::Regex::new(r"Name='([^']+)\.exe'").unwrap();
    wix.lines()
        .filter(|line| line.contains(r"paneflow-helpers\"))
        .filter_map(|line| name.captures(line))
        .map(|captures| format!("{}.exe", &captures[1]))
        .filter(|file| file != AI_HOOK_EXE)
        .collect()
}

fn release_shims(workflow: &str) -> BTreeSet<String> {
    let start = workflow
        .find("$shim = Join-Path $artifactDir 'paneflow-shim.exe'")
        .expect("release.yml copies the shim under its alias names");
    let list = &workflow[start..];
    let list = &list[list.find("@(").expect("shim alias list opens")..];
    let list = &list[..list.find("))").expect("shim alias list closes")];
    let entry = regex::Regex::new(r"'([^']+\.exe)'").unwrap();
    entry
        .captures_iter(list)
        .map(|captures| captures[1].to_string())
        .collect()
}

fn shim_files<'a>(canonical_aliases: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
    canonical_aliases
        .into_iter()
        .map(|alias| format!("{alias}.exe"))
        .collect()
}

fn packaging_drift(
    expected: &BTreeSet<String>,
    source: &str,
    listed: &BTreeSet<String>,
) -> Option<String> {
    let missing = expected.difference(listed).cloned().collect::<Vec<_>>();
    let extra = listed.difference(expected).cloned().collect::<Vec<_>>();
    (!missing.is_empty() || !extra.is_empty()).then(|| {
        format!(
            "{source}: missing catalog aliases {missing:?}; entries without a runtime {extra:?}"
        )
    })
}

fn check_packaging(expected: &BTreeSet<String>, wix: &str, workflow: &str) -> Result<(), String> {
    let drift = [
        packaging_drift(expected, "packaging/wix/main.wxs", &wix_shims(wix)),
        packaging_drift(
            expected,
            ".github/workflows/release.yml",
            &release_shims(workflow),
        ),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if drift.is_empty() {
        Ok(())
    } else {
        Err(drift.join("\n"))
    }
}

fn packaged_lists() -> (String, String) {
    let root = repository_root();
    (
        fs::read_to_string(root.join("packaging/wix/main.wxs")).unwrap(),
        fs::read_to_string(root.join(".github/workflows/release.yml")).unwrap(),
    )
}

#[test]
fn windows_packaging_ships_one_shim_per_catalog_runtime() {
    let expected = shim_files(
        RUNTIMES
            .iter()
            .map(|runtime| runtime.detection.command_aliases[0]),
    );
    assert_eq!(expected.len(), RUNTIMES.len());
    let (wix, workflow) = packaged_lists();
    let drift = check_packaging(&expected, &wix, &workflow).err();
    assert!(drift.is_none(), "{}", drift.unwrap_or_default());
}

#[test]
fn a_fixture_runtime_without_a_wix_entry_fails_naming_its_alias() {
    let catalog = tempfile::tempdir().unwrap();
    let runtimes = repository_root().join("runtimes");
    for entry in fs::read_dir(&runtimes).unwrap() {
        let source = entry.unwrap().path().join("runtime.toml");
        if source.is_file() {
            let slug = source.parent().unwrap().file_name().unwrap();
            fs::create_dir_all(catalog.path().join(slug)).unwrap();
            fs::copy(&source, catalog.path().join(slug).join("runtime.toml")).unwrap();
        }
    }
    let fixture = fs::read_to_string(runtimes.join("amp").join("runtime.toml"))
        .unwrap()
        .replace("com.sourcegraph.amp", "dev.example.alpha")
        .replace("slug = \"amp\"", "slug = \"alpha\"")
        .replace("label = \"Amp\"", "label = \"Alpha\"")
        .replace("order = 6", "order = 99")
        .replace("amp_button_visible", "alpha_button_visible")
        .replace("[\"amp\"]", "[\"alpha-cli\"]")
        .replace("id = \"amp\"", "id = \"alpha\"")
        .replace("command = \"amp\"", "command = \"alpha-cli\"");
    fs::create_dir_all(catalog.path().join("alpha")).unwrap();
    fs::write(catalog.path().join("alpha").join("runtime.toml"), fixture).unwrap();

    let descriptors = build_support::discover_and_validate(catalog.path()).unwrap();
    let expected = shim_files(
        descriptors
            .iter()
            .map(|located| located.descriptor.detection.command_aliases[0].as_str()),
    );
    let (wix, workflow) = packaged_lists();
    let error = check_packaging(&expected, &wix, &workflow).unwrap_err();
    assert!(
        error.contains("packaging/wix/main.wxs: missing catalog aliases [\"alpha-cli.exe\"]"),
        "{error}"
    );
    assert!(
        error
            .contains(".github/workflows/release.yml: missing catalog aliases [\"alpha-cli.exe\"]"),
        "{error}"
    );
}
