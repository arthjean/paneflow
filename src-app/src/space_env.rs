use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use paneflow_config::schema::{PaneFlowConfig, SpaceId};

pub(crate) const CONFIG_DIR_KEYS: &[&str] = &["CLAUDE_CONFIG_DIR", "CODEX_HOME", "GH_CONFIG_DIR"];

pub(crate) const SHADOWING_CREDENTIAL_KEYS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "OPENAI_API_KEY",
    "GH_TOKEN",
    "GITHUB_TOKEN",
];

pub(crate) const SUBPROCESS_ENV_ALLOWLIST: &[&str] = &[
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

pub(crate) type SubprocessEnv = std::sync::Arc<[(String, String)]>;

pub(crate) const PLAIN_TEXT_WARNING: &str =
    "Stored in plain text in paneflow.json; prefer a config directory";

pub(crate) const KEPT_ENVIRONMENT_NOTICE: &str = "Open terminals keep their previous environment";

const SECRET_KEY_MARKERS: &[&str] = &["KEY", "TOKEN", "SECRET", "PASSWORD"];

const RESERVED_PREFIX: &str = "PANEFLOW_";

pub(crate) fn platform_key(key: &str) -> String {
    if cfg!(windows) {
        key.to_uppercase()
    } else {
        key.to_string()
    }
}

pub(crate) fn env_key_error(key: &str) -> Option<&'static str> {
    let key = platform_key(key);
    if !paneflow_host::env::is_valid_env_name(&key) || !crate::agent_launcher::is_env_key(&key) {
        return Some("Not a valid variable name");
    }
    if key.starts_with(RESERVED_PREFIX) {
        return Some("PANEFLOW_ variables are reserved for Paneflow");
    }
    if crate::terminal::PROTECTED_ENV_KEYS.contains(&key.as_str()) {
        return Some("Paneflow sets it for every terminal");
    }
    if paneflow_host::env::is_forbidden_child_env_key(&key) {
        return Some("It could change how programs load or run agents");
    }
    None
}

pub(crate) fn env_value_error(key: &str, expanded: &str) -> Option<&'static str> {
    let key = platform_key(key);
    (CONFIG_DIR_KEYS.contains(&key.as_str()) && !Path::new(expanded).is_absolute())
        .then_some("Must be an absolute path")
}

pub(crate) fn stores_secret(key: &str) -> bool {
    let key = key.to_uppercase();
    SECRET_KEY_MARKERS.iter().any(|marker| key.contains(marker))
}

pub(crate) fn shadowing_credentials(
    env: &BTreeMap<String, String>,
    inherited: impl Fn(&str) -> bool,
) -> Vec<&'static str> {
    SHADOWING_CREDENTIAL_KEYS
        .iter()
        .copied()
        .filter(|key| inherited(key))
        .filter(|key| !env.keys().any(|defined| platform_key(defined) == *key))
        .collect()
}

pub(crate) fn inherited_from_paneflow(key: &str) -> bool {
    std::env::var_os(key).is_some_and(|value| !value.is_empty())
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

pub(crate) fn resolve_env(
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

pub(crate) fn space_env(
    config: &PaneFlowConfig,
    space: Option<&SpaceId>,
) -> Option<HashMap<String, String>> {
    let declared = config.space(space?)?;
    let env = resolve_env(&declared.id, &declared.env, dirs::home_dir().as_deref());
    (!env.is_empty()).then_some(env)
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

pub(crate) fn allowlisted(env: &HashMap<String, String>) -> Vec<(String, String)> {
    SUBPROCESS_ENV_ALLOWLIST
        .iter()
        .filter_map(|key| {
            env.get(*key)
                .map(|value| ((*key).to_string(), value.clone()))
        })
        .collect()
}

pub(crate) fn subprocess_env(
    config: &PaneFlowConfig,
    space: Option<&SpaceId>,
) -> Vec<(String, String)> {
    space_env(config, space)
        .map(|env| allowlisted(&env))
        .unwrap_or_default()
}

type DirectoryEnvs = Vec<(PathBuf, SubprocessEnv)>;

#[cfg(not(test))]
fn with_directory_envs<R>(f: impl FnOnce(&mut DirectoryEnvs) -> R) -> R {
    static DIRECTORY_ENVS: std::sync::Mutex<DirectoryEnvs> = std::sync::Mutex::new(Vec::new());
    f(&mut DIRECTORY_ENVS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
}

#[cfg(test)]
fn with_directory_envs<R>(f: impl FnOnce(&mut DirectoryEnvs) -> R) -> R {
    thread_local! {
        static DIRECTORY_ENVS: std::cell::RefCell<DirectoryEnvs> =
            const { std::cell::RefCell::new(Vec::new()) };
    }
    DIRECTORY_ENVS.with(|envs| f(&mut envs.borrow_mut()))
}

pub(crate) fn publish_directory_envs(entries: Vec<(PathBuf, Vec<(String, String)>)>) {
    let entries = if entries.iter().all(|(_, env)| env.is_empty()) {
        Vec::new()
    } else {
        entries
            .into_iter()
            .map(|(dir, env)| (dir, SubprocessEnv::from(env)))
            .collect()
    };
    with_directory_envs(|envs| *envs = entries);
}

pub(crate) fn directory_env(dir: &Path) -> Option<SubprocessEnv> {
    with_directory_envs(|entries| {
        entries
            .iter()
            .filter(|(root, _)| dir.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
            .map(|(_, env)| env.clone())
            .filter(|env| !env.is_empty())
    })
}

pub(crate) fn apply_directory_env(command: &mut std::process::Command) {
    if let Some(env) = command.get_current_dir().and_then(directory_env) {
        command.envs(env.iter().map(|(key, value)| (key, value)));
    }
}

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
    let env = space_env(config, Some(space))?;
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

pub(crate) fn config_dirs(
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

pub(crate) fn ensure_dir(dir: &Path) -> std::io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

pub(crate) fn ensure_config_dirs(dirs: &[PathBuf]) -> Vec<String> {
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
            env: env
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
        });
        config
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
            ("GITHUB_TOKEN".to_string(), String::new()),
        ]);
        let inherited = |key: &str| matches!(key, "GITHUB_TOKEN" | "ANTHROPIC_API_KEY");
        assert_eq!(
            shadowing_credentials(&env, inherited),
            vec!["ANTHROPIC_API_KEY"]
        );
        assert!(shadowing_credentials(&env, |_| false).is_empty());
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
    fn the_default_space_and_an_empty_space_carry_no_env() {
        assert_eq!(space_env(&config_with(&[]), Some(&work())), None);
        let config = config_with(&[("GIT_AUTHOR_NAME", "Work Me")]);
        assert_eq!(space_env(&config, None), None);
        assert!(space_env(&config, Some(&work())).is_some());
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
        let config = config_with(&[
            ("GIT_AUTHOR_EMAIL", "me@work.example"),
            ("GH_CONFIG_DIR", "/work/gh"),
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
                ("GH_CONFIG_DIR".to_string(), "/work/gh".to_string()),
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
        let env_of = |dir: &Path| directory_env(dir).map(|env| env.to_vec());
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
