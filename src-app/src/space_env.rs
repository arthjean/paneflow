use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use paneflow_config::schema::{PaneFlowConfig, SpaceId};
pub(crate) use paneflow_launch::space::{
    config_dirs, configured_default_account_env, declared_space_env, ensure_config_dirs,
    ensure_dir, env_key_error, env_value_error, layered_env, platform_key, space_env,
    subprocess_env,
};

pub(crate) const SHADOWING_CREDENTIALS: &[(&str, &str, &str)] = &[
    ("ANTHROPIC_API_KEY", "CLAUDE_CONFIG_DIR", "Claude Code"),
    ("ANTHROPIC_AUTH_TOKEN", "CLAUDE_CONFIG_DIR", "Claude Code"),
    (
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDE_CONFIG_DIR",
        "Claude Code",
    ),
    ("OPENAI_API_KEY", "CODEX_HOME", "Codex"),
    ("GH_TOKEN", "GH_CONFIG_DIR", "GitHub CLI"),
    ("GITHUB_TOKEN", "GH_CONFIG_DIR", "GitHub CLI"),
];

pub(crate) const PLAIN_TEXT_WARNING: &str =
    "Stored in plain text in paneflow.json; prefer a config directory";

pub(crate) const KEPT_ENVIRONMENT_NOTICE: &str = "Open terminals keep their previous environment";

const SECRET_KEY_MARKERS: &[&str] = &["KEY", "TOKEN", "SECRET", "PASSWORD"];

pub(crate) fn stores_secret(key: &str) -> bool {
    let key = key.to_uppercase();
    SECRET_KEY_MARKERS.iter().any(|marker| key.contains(marker))
}

pub(crate) fn shadowing_credentials(
    env: &BTreeMap<String, String>,
    inherited: impl Fn(&str) -> bool,
) -> Vec<(&'static str, &'static str)> {
    let defines = |wanted: &str| env.keys().any(|defined| platform_key(defined) == wanted);
    SHADOWING_CREDENTIALS
        .iter()
        .filter(|(credential, account, _)| {
            defines(account) && inherited(credential) && !defines(credential)
        })
        .map(|(credential, _, tool)| (*credential, *tool))
        .collect()
}

pub(crate) fn inherited_from_paneflow(key: &str) -> bool {
    std::env::var_os(key).is_some_and(|value| !value.is_empty())
}

pub(crate) fn workspace_env(
    config: &PaneFlowConfig,
    workspace_id: u64,
) -> Option<HashMap<String, String>> {
    space_env(
        config,
        crate::workspace::workspace_space(workspace_id).as_ref(),
    )
}

pub(crate) use paneflow_git::directory_env::publish as publish_directory_envs;

pub(crate) fn claude_paths(
    config: &PaneFlowConfig,
    space: Option<&SpaceId>,
) -> Option<paneflow_agent_config::ClaudePaths> {
    match space_env(config, space).filter(|env| env.contains_key("CLAUDE_CONFIG_DIR")) {
        Some(env) => paneflow_agent_config::ClaudePaths::from_env(
            |name| env.get(name).map(std::ffi::OsString::from),
            dirs::home_dir(),
        ),
        None => paneflow_agent_config::ClaudePaths::current(),
    }
}

pub(crate) fn integration_target(
    config: &PaneFlowConfig,
    space: &SpaceId,
) -> Option<paneflow_mcp_install::IntegrationTarget> {
    let env = declared_space_env(config, Some(space))?;
    integration_target_in(
        &paneflow_home::paneflow_home()?,
        space,
        &env,
        dirs::home_dir(),
        paneflow_agent_config::ClaudePaths::current(),
        paneflow_agent_config::codex_home(),
    )
}

