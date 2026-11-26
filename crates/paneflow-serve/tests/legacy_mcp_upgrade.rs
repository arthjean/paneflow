#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

use paneflow_agent_integrations::{IntegrationBinaries, IntegrationState};

const SPACE_ID: &str = "550e8400-e29b-41d4-a716-446655440001";

struct Upgrade {
    _root: tempfile::TempDir,
    user: PathBuf,
    paneflow: PathBuf,
    account: PathBuf,
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn hook_helper(paneflow: &Path) -> PathBuf {
    paneflow_serve::integrations::helper_path(paneflow, paneflow_serve::integrations::HOOK_HELPER)
}

impl Upgrade {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let user = root.path().join("user");
        let paneflow = root.path().join("paneflow-home");
        let account = root.path().join("accounts").join("work");
        std::fs::create_dir_all(&user).unwrap();
        unsafe {
            std::env::set_var("HOME", &user);
            std::env::set_var(paneflow_home::HOME_ENV, &paneflow);
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
        write(&hook_helper(&paneflow), "hook");
        write(
            &paneflow_home::config_path_in(&paneflow),
            &format!(
                r#"{{"spaces":[{{"id":"{SPACE_ID}","name":"Work","env":{{"CLAUDE_CONFIG_DIR":"{}","CODEX_HOME":"{}"}}}}]}}"#,
                account.join("claude").display(),
                account.join("codex").display()
            ),
        );
        Self {
            _root: root,
            user,
            paneflow,
            account,
        }
    }

    fn bridge(&self) -> String {
        self.paneflow
            .join("bin")
            .join("paneflow-mcp")
            .display()
            .to_string()
    }

    fn legacy_files(&self) -> Vec<PathBuf> {
        vec![
            self.user.join(".claude.json"),
            self.user.join(".codex").join("config.toml"),
            self.user.join(".gemini").join("settings.json"),
            self.user
                .join(".config")
                .join("opencode")
                .join("opencode.json"),
            self.user.join(".fx").join("mcp.json"),
            self.account.join("claude").join(".claude.json"),
            self.account.join("codex").join("config.toml"),
        ]
    }

    fn seed_the_entries_0_20_0_wrote(&self) {
        let bridge = self.bridge();
        let dev_bridge = format!("{}-dev/bin/paneflow-mcp", self.paneflow.display());
        let claude = format!(
            r#"{{"numStartups":3,"mcpServers":{{"paneflow":{{"type":"stdio","command":"{bridge}","args":[]}},"paneflow-dev":{{"type":"stdio","command":"{dev_bridge}","args":[]}},"mine":{{"type":"stdio","command":"mine"}}}}}}"#
        );
        let codex = format!(
            "# my settings\nmodel = \"gpt-6\"\n\n[mcp_servers.paneflow]\ncommand = \"{bridge}\"\nargs = []\nenv_vars = [\"PANEFLOW_SESSION_ID\", \"PANEFLOW_WORKSPACE_ID\", \"PANEFLOW_HOME\", \"PANEFLOW_SOCKET_PATH\", \"PANEFLOW_HOST_ENDPOINT\", \"XDG_RUNTIME_DIR\"]\n"
        );
        write(&self.legacy_files()[0], &claude);
        write(&self.legacy_files()[1], &codex);
        write(
            &self.legacy_files()[2],
            &format!(
                "{{\n  // theme\n  \"theme\": \"Dracula\",\n  \"mcpServers\": {{\n    \"paneflow\": {{ \"command\": \"{bridge}\", \"args\": [], \"trust\": true }}\n  }}\n}}\n"
            ),
        );
        write(
            &self.legacy_files()[3],
            &format!(
                r#"{{"$schema":"https://opencode.ai/config.json","mcp":{{"paneflow":{{"type":"local","command":["{bridge}"],"enabled":true}}}}}}"#
            ),
        );
        write(
            &self.legacy_files()[4],
            &format!(
                r#"{{"mcp":{{"paneflow":{{"type":"local","command":["{bridge}"],"enabled":true}}}}}}"#
            ),
        );
        write(&self.legacy_files()[5], &claude);
        write(&self.legacy_files()[6], &codex);
    }

