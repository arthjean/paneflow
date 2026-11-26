#![allow(
    clippy::panic,
    reason = "integration test setup failures need contextual diagnostics"
)]

use std::path::{Path, PathBuf};

const PATTERNS: [&str; 4] = [
    "paneflow-mcp",
    "paneflow mcp install",
    "mcp bridge",
    "mcp_config",
];

const UNBOUNDED: [&str; 7] = [
    "CHANGELOG.md",
    "assets/io.github.arthurdev44.paneflow.metainfo.xml",
    "tasks/",
    "docs/release/qualification/",
    "docs/user/blog/",
    "bench/results/",
    "src-app/tests/mcp_bridge_is_gone.rs",
];

const BUDGETED: [(&str, usize); 12] = [
    ("docs/user/README.md", 1),
    ("crates/paneflow-agent-integrations/src/legacy_mcp.rs", 13),
    ("crates/paneflow-agent-integrations/src/integrations.rs", 4),
    (
        "crates/paneflow-agent-integrations/tests/cli_without_bridge.rs",
        5,
    ),
    ("crates/paneflow-serve/src/integrations.rs", 1),
    ("crates/paneflow-serve/tests/legacy_mcp_upgrade.rs", 4),
    ("src-app/src/ai_hooks/extract.rs", 2),
    ("src-app/src/launch.rs", 1),
    ("src-app/src/settings/tabs/spaces.rs", 1),
    ("crates/paneflow-agent-integrations/src/cli.rs", 1),
    ("src-app/tests/cli_mcp_tombstone.rs", 2),
    ("crates/paneflow-agent-config/build_support.rs", 6),
];

fn unbounded(path: &str) -> bool {
    UNBOUNDED
        .iter()
        .any(|allowed| path == *allowed || (allowed.ends_with('/') && path.starts_with(allowed)))
}

fn matching_lines(text: &str) -> usize {
    text.lines()
        .filter(|line| {
            let line = line.to_lowercase();
            PATTERNS.iter().any(|pattern| line.contains(pattern))
        })
        .count()
}

fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|byte| *byte == 0)
}

fn offenders(files: &[(String, String)]) -> Vec<String> {
    let mut found = Vec::new();
    for (path, text) in files {
        if unbounded(path) {
            continue;
        }
        let count = matching_lines(text);
        let budget = BUDGETED
            .iter()
            .find(|(allowed, _)| allowed == path)
            .map_or(0, |(_, budget)| *budget);
        if count > budget {
            found.push(format!(
                "{path}: {count} line(s) name the removed MCP bridge, {budget} allowed"
            ));
        }
    }
    found
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or_else(|| panic!("src-app has a parent directory"))
        .to_path_buf()
}

fn tracked_files(root: &Path) -> Vec<(String, String)> {
    let output = paneflow_git::fixture::output(root, &["ls-files", "-z"]);
    assert!(
        output.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut files = Vec::new();
    for path in output.stdout.split(|byte| *byte == 0) {
        if path.is_empty() {
            continue;
        }
        let path = String::from_utf8_lossy(path).into_owned();
        let Ok(bytes) = std::fs::read(root.join(&path)) else {
            continue;
        };
        if looks_binary(&bytes) {
            continue;
        }
        files.push((path, String::from_utf8_lossy(&bytes).into_owned()));
    }
    files
}

#[test]
fn no_tracked_file_outside_the_allowlist_names_the_removed_bridge() {
    let files = tracked_files(&repository_root());
    assert!(
        files.len() > 100,
        "git ls-files listed {} files",
        files.len()
    );
    let found = offenders(&files);
    assert!(
        found.is_empty(),
        "the MCP bridge was removed; describe the paneflow CLI instead:\n{}",
        found.join("\n")
    );
}

#[test]
fn every_allowlisted_file_still_exists_and_still_needs_its_budget() {
    let root = repository_root();
    for (path, budget) in BUDGETED {
        let text = std::fs::read_to_string(root.join(path))
            .unwrap_or_else(|error| panic!("allowlisted {path} cannot be read: {error}"));
        let count = matching_lines(&text);
        assert!(
            count > 0,
            "{path} no longer names the bridge; remove it from the allowlist"
        );
        assert!(
            count <= budget,
            "{path} has {count} matching lines, over its budget of {budget}"
        );
    }
}

#[test]
fn a_new_file_that_documents_the_removed_install_command_fails_naming_it() {
    let files = vec![
        (
            "docs/user/new-guide.md".to_string(),
            "Run Paneflow MCP Install to connect your agent.\n".to_string(),
        ),
        (
            "tasks/prd-old.md".to_string(),
            "paneflow mcp install\n".to_string(),
        ),
        ("README.md".to_string(), "paneflow ls\n".to_string()),
    ];
    let found = offenders(&files);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].starts_with("docs/user/new-guide.md:"), "{found:?}");
}

#[test]
fn an_allowlisted_file_over_its_budget_fails_naming_it() {
    let budget = BUDGETED
        .iter()
        .find(|(path, _)| *path == "src-app/src/launch.rs")
        .map(|(_, budget)| *budget)
        .unwrap_or_else(|| panic!("launch.rs is allowlisted"));
    let text = "a paneflow-mcp line\n".repeat(budget + 1);
    let found = offenders(&[("src-app/src/launch.rs".to_string(), text)]);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].starts_with("src-app/src/launch.rs:"), "{found:?}");
}