pub(crate) fn integration_target_in(
    paneflow_home: &Path,
    space: &SpaceId,
    env: &HashMap<String, String>,
    home: Option<PathBuf>,
    default_claude: Option<paneflow_agent_config::ClaudePaths>,
    default_codex: Option<PathBuf>,
) -> Option<paneflow_mcp_install::IntegrationTarget> {
    let lookup = |name: &str| env.get(name).map(std::ffi::OsString::from);
    let claude = env
        .contains_key("CLAUDE_CONFIG_DIR")
        .then(|| paneflow_agent_config::ClaudePaths::from_env(lookup, home.clone()))
        .flatten()
        .filter(|paths| Some(paths) != default_claude.as_ref());
    let codex_home = env
        .get("CODEX_HOME")
        .and_then(|value| paneflow_agent_config::absolute_env_dir("CODEX_HOME", Some(value.into())))
        .filter(|dir| Some(dir) != default_codex.as_ref());
    let target = paneflow_mcp_install::IntegrationTarget::for_space(
        paneflow_home,
        &space.to_string(),
        claude,
        codex_home,
    );
    target.moves_any_directory().then_some(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work() -> SpaceId {
        SpaceId::parse("550e8400-e29b-41d4-a716-446655440001").unwrap()
    }

    fn config_with(env: &[(&str, &str)]) -> PaneFlowConfig {
        let mut config = PaneFlowConfig::default();
        config.spaces.push(paneflow_config::schema::SpaceConfig {
            id: work(),
            name: "Work".to_string(),
            color: None,
            avatar: None,
            env: env
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
        });
        config
    }

    #[test]
    fn keys_naming_a_secret_are_flagged_for_plain_text_storage() {
        for key in ["OPENAI_API_KEY", "GH_TOKEN", "DB_SECRET", "my_password"] {
            assert!(stores_secret(key), "{key}");
        }
        for key in ["CLAUDE_CONFIG_DIR", "GIT_AUTHOR_EMAIL"] {
            assert!(!stores_secret(key), "{key}");
        }
    }

    #[test]
    fn inherited_credentials_shadow_the_space_unless_it_redefines_them() {
        let env = BTreeMap::from([
            ("CLAUDE_CONFIG_DIR".to_string(), "/x".to_string()),
            ("GH_CONFIG_DIR".to_string(), "/gh".to_string()),
            ("GITHUB_TOKEN".to_string(), String::new()),
        ]);
        let inherited =
            |key: &str| matches!(key, "GITHUB_TOKEN" | "ANTHROPIC_API_KEY" | "OPENAI_API_KEY");
        assert_eq!(
            shadowing_credentials(&env, inherited),
            vec![("ANTHROPIC_API_KEY", "Claude Code")],
            "OPENAI_API_KEY is ignored: this space sets no Codex account"
        );
        assert!(shadowing_credentials(&env, |_| false).is_empty());
    }

    #[test]
    fn a_space_targets_only_the_agent_directories_it_moves_away_from_the_default() {
        let root = std::env::temp_dir().join("space-target");
        let paneflow_home = root.join(".paneflow");
        let default_claude = paneflow_agent_config::ClaudePaths {
            config_dir: root.join(".claude"),
            global_config: root.join(".claude.json"),
        };
        let default_codex = root.join(".codex");
        let work_claude = root.join(".claude-work");
        let env = HashMap::from([
            (
                "CLAUDE_CONFIG_DIR".to_string(),
                work_claude.display().to_string(),
            ),
            (
                "CODEX_HOME".to_string(),
                default_codex.display().to_string(),
            ),
        ]);
        let target = integration_target_in(
            &paneflow_home,
            &work(),
            &env,
            Some(root.clone()),
            Some(default_claude.clone()),
            Some(default_codex.clone()),
        )
        .unwrap();
        assert_eq!(
            target.claude.map(|paths| paths.config_dir),
            Some(work_claude.clone())
        );
        assert_eq!(target.codex_home, None, "CODEX_HOME points at the default");
        assert_eq!(
            target.marker_dir,
            paneflow_home
                .join("integrations")
                .join("spaces")
                .join(work().to_string())
        );
        let unchanged = HashMap::from([("GIT_AUTHOR_NAME".to_string(), "Work Me".to_string())]);
        assert!(
            integration_target_in(
                &paneflow_home,
                &work(),
                &unchanged,
                Some(root),
                Some(default_claude),
                Some(default_codex),
            )
            .is_none()
        );
    }

    #[test]
    fn only_allowlisted_space_variables_reach_paneflow_subprocesses() {
        const GH_DIR: &str = if cfg!(windows) {
            r"C:\work\gh"
        } else {
            "/work/gh"
        };
        let config = config_with(&[
            ("GIT_AUTHOR_EMAIL", "me@work.example"),
            ("GH_CONFIG_DIR", GH_DIR),
            ("FOO_TOKEN_FILE", "/work/foo"),
        ]);
        let env = subprocess_env(&config, Some(&work()));
        assert_eq!(
            env,
            vec![
                (
                    "GIT_AUTHOR_EMAIL".to_string(),
                    "me@work.example".to_string()
                ),
                ("GH_CONFIG_DIR".to_string(), GH_DIR.to_string()),
            ]
        );
        assert!(subprocess_env(&config, None).is_empty());
    }

    #[test]
    fn git_runs_under_the_env_of_the_most_specific_workspace_directory() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        let perso = repo.join("perso");
        std::fs::create_dir_all(&perso).unwrap();
        let work_env = vec![
            ("GIT_AUTHOR_NAME".to_string(), "Work Me".to_string()),
            (
                "GIT_AUTHOR_EMAIL".to_string(),
                "me@work.example".to_string(),
            ),
        ];
        publish_directory_envs(vec![
            (repo.clone(), work_env.clone()),
            (perso.clone(), Vec::new()),
        ]);
        let env_of =
            |dir: &Path| paneflow_git::directory_env::directory_env(dir).map(|env| env.to_vec());
        assert_eq!(env_of(&repo.join("src")), Some(work_env));
        assert_eq!(env_of(&perso.join("src")), None);
        assert_eq!(env_of(&root.path().join("elsewhere")), None);

        let author_in = |dir: &Path| {
            let mut command = crate::git_command::git(
                crate::git_command::GitProfile::UserAction,
                ["var", "GIT_AUTHOR_IDENT"],
            );
            command
                .current_dir(dir)
                .env("GIT_AUTHOR_NAME", "Default Me")
                .env("GIT_AUTHOR_EMAIL", "me@default.example");
            let output = crate::git_command::run(command, std::time::Duration::from_secs(10), 4096)
                .expect("git var runs");
            String::from_utf8_lossy(&output.stdout).into_owned()
        };
        assert!(author_in(&repo).starts_with("Work Me <me@work.example>"));
        assert!(author_in(&perso).starts_with("Default Me <me@default.example>"));

        publish_directory_envs(vec![(repo.clone(), Vec::new())]);
        assert_eq!(env_of(&repo), None);
        assert!(author_in(&repo).starts_with("Default Me"));
    }
}
