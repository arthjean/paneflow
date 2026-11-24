use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use paneflow_config::schema::{PaneFlowConfig, SpaceId};

pub const CONFIG_DIR_KEYS: &[&str] = &["CLAUDE_CONFIG_DIR", "CODEX_HOME", "GH_CONFIG_DIR"];

const RESERVED_PREFIX: &str = "PANEFLOW_";

pub fn platform_key(key: &str) -> String {
    if cfg!(windows) {
        key.to_uppercase()
    } else {
        key.to_string()
    }
}

pub fn env_key_error(key: &str) -> Option<&'static str> {
    let key = platform_key(key);
    if !paneflow_host::env::is_valid_env_name(&key) || !is_env_key(&key) {
        return Some("Not a valid variable name");
    }
    if key.starts_with(RESERVED_PREFIX) {
        return Some("PANEFLOW_ variables are reserved for Paneflow");
    }
    if crate::pty_env::PROTECTED_ENV_KEYS.contains(&key.as_str()) {
        return Some("Paneflow sets it for every terminal");
    }
    if paneflow_host::env::is_forbidden_child_env_key(&key) {
        return Some("It could change how programs load or run agents");
    }
    None
}

pub fn env_value_error(key: &str, expanded: &str) -> Option<&'static str> {
    let key = platform_key(key);
    (CONFIG_DIR_KEYS.contains(&key.as_str()) && !Path::new(expanded).is_absolute())
        .then_some("Must be an absolute path")
}

fn warn_once(message: String) {
    static LOGGED: std::sync::Mutex<Option<std::collections::HashSet<String>>> =
        std::sync::Mutex::new(None);
    let mut logged = LOGGED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if logged
        .get_or_insert_with(std::collections::HashSet::new)
        .insert(message.clone())
    {
        log::warn!("{message}");
    }
}

fn expand_value(key: &str, value: &str, home: Option<&Path>) -> String {
    match crate::env_expand::expand_with_process_env(value, home) {
        Ok(expanded) => expanded,
        Err(name) => {
            warn_once(format!(
                "spaces: env '{key}' references '{name}', which is not set; passing the value through unchanged"
            ));
            value.to_string()
        }
    }
}

pub fn resolve_env(
    space: &SpaceId,
    env: &BTreeMap<String, String>,
    home: Option<&Path>,
) -> HashMap<String, String> {
    env.iter()
        .filter_map(|(key, value)| {
            if let Some(reason) = env_key_error(key) {
                warn_once(format!(
                    "spaces: space {space}: ignoring env {key:?}: {reason}"
                ));
                return None;
            }
            let expanded = expand_value(key, value, home);
            if let Some(reason) = env_value_error(key, &expanded) {
                warn_once(format!(
                    "spaces: space {space}: ignoring env {key:?}={expanded:?}: {reason}"
                ));
                return None;
            }
            Some((platform_key(key), expanded))
        })
        .collect()
}

pub fn declared_space_env(
    config: &PaneFlowConfig,
    space: Option<&SpaceId>,
) -> Option<HashMap<String, String>> {
    let declared = config.space(space?)?;
    let env = resolve_env(&declared.id, &declared.env, dirs::home_dir().as_deref());
    (!env.is_empty()).then_some(env)
}

pub fn default_account_env(
    accounts: &BTreeMap<String, String>,
    home: Option<&Path>,
) -> Option<HashMap<String, String>> {
    let env: HashMap<String, String> = accounts
        .iter()
        .filter_map(|(slug, dir)| {
            let Some(agent) = paneflow_agent_config::usage::accounts::UsageAgent::for_runtime_slug(slug)
            else {
                warn_once(format!(
                    "agents: ignoring the default account of {slug:?}: Paneflow tracks accounts only for claude-code, codex and grok"
                ));
                return None;
            };
            let key = agent.config_dir_key();
            let expanded = expand_value(key, dir, home);
            if let Some(reason) = env_value_error(key, &expanded) {
                warn_once(format!(
                    "agents: ignoring the default account {key}={expanded:?}: {reason}"
                ));
                return None;
            }
            Some((platform_key(key), expanded))
        })
        .collect();
    (!env.is_empty()).then_some(env)
}

pub fn configured_default_account_env(config: &PaneFlowConfig) -> Option<HashMap<String, String>> {
    default_account_env(
        &config.agents.as_ref()?.default_accounts,
        dirs::home_dir().as_deref(),
    )
}

pub fn space_env(
    config: &PaneFlowConfig,
    space: Option<&SpaceId>,
) -> Option<HashMap<String, String>> {
    layered_env([
        configured_default_account_env(config),
        declared_space_env(config, space),
    ])
}

