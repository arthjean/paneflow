#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Output};

fn paneflow(home: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_paneflow"));
    command
        .args(args)
        .env("HOME", home)
        .env("CODEX_HOME", home.join(".codex"))
        .env(paneflow_home::HOME_ENV, home.join(".paneflow-dev"))
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("OPENCODE_CONFIG")
        .env_remove("OPENCODE_CONFIG_DIR")
        .env_remove("PANEFLOW_SOCKET_PATH")
        .env_remove("PANEFLOW_ALLOW_SOCKET_OVERRIDE");
    for key in paneflow_host::env::PANE_CONTEXT_ENV {
        command.env_remove(key);
    }
    command.output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn every_mcp_verb_but_uninstall_names_the_replacement_and_exits_1() {
    let home = tempfile::tempdir().unwrap();
    for verb in ["install", "status"] {
        let output = paneflow(home.path(), &["mcp", verb]);
        let stderr = text(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{verb}: {stderr}");
        assert!(stderr.contains("bridge was removed"), "{stderr}");
        assert!(
            stderr.contains("paneflow integrations install <agent>"),
            "{stderr}"
        );
        assert!(stderr.contains("paneflow-conductor"), "{stderr}");
    }
    assert!(!home.path().join(".claude.json").exists());
    assert!(!home.path().join(".paneflow-dev").join("bin").exists());
}

#[test]
fn mcp_uninstall_purges_the_old_entries_and_says_when_none_are_left() {
    let home = tempfile::tempdir().unwrap();
    let bridge = home
        .path()
        .join(".paneflow")
        .join("bin")
        .join("paneflow-mcp");
    let claude = home.path().join(".claude.json");
    std::fs::write(
        &claude,
        format!(
            r#"{{"theme":"dark","mcpServers":{{"paneflow":{{"type":"stdio","command":"{}"}},"mine":{{"command":"mine"}}}}}}"#,
            bridge.display()
        ),
    )
    .unwrap();
    let codex = home.path().join(".codex").join("config.toml");
    std::fs::create_dir_all(codex.parent().unwrap()).unwrap();
    std::fs::write(
        &codex,
        format!(
            "# mine\nmodel = \"gpt-6\"\n\n[mcp_servers.paneflow]\ncommand = \"{}\"\n",
            bridge.display()
        ),
    )
    .unwrap();

    let purged = paneflow(home.path(), &["mcp", "uninstall"]);
    let stdout = text(&purged.stdout);
    assert_eq!(purged.status.code(), Some(0), "{}", text(&purged.stderr));
    let removed: Vec<&str> = stdout
        .lines()
        .filter(|line| line.starts_with("removed the "))
        .collect();
    assert_eq!(removed.len(), 2, "{stdout}");
    assert!(
        removed
            .iter()
            .any(|line| line.contains(&claude.display().to_string())),
        "{stdout}"
    );
    assert!(
        removed
            .iter()
            .any(|line| line.contains(&codex.display().to_string())),
        "{stdout}"
    );
    let claude_json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&claude).unwrap()).unwrap();
    assert_eq!(claude_json["theme"], "dark");
    assert_eq!(claude_json["mcpServers"]["mine"]["command"], "mine");
    assert!(claude_json["mcpServers"].get("paneflow").is_none());
    let codex_toml = std::fs::read_to_string(&codex).unwrap();
    assert!(codex_toml.starts_with("# mine\nmodel"), "{codex_toml}");
    assert!(!codex_toml.contains("paneflow-mcp"), "{codex_toml}");

    let again = paneflow(home.path(), &["mcp", "uninstall"]);
    assert_eq!(again.status.code(), Some(0), "{}", text(&again.stderr));
    assert_eq!(
        text(&again.stdout).trim_end(),
        "no Paneflow MCP entry found"
    );
}

#[test]
fn the_global_help_does_not_list_mcp() {
    let home = tempfile::tempdir().unwrap();
    let help = paneflow(home.path(), &["--help"]);
    assert!(help.status.success(), "{}", text(&help.stderr));
    let stdout = text(&help.stdout);
    assert!(stdout.contains("integrations"), "{stdout}");
    assert!(
        !stdout
            .lines()
            .any(|line| line.split([',', ' ']).any(|word| word == "mcp")),
        "{stdout}"
    );
}
