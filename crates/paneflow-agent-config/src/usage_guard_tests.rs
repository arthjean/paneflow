use std::path::{Path, PathBuf};

const FORBIDDEN: &[&str] = &[
    ".credentials.json",
    "auth.json",
    "find-generic-password",
    "Claude Code-credentials",
];

const GROK_IDENTITY_FILE: &str = "auth.json";
const GROK_IDENTITY_SOURCE: &str = "crates/paneflow-agent-config/src/usage/grok_identity.rs";

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
    sources.extend(
        std::fs::read_dir(root.join("src-app/src/app/usage"))
            .expect("the app usage module directory")
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
            let allowed = usize::from(
                *needle == GROK_IDENTITY_FILE && source.ends_with(GROK_IDENTITY_SOURCE),
            );
            assert_eq!(
                text.matches(needle).count(),
                allowed,
                "{} references the credential store `{needle}`",
                source.display()
            );
        }
    }
}

#[test]
fn the_grok_identity_reader_keeps_two_fields_and_no_token() {
    let text = std::fs::read_to_string(workspace_root().join(GROK_IDENTITY_SOURCE))
        .expect("the Grok identity source");
    for field in ["\"email\"", "\"subscription_tier\"", "\"oidc_issuer\""] {
        assert!(text.contains(field), "{field}");
    }
    for token in ["\"key\"", "refresh_token", "access_token", "id_token"] {
        assert!(
            !text.contains(token),
            "the reader names the token field {token}"
        );
    }
}