    fn files_naming_the_bridge(&self) -> Vec<PathBuf> {
        self.legacy_files()
            .into_iter()
            .filter(|path| {
                std::fs::read_to_string(path)
                    .unwrap()
                    .contains("paneflow-mcp")
            })
            .collect()
    }
}

fn lines(logs: &[(log::Level, String)], level: log::Level) -> Vec<&str> {
    logs.iter()
        .filter(|(at, _)| *at == level)
        .map(|(_, line)| line.as_str())
        .collect()
}

#[test]
fn an_upgrade_from_the_bridge_keeps_the_hooks_and_purges_every_entry_0_20_0_wrote() {
    let upgrade = Upgrade::new();
    let binaries = IntegrationBinaries {
        hook_binary: hook_helper(&upgrade.paneflow),
    };
    assert!(!upgrade.paneflow.join("bin").join("paneflow-mcp").exists());
    for slug in ["claude-code", "codex"] {
        paneflow_agent_integrations::install_integration(slug, &binaries, false).unwrap();
    }
    std::fs::remove_file(upgrade.user.join(".claude").join("settings.json")).unwrap();
    std::fs::remove_file(upgrade.user.join(".codex").join("hooks.json")).unwrap();
    upgrade.seed_the_entries_0_20_0_wrote();

    let debug = paneflow_serve::worker::refresh_integrations_as(&upgrade.paneflow, true);
    assert_eq!(
        upgrade.files_naming_the_bridge().len(),
        7,
        "a debug worker never purges: {debug:?}"
    );

    let release = paneflow_serve::worker::refresh_integrations_as(&upgrade.paneflow, false);

    let info = lines(&release, log::Level::Info);
    assert!(lines(&release, log::Level::Warn).is_empty(), "{release:?}");
    for runtime in ["claude-code", "codex"] {
        let refreshed = format!("paneflow-serve: refreshed the {runtime} integration");
        assert!(info.contains(&refreshed.as_str()), "{release:?}");
    }
    let removed: Vec<&&str> = info
        .iter()
        .filter(|line| line.contains("MCP entry from"))
        .collect();
    assert_eq!(removed.len(), 9, "{release:?}");
    let first_refresh = info
        .iter()
        .position(|line| line.contains("refreshed the"))
        .unwrap();
    let last_purge = info
        .iter()
        .rposition(|line| line.contains("MCP entry from"))
        .unwrap();
    assert!(
        last_purge < first_refresh,
        "the purge runs before the refresh: {release:?}"
    );
    assert!(
        upgrade.files_naming_the_bridge().is_empty(),
        "{:?}",
        upgrade.files_naming_the_bridge()
    );
    let claude: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&upgrade.legacy_files()[0]).unwrap())
            .unwrap();
    assert_eq!(claude["numStartups"], 3);
    assert_eq!(claude["mcpServers"]["mine"]["command"], "mine");
    assert!(
        std::fs::read_to_string(&upgrade.legacy_files()[1])
            .unwrap()
            .starts_with("# my settings\nmodel = \"gpt-6\"\n")
    );
    assert!(
        std::fs::read_to_string(&upgrade.legacy_files()[2])
            .unwrap()
            .contains("// theme")
    );
    for status in paneflow_agent_integrations::list_integrations()
        .into_iter()
        .filter(|status| ["claude-code", "codex"].contains(&status.slug))
    {
        assert_eq!(status.state, IntegrationState::Installed, "{}", status.slug);
    }
    assert!(
        std::fs::read_to_string(upgrade.user.join(".claude").join("settings.json"))
            .unwrap()
            .contains("paneflow-ai-hook")
    );

    let again = paneflow_serve::worker::refresh_integrations_as(&upgrade.paneflow, false);
    assert!(
        !again
            .iter()
            .any(|(_, line)| line.contains("MCP entry from")),
        "{again:?}"
    );

    std::fs::remove_file(hook_helper(&upgrade.paneflow)).unwrap();
    let missing = paneflow_serve::worker::refresh_integrations_as(&upgrade.paneflow, false);
    let warnings = lines(&missing, log::Level::Warn);
    assert_eq!(warnings.len(), 1, "{missing:?}");
    assert!(
        warnings[0].contains(&hook_helper(&upgrade.paneflow).display().to_string()),
        "{missing:?}"
    );
    assert!(
        !missing
            .iter()
            .any(|(_, line)| line.contains("refreshed the")),
        "{missing:?}"
    );
}
