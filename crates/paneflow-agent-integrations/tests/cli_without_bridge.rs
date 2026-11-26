#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

use paneflow_agent_integrations::{
    list_integrations, run_hooks_cli, run_integrations_cli, IntegrationBinaries, IntegrationState,
};

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| word.to_string()).collect()
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn state_of(slug: &str) -> IntegrationState {
    list_integrations()
        .into_iter()
        .find(|status| status.slug == slug)
        .unwrap()
        .state
}

#[test]
fn the_integrations_and_hooks_verbs_work_without_a_bridge_and_purge_the_old_entries() {
    let root = tempfile::tempdir().unwrap();
    let user = root.path().join("user");
    let paneflow = root.path().join("paneflow-home");
    unsafe {
        std::env::set_var("HOME", &user);
        std::env::set_var(paneflow_home::HOME_ENV, &paneflow);
        std::env::set_var("PATH", root.path().join("empty-path"));
        for name in [
            "CLAUDE_CONFIG_DIR",
            "CODEX_HOME",
            "XDG_CONFIG_HOME",
            "OPENCODE_CONFIG",
            "OPENCODE_CONFIG_DIR",
        ] {
            std::env::remove_var(name);
        }
    }
    let hook_binary: PathBuf = paneflow.join("bin").join("paneflow-ai-hook");
    write(&hook_binary, "hook");
    assert!(!paneflow.join("bin").join("paneflow-mcp").exists());
    let binaries = || {
        Some(IntegrationBinaries {
            hook_binary: hook_binary.clone(),
        })
    };
    let bridge = paneflow.join("bin").join("paneflow-mcp");
    let gemini = user.join(".gemini").join("settings.json");
    write(
        &gemini,
        &format!(
            "{{\n  // mine\n  \"mcpServers\": {{ \"paneflow\": {{ \"command\": \"{}\" }} }}\n}}\n",
            bridge.display()
        ),
    );

    assert_eq!(
        run_integrations_cli(&args(&["install", "claude-code"]), binaries()),
        0
    );

    let claude_json = user.join(".claude.json");
    assert!(
        !claude_json.exists(),
        "the install adds no mcpServers key to ~/.claude.json"
    );
    let settings = std::fs::read_to_string(user.join(".claude").join("settings.json")).unwrap();
    assert!(settings.contains("paneflow-ai-hook"), "{settings}");
    assert!(settings.contains("statusLine"), "{settings}");
    assert!(user
        .join(".claude")
        .join("skills")
        .join("paneflow-conductor")
        .join("SKILL.md")
        .is_file());
    assert_eq!(state_of("claude-code"), IntegrationState::Installed);
    let gemini_text = std::fs::read_to_string(&gemini).unwrap();
    assert!(!gemini_text.contains("paneflow-mcp"), "{gemini_text}");
    assert!(gemini_text.contains("// mine"), "{gemini_text}");

    write(
        &claude_json,
        &format!(
            r#"{{"mcpServers":{{"paneflow":{{"type":"stdio","command":"{}","args":[]}}}}}}"#,
            bridge.display()
        ),
    );
    assert_eq!(
        run_integrations_cli(&args(&["remove", "claude-code"]), None),
        0
    );
    assert!(!std::fs::read_to_string(&claude_json)
        .unwrap()
        .contains("paneflow-mcp"));
    assert_eq!(state_of("claude-code"), IntegrationState::NotInstalled);

    std::fs::create_dir_all(user.join(".codex")).unwrap();
    assert_eq!(run_hooks_cli(&args(&["setup"]), binaries()), 0);
    assert_eq!(state_of("claude-code"), IntegrationState::Installed);
    assert_eq!(state_of("codex"), IntegrationState::Installed);
    assert!(!user.join(".codex").join("config.toml").exists());
    assert!(!paneflow.join("bin").join("paneflow-mcp").exists());
}