pub fn config_dirs(
    space_env: &HashMap<String, String>,
    launch_env: &HashMap<String, String>,
) -> Vec<PathBuf> {
    CONFIG_DIR_KEYS
        .iter()
        .filter(|key| space_env.contains_key(**key))
        .filter_map(|key| launch_env.get(*key))
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .collect()
}

pub fn ensure_dir(dir: &Path) -> std::io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

pub fn ensure_config_dirs(dirs: &[PathBuf]) -> Vec<String> {
    dirs.iter()
        .filter_map(|dir| {
            ensure_dir(dir).err().map(|error| {
                log::warn!(
                    "spaces: could not create the config directory {}: {error:?}",
                    dir.display()
                );
                format!("Could not create {}: {error}", dir.display())
            })
        })
        .collect()
}

pub fn layered_env(
    layers: impl IntoIterator<Item = Option<HashMap<String, String>>>,
) -> Option<HashMap<String, String>> {
    layers
        .into_iter()
        .flatten()
        .map(|layer| {
            layer
                .into_iter()
                .map(|(key, value)| (platform_key(&key), value))
                .collect::<HashMap<_, _>>()
        })
        .reduce(|mut lower, higher| {
            lower.extend(higher);
            lower
        })
}

pub fn is_env_key(key: &str) -> bool {
    !key.is_empty()
        && !key.starts_with(|c: char| c.is_ascii_digit())
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub const SUBPROCESS_ENV_ALLOWLIST: &[&str] = &[
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "GIT_CONFIG_GLOBAL",
    "GIT_SSH_COMMAND",
    "SSH_AUTH_SOCK",
    "GH_CONFIG_DIR",
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
];

pub fn allowlisted(env: &HashMap<String, String>) -> Vec<(String, String)> {
    SUBPROCESS_ENV_ALLOWLIST
        .iter()
        .filter_map(|key| {
            env.get(*key)
                .map(|value| ((*key).to_string(), value.clone()))
        })
        .collect()
}

pub fn subprocess_env(config: &PaneFlowConfig, space: Option<&SpaceId>) -> Vec<(String, String)> {
    space_env(config, space)
        .map(|env| allowlisted(&env))
        .unwrap_or_default()
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
    fn test_layered_env_lets_a_higher_layer_win_whatever_the_key_case() {
        let layered = layered_env([
            Some(HashMap::from([(
                "CODEX_HOME".to_string(),
                "/space".to_string(),
            )])),
            None,
            Some(HashMap::from([(
                "codex_home".to_string(),
                "/profile".to_string(),
            )])),
        ])
        .unwrap();
        assert_eq!(
            layered.get(&platform_key("codex_home")).map(String::as_str),
            Some("/profile")
        );
        assert_eq!(layered.len(), if cfg!(windows) { 1 } else { 2 });
        assert_eq!(layered_env([None, None]), None);
    }

    #[test]
    fn invalid_reserved_protected_and_forbidden_keys_are_refused_with_a_reason() {
        assert_eq!(env_key_error("1ABC"), Some("Not a valid variable name"));
        assert_eq!(env_key_error("A=B"), Some("Not a valid variable name"));
        assert_eq!(env_key_error(""), Some("Not a valid variable name"));
        assert_eq!(
            env_key_error("PANEFLOW_HOME"),
            Some("PANEFLOW_ variables are reserved for Paneflow")
        );
        assert_eq!(
            env_key_error("TERM"),
            Some("Paneflow sets it for every terminal")
        );
        assert_eq!(
            env_key_error("LD_PRELOAD"),
            Some("It could change how programs load or run agents")
        );
        assert_eq!(
            env_key_error("CLAUDECODE"),
            Some("It could change how programs load or run agents")
        );
        assert_eq!(env_key_error("CLAUDE_CONFIG_DIR"), None);
        assert_eq!(env_key_error("GIT_AUTHOR_EMAIL"), None);
    }

    #[test]
    fn config_directories_must_be_absolute_and_other_values_are_free() {
        assert_eq!(
            env_value_error("CODEX_HOME", "codex"),
            Some("Must be an absolute path")
        );
        assert_eq!(
            env_value_error("GH_CONFIG_DIR", "./gh"),
            Some("Must be an absolute path")
        );
        let absolute = std::env::temp_dir().join("claude-work");
        assert_eq!(
            env_value_error("CLAUDE_CONFIG_DIR", &absolute.display().to_string()),
            None
        );
        assert_eq!(env_value_error("GIT_AUTHOR_NAME", "relative"), None);
    }

    #[test]
    fn the_space_env_expands_values_and_drops_refused_entries() {
        let home = std::env::temp_dir().join("space-home");
        let config = config_with(&[
            ("CLAUDE_CONFIG_DIR", "~/.claude-work"),
            ("CODEX_HOME", "relative/codex"),
            ("PANEFLOW_SOCKET_PATH", "/tmp/elsewhere"),
            ("GIT_AUTHOR_NAME", "Work Me"),
        ]);
        let space = config.space(&work()).unwrap();
        let env = resolve_env(&space.id, &space.env, Some(&home));
        assert_eq!(
            env.get("CLAUDE_CONFIG_DIR").map(String::as_str),
            Some(home.join(".claude-work").display().to_string().as_str())
        );
        assert_eq!(
            env.get("GIT_AUTHOR_NAME").map(String::as_str),
            Some("Work Me")
        );
        assert!(!env.contains_key("CODEX_HOME"));
        assert!(!env.contains_key("PANEFLOW_SOCKET_PATH"));
    }

    #[test]
    fn the_default_account_reaches_every_new_pane_unless_the_space_sets_its_own() {
        let absolute =
            |unix: &str, windows: &str| if cfg!(windows) { windows } else { unix }.to_string();
        let home = absolute("/home/a", "C:\\Users\\a");
        let accounts = BTreeMap::from([
            ("claude-code".to_string(), "~/.claude-work".to_string()),
            ("codex".to_string(), "relative/codex".to_string()),
            ("grok".to_string(), "~/.grok-work".to_string()),
            ("gemini".to_string(), "~/.gemini-work".to_string()),
        ]);
        let env = default_account_env(&accounts, Some(Path::new(&home))).unwrap();
        let key = platform_key("CLAUDE_CONFIG_DIR");
        assert_eq!(
            env.len(),
            2,
            "a relative folder and an untracked agent are dropped"
        );
        assert_eq!(
            env.get(&platform_key("GROK_HOME")).map(PathBuf::from),
            Some(Path::new(&home).join(".grok-work"))
        );
        assert_eq!(
            env.get(&key).map(PathBuf::from),
            Some(Path::new(&home).join(".claude-work"))
        );

        let space_dir = absolute("/srv/space-claude", "C:\\srv\\space-claude");
        let default_dir = absolute("/srv/default-claude", "C:\\srv\\default-claude");
        let mut config = config_with(&[("CLAUDE_CONFIG_DIR", &space_dir)]);
        config.agents = Some(paneflow_config::schema::AgentsConfig {
            default_accounts: BTreeMap::from([("claude-code".to_string(), default_dir.clone())]),
            ..Default::default()
        });
        assert_eq!(
            space_env(&config, None).and_then(|env| env.get(&key).cloned()),
            Some(default_dir)
        );
        assert_eq!(
            space_env(&config, Some(&work())).and_then(|env| env.get(&key).cloned()),
            Some(space_dir)
        );
    }

    #[test]
    fn the_default_space_and_an_empty_space_carry_no_env() {
        assert_eq!(space_env(&config_with(&[]), Some(&work())), None);
        let config = config_with(&[("GIT_AUTHOR_NAME", "Work Me")]);
        assert_eq!(space_env(&config, None), None);
        assert!(space_env(&config, Some(&work())).is_some());
    }

    #[test]
    fn only_the_space_config_directories_that_reach_the_launch_are_created() {
        let root = tempfile::tempdir().unwrap();
        let space_dir = root.path().join("work").join(".claude");
        let surface_dir = root.path().join("surface-codex");
        let space = HashMap::from([
            (
                "CLAUDE_CONFIG_DIR".to_string(),
                space_dir.display().to_string(),
            ),
            ("CODEX_HOME".to_string(), "/space/codex".to_string()),
        ]);
        let launch = HashMap::from([
            (
                "CLAUDE_CONFIG_DIR".to_string(),
                space_dir.display().to_string(),
            ),
            ("CODEX_HOME".to_string(), surface_dir.display().to_string()),
            ("GH_CONFIG_DIR".to_string(), "/global/gh".to_string()),
        ]);
        let dirs = config_dirs(&space, &launch);
        assert_eq!(dirs, vec![space_dir.clone(), surface_dir.clone()]);
        assert!(ensure_config_dirs(&dirs).is_empty());
        assert!(space_dir.is_dir() && surface_dir.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&space_dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_that_cannot_be_created_is_named_with_its_error() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let locked = root.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
        let target = locked.join("claude");
        let errors = ensure_config_dirs(std::slice::from_ref(&target));
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        assert_eq!(errors.len(), 1);
        assert!(errors[0].starts_with(&format!("Could not create {}", target.display())));
    }
}
