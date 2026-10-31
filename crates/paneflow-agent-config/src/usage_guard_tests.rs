use std::path::{Path, PathBuf};

const FORBIDDEN: &[&str] = &[
    ".credentials.json",
    "auth.json",
    "find-generic-password",
    "Claude Code-credentials",
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn usage_sources() -> Vec<PathBuf> {
    let root = workspace_root();
    let mut sources: Vec<PathBuf> =
        std::fs::read_dir(root.join("crates/paneflow-agent-config/src/usage"))
            .expect("the usage module directory")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
            .collect();
    sources.extend(
        std::fs::read_dir(root.join("crates/paneflow-serve/src/usage"))
            .expect("the worker usage module directory")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|extension| extension == "rs")),
    );
    sources.push(root.join("crates/paneflow-ai-hook/src/status_line.rs"));
    sources.sort();
    sources
}

#[test]
fn no_usage_module_names_a_credential_store() {
    let sources = usage_sources();
    assert!(sources.len() >= 8, "{sources:?}");
    for source in sources {
        let text = std::fs::read_to_string(&source).expect("a readable usage source");
        for needle in FORBIDDEN {
            assert!(
                !text.contains(needle),
                "{} references the credential store `{needle}`",
                source.display()
            );
        }
    }
}
