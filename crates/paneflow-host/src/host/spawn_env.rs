use super::*;

pub(super) fn resolve_cwd(requested: Option<&str>) -> PathBuf {
    if let Some(raw) = requested.map(str::trim).filter(|s| !s.is_empty()) {
        let path = PathBuf::from(raw);
        if path.is_dir() {
            return path;
        }
        log::warn!(
            "paneflow-host: requested cwd {raw:?} is not a directory; using the home directory"
        );
    }
    dirs_home().unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

fn dirs_home() -> Option<PathBuf> {
    #[cfg(windows)]
    let raw = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let raw = std::env::var_os("HOME");
    raw.map(PathBuf::from).filter(|p| p.is_dir())
}

pub fn default_shell() -> String {
    let configured = paneflow_config::loader::load_config().default_shell;
    if let Some(shell) = configured
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if let Some(resolved) = usable_executable(shell) {
            return resolved;
        }
        log::warn!("paneflow-host: configured default_shell {shell:?} is not usable; falling back");
    }
    platform_default_shell()
}

fn usable_executable(candidate: &str) -> Option<String> {
    let path = PathBuf::from(candidate);
    if path.is_absolute() {
        return path.is_file().then(|| candidate.to_string());
    }
    which::which(candidate)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

#[cfg(unix)]
fn platform_default_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .and_then(|s| usable_executable(&s))
        .unwrap_or_else(|| "/bin/sh".to_string())
}

#[cfg(windows)]
fn platform_default_shell() -> String {
    ["pwsh.exe", "powershell.exe"]
        .iter()
        .find_map(|name| usable_executable(name))
        .or_else(|| {
            std::env::var("COMSPEC")
                .ok()
                .filter(|s| PathBuf::from(s).is_file())
        })
        .unwrap_or_else(|| "cmd.exe".to_string())
}

pub fn launch_env(
    session: &SessionId,
    generation: SessionGeneration,
    workspace: Option<&WorkspaceId>,
    home: &Path,
    endpoint: &Path,
    helper_dir: Option<&Path>,
    user: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    const PROTECTED: &[&str] = &[
        "TERM",
        "COLORTERM",
        "TERM_PROGRAM",
        "TERM_PROGRAM_VERSION",
        "SHLVL",
        "PANEFLOW_SESSION_ID",
        "PANEFLOW_SESSION_DIR",
        "PANEFLOW_WORKSPACE_UUID",
        "PANEFLOW_HOST_ENDPOINT",
        "PANEFLOW_RUNTIME_GENERATION",
        "PANEFLOW_HOME",
    ];
    let mut env = BTreeMap::new();
    for (key, value) in user {
        #[cfg(windows)]
        let key = key.to_uppercase();
        #[cfg(not(windows))]
        let key = key.clone();
        if !crate::env::is_valid_env_name(&key)
            || crate::env::is_forbidden_child_env_key(&key)
            || crate::env::is_inherited_host_terminal_env_key(&key)
            || PROTECTED.contains(&key.as_str())
        {
            continue;
        }
        env.insert(key, value.clone());
    }
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    env.insert("COLORTERM".to_string(), "truecolor".to_string());
    env.insert("TERM_PROGRAM".to_string(), "ghostty".to_string());
    env.insert(
        "TERM_PROGRAM_VERSION".to_string(),
        paneflow_terminal_ghostty::GHOSTTY_APP_VERSION.to_string(),
    );
    env.insert("SHLVL".to_string(), "0".to_string());
    if std::env::var("LANG").map_or(true, |v| v.is_empty()) {
        env.insert("LANG".to_string(), "en_US.UTF-8".to_string());
    }
    env.insert("PANEFLOW_SESSION_ID".to_string(), session.to_string());
    env.insert(
        "PANEFLOW_SESSION_DIR".to_string(),
        paneflow_home::host_session_data_dir_in(home, session.as_str())
            .display()
            .to_string(),
    );
    if let Some(workspace) = workspace {
        env.insert("PANEFLOW_WORKSPACE_UUID".to_string(), workspace.to_string());
    }
    env.insert(
        "PANEFLOW_HOST_ENDPOINT".to_string(),
        endpoint.display().to_string(),
    );
    env.insert(
        "PANEFLOW_RUNTIME_GENERATION".to_string(),
        generation.to_string(),
    );
    env.insert("PANEFLOW_HOME".to_string(), home.display().to_string());
    if let Some(helper_dir) = helper_dir
        && !env.contains_key("PANEFLOW_BIN_DIR")
    {
        env.insert(
            "PANEFLOW_BIN_DIR".to_string(),
            helper_dir.display().to_string(),
        );
        let inherited = std::env::var("PATH").ok();
        let existing = env.get("PATH").map(String::as_str).or(inherited.as_deref());
        if let Some(path) = crate::helpers::prepend_to_path(existing, helper_dir) {
            env.insert("PATH".to_string(), path);
        }
    }
    env
}
