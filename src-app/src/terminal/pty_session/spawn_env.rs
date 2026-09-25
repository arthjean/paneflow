use super::*;

#[derive(Clone)]
pub(in crate::terminal) struct SpawnParams {
    pub(in crate::terminal) shell: String,
    pub(in crate::terminal) shell_quoting: ShellQuoting,
    pub(in crate::terminal) extra_args: Vec<String>,
    pub(in crate::terminal) env: std::collections::HashMap<String, String>,
    pub(in crate::terminal) cwd: std::path::PathBuf,
    pub(in crate::terminal) cols: usize,
    pub(in crate::terminal) rows: usize,
    pub(in crate::terminal) profile: TerminalSurfaceProfile,
}

fn paneflow_socket_path() -> Option<String> {
    crate::runtime_paths::socket_path().map(|p| p.display().to_string())
}

fn inject_ai_hook_env(env: &mut std::collections::HashMap<String, String>) {
    let bin_dir = match crate::ai_hooks::extract::ensure_binaries_extracted() {
        Ok(p) => p,
        Err(e) => {
            log::warn!(
                "paneflow: AI-hook binary extraction failed ({e:#}); sidebar loader will not activate for this terminal session"
            );
            return;
        }
    };

    env.insert("PANEFLOW_BIN_DIR".into(), bin_dir.display().to_string());

    prepend_bin_dir_to_path(env, &bin_dir);
}

fn reassert_paneflow_bin_dir_first(env: &mut std::collections::HashMap<String, String>) {
    let Some(bin_dir) = env.get("PANEFLOW_BIN_DIR").cloned() else {
        return;
    };
    if bin_dir.is_empty() {
        return;
    }
    prepend_bin_dir_to_path(env, std::path::Path::new(&bin_dir));
}

fn prepend_bin_dir_to_path(
    env: &mut std::collections::HashMap<String, String>,
    bin_dir: &std::path::Path,
) {
    let existing: Option<std::ffi::OsString> = env
        .get("PATH")
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("PATH"));

    let mut components: Vec<std::path::PathBuf> = vec![bin_dir.to_path_buf()];
    if let Some(existing) = existing.as_deref()
        && !existing.is_empty()
    {
        components.extend(std::env::split_paths(existing));
    }

    match std::env::join_paths(components) {
        Ok(joined) => {
            env.insert("PATH".into(), joined.to_string_lossy().into_owned());
        }
        Err(e) => {
            log::warn!(
                "paneflow: could not prepend AI-hook bin dir {} to PATH: {e}",
                bin_dir.display()
            );
        }
    }
}

fn is_wsl_shell(shell: &str) -> bool {
    let executable = shell.rsplit(['/', '\\']).next().unwrap_or(shell);
    executable.eq_ignore_ascii_case("wsl.exe") || executable.eq_ignore_ascii_case("wsl")
}

fn is_wslenv_identifier(key: &str) -> bool {
    let mut bytes = key.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn wslenv_entry_covers(entry: &str, key: &str, requires_path_translation: bool) -> bool {
    let (name, flags) = entry.split_once('/').unwrap_or((entry, ""));
    name == key
        && (!flags.contains('w') || flags.contains('u'))
        && (!requires_path_translation || flags.contains('p'))
}

fn merge_wslenv<'a>(
    initial: Option<&str>,
    env_keys: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    let existing_entries = initial
        .map(|value| value.split(':').collect::<Vec<_>>())
        .unwrap_or_default();
    let mut keys = env_keys
        .into_iter()
        .filter(|key| {
            is_wslenv_identifier(key) && !matches!(*key, "PATH" | "WSLENV" | "SHLVL" | "LANG")
        })
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys.dedup();

    let additions = keys
        .into_iter()
        .filter_map(|key| {
            let requires_path_translation = matches!(key, "PANEFLOW_BIN_DIR" | "PANEFLOW_HOOK_LOG");
            if existing_entries
                .iter()
                .any(|entry| wslenv_entry_covers(entry, key, requires_path_translation))
            {
                None
            } else if requires_path_translation {
                Some(format!("{key}/up"))
            } else {
                Some(format!("{key}/u"))
            }
        })
        .collect::<Vec<_>>();

    if additions.is_empty() {
        return initial.map(str::to_owned);
    }

    let additions = additions.join(":");
    Some(match initial.filter(|value| !value.is_empty()) {
        Some(initial) => format!("{initial}:{additions}"),
        None => additions,
    })
}

fn augment_wslenv(env: &mut std::collections::HashMap<String, String>) {
    let initial = env
        .get("WSLENV")
        .cloned()
        .or_else(|| std::env::var("WSLENV").ok());
    if let Some(merged) = merge_wslenv(initial.as_deref(), env.keys().map(String::as_str)) {
        env.insert("WSLENV".into(), merged);
    }
}

fn assemble_pty_env(
    mut env: std::collections::HashMap<String, String>,
    workspace_id: u64,
    surface_id: u64,
    user_env: Option<std::collections::HashMap<String, String>>,
) -> std::collections::HashMap<String, String> {
    if workspace_id != 0 {
        env.insert("PANEFLOW_WORKSPACE_ID".into(), workspace_id.to_string());
    }
    env.insert("PANEFLOW_SURFACE_ID".into(), surface_id.to_string());
    if let Some(socket_path) = paneflow_socket_path() {
        env.insert("PANEFLOW_SOCKET_PATH".into(), socket_path);
    }

    if let Some(log_path) = std::env::var_os("PANEFLOW_HOOK_LOG")
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string_lossy().into_owned())
    {
        env.insert("PANEFLOW_HOOK_LOG".into(), log_path);
    }

    env.insert("TERM".into(), "xterm-256color".into());

    if std::env::var("LANG").map_or(true, |v| v.is_empty()) {
        env.insert("LANG".into(), "en_US.UTF-8".into());
    }

    env.insert("TERM_PROGRAM".into(), "paneflow".into());
    env.insert(
        "TERM_PROGRAM_VERSION".into(),
        env!("CARGO_PKG_VERSION").into(),
    );
    env.insert("COLORTERM".into(), "truecolor".into());

    env.insert("SHLVL".into(), "0".into());

    inject_ai_hook_env(&mut env);

    if let Some(user_vars) = user_env {
        const PROTECTED: &[&str] = &[
            "TERM",
            "COLORTERM",
            "TERM_PROGRAM",
            "TERM_PROGRAM_VERSION",
            "SHLVL",
            "PANEFLOW_WORKSPACE_ID",
            "PANEFLOW_SURFACE_ID",
            "PANEFLOW_SOCKET_PATH",
            "PANEFLOW_BIN_DIR",
        ];
        for (k, v) in user_vars {
            #[cfg(windows)]
            let k = k.to_uppercase();
            if !is_valid_env_name(&k) || is_forbidden_child_env_key(&k) {
                continue;
            }
            if PROTECTED.contains(&k.as_str()) {
                continue;
            }
            env.insert(k, v);
        }
    }

    env.retain(|k, _| !is_inherited_agent_session_env_key(k));
    reassert_paneflow_bin_dir_first(&mut env);

    env
}

impl TerminalState {
    #[cfg(test)]
    pub(in crate::terminal) fn resolve_spawn_params(
        working_directory: Option<std::path::PathBuf>,
        workspace_id: u64,
        surface_id: u64,
        initial_size: Option<(usize, usize)>,
        user_env: Option<std::collections::HashMap<String, String>>,
    ) -> SpawnParams {
        Self::resolve_spawn_params_with_profile(
            working_directory,
            workspace_id,
            surface_id,
            initial_size,
            user_env,
            TerminalSurfaceProfile::Normal,
        )
    }

    pub(in crate::terminal) fn resolve_spawn_params_with_profile(
        working_directory: Option<std::path::PathBuf>,
        workspace_id: u64,
        surface_id: u64,
        initial_size: Option<(usize, usize)>,
        user_env: Option<std::collections::HashMap<String, String>>,
        profile: TerminalSurfaceProfile,
    ) -> SpawnParams {
        let config = paneflow_config::loader::load_config();
        let shell = {
            let configured = config
                .default_shell
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let resolved = resolve_default_shell(configured);
            log::info!(
                target: "paneflow::terminal::backend",
                "Terminal shell resolved: {resolved:?} (default_shell={configured:?})"
            );
            resolved
        };
        let shell_quoting = ShellQuoting::for_shell(&shell);
        let global_env = config.terminal.as_ref().and_then(|t| t.env.clone());
        let merged_env = match (global_env, user_env) {
            (None, None) => None,
            (Some(g), None) => Some(g),
            (None, Some(s)) => Some(s),
            (Some(mut g), Some(s)) => {
                g.extend(s);
                Some(g)
            }
        };
        let mut env = std::collections::HashMap::new();
        let extra_args = if config.shell_integration.unwrap_or(true) {
            setup_shell_integration(&shell, &mut env)
        } else {
            vec![]
        };
        let mut env = assemble_pty_env(env, workspace_id, surface_id, merged_env);
        if is_wsl_shell(&shell) {
            augment_wslenv(&mut env);
        }
        let cwd = working_directory.unwrap_or_else(crate::launch_cwd::implicit_launch_cwd);
        let (cols, rows) = initial_size.unwrap_or((120, 40));
        SpawnParams {
            shell,
            shell_quoting,
            extra_args,
            env,
            cwd,
            cols,
            rows,
            profile,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    fn platform_sep() -> char {
        if cfg!(windows) { ';' } else { ':' }
    }

    #[test]
    fn resolve_spawn_params_honors_initial_size() {
        let p = TerminalState::resolve_spawn_params(None, 1, 1, Some((100, 30)), None);
        assert_eq!((p.cols, p.rows), (100, 30));
        let d = TerminalState::resolve_spawn_params(None, 1, 1, None, None);
        assert_eq!((d.cols, d.rows), (120, 40));
    }

    #[test]
    fn prepend_puts_bin_dir_first_and_preserves_existing_entries() {
        let mut env: HashMap<String, String> = HashMap::new();
        let sep = platform_sep();
        env.insert("PATH".into(), format!("/usr/bin{sep}/usr/local/bin"));

        let bin_dir = PathBuf::from("/home/u/.cache/paneflow/bin/0.2.6");
        prepend_bin_dir_to_path(&mut env, &bin_dir);

        let joined = env.get("PATH").expect("PATH set by helper");
        let components: Vec<PathBuf> = std::env::split_paths(joined).collect();
        assert_eq!(
            components.first(),
            Some(&bin_dir),
            "US-009 AC: bin_dir must be first on PATH; got {components:?}"
        );
        assert!(
            components.iter().any(|p| p == Path::new("/usr/bin")),
            "US-009: original PATH entries must be preserved; got {components:?}"
        );
        assert!(
            components.iter().any(|p| p == Path::new("/usr/local/bin")),
            "US-009: original PATH entries must be preserved; got {components:?}"
        );
    }

    #[test]
    fn prepend_inserts_bin_dir_even_when_env_path_absent() {
        let mut env: HashMap<String, String> = HashMap::new();
        let bin_dir = PathBuf::from("/tmp/paneflow-bins");
        prepend_bin_dir_to_path(&mut env, &bin_dir);

        let joined = env.get("PATH").expect("PATH set by helper");
        let components: Vec<PathBuf> = std::env::split_paths(joined).collect();
        assert_eq!(
            components.first(),
            Some(&bin_dir),
            "US-009: bin_dir must be first on PATH in the no-prior-PATH case"
        );
    }

    #[test]
    fn prepend_uses_platform_separator() {
        let mut env: HashMap<String, String> = HashMap::new();
        let sep = platform_sep();
        env.insert("PATH".into(), format!("/a{sep}/b{sep}/c"));
        let bin_dir = PathBuf::from("/z");
        prepend_bin_dir_to_path(&mut env, &bin_dir);

        let joined = env.get("PATH").unwrap();
        let components: Vec<PathBuf> = std::env::split_paths(joined).collect();
        assert_eq!(
            components,
            vec![
                PathBuf::from("/z"),
                PathBuf::from("/a"),
                PathBuf::from("/b"),
                PathBuf::from("/c"),
            ],
            "US-009: split_paths(join_paths(...)) must round-trip on all platforms"
        );
    }

    #[test]
    fn prepend_treats_empty_path_as_absent() {
        let mut env: HashMap<String, String> = HashMap::new();
        env.insert("PATH".into(), String::new());
        let bin_dir = PathBuf::from("/z");
        prepend_bin_dir_to_path(&mut env, &bin_dir);

        let joined = env.get("PATH").expect("PATH set by helper");
        let components: Vec<PathBuf> = std::env::split_paths(joined).collect();
        assert!(
            !components.iter().any(|p| p.as_os_str().is_empty()),
            "US-009 hardening: empty PATH must not yield a phantom CWD entry; got {components:?}"
        );
        assert_eq!(
            components.first(),
            Some(&bin_dir),
            "US-009: bin_dir must still be first when empty PATH is treated as absent"
        );
    }

    #[test]
    fn wslenv_merge_preserves_existing_entries_and_deduplicates() {
        let merged = merge_wslenv(
            Some("EXISTING/p:ALREADY/u:CUSTOM/uw"),
            ["ZED", "ALREADY", "EXISTING", "ZED", "PANEFLOW_BIN_DIR"],
        );

        assert_eq!(
            merged.as_deref(),
            Some("EXISTING/p:ALREADY/u:CUSTOM/uw:PANEFLOW_BIN_DIR/up:ZED/u")
        );
    }

    #[test]
    fn wslenv_merge_adds_u_when_w_is_one_way() {
        let merged = merge_wslenv(
            Some("FORWARD_ONLY/w:UNCHANGED/l"),
            ["FORWARD_ONLY", "UNCHANGED"],
        );

        assert_eq!(
            merged.as_deref(),
            Some("FORWARD_ONLY/w:UNCHANGED/l:FORWARD_ONLY/u")
        );
    }

    #[test]
    fn wslenv_merge_adds_up_when_paneflow_paths_lack_path_flag() {
        let merged = merge_wslenv(
            Some("PANEFLOW_HOOK_LOG/u:PANEFLOW_BIN_DIR/u"),
            ["PANEFLOW_HOOK_LOG", "PANEFLOW_BIN_DIR"],
        );

        assert_eq!(
            merged.as_deref(),
            Some("PANEFLOW_HOOK_LOG/u:PANEFLOW_BIN_DIR/u:PANEFLOW_BIN_DIR/up:PANEFLOW_HOOK_LOG/up")
        );
    }

    #[test]
    fn wslenv_merge_skips_excluded_and_invalid_names() {
        let merged = merge_wslenv(
            None,
            [
                "PATH",
                "WSLENV",
                "SHLVL",
                "LANG",
                "9INVALID",
                "HAS-DASH",
                "NON_ASCII_é",
                "",
                "_ALSO_2",
                "GOOD_VAR",
            ],
        );

        assert_eq!(merged.as_deref(), Some("GOOD_VAR/u:_ALSO_2/u"));
    }

    #[test]
    fn wslenv_shell_detection_is_exact() {
        assert!(is_wsl_shell("wsl"));
        assert!(is_wsl_shell("WSL.EXE"));
        assert!(is_wsl_shell(r"C:\Windows\System32\wsl.exe"));
        assert!(!is_wsl_shell("pwsh.exe"));
        assert!(!is_wsl_shell("my-wsl.exe"));
    }

    #[test]
    fn pty_spawn_injects_paneflow_bin_dir_and_prepends_path() {
        if dirs::cache_dir().is_none() {
            eprintln!("skip: dirs::cache_dir() unresolvable in this environment");
            return;
        }

        let env = assemble_pty_env(HashMap::new(), 7, 3, None);

        let bin_dir = env
            .get("PANEFLOW_BIN_DIR")
            .expect("US-009 AC: PANEFLOW_BIN_DIR must be set in the child env")
            .clone();
        assert!(
            !bin_dir.is_empty(),
            "US-009: PANEFLOW_BIN_DIR must not be empty"
        );

        let path = env
            .get("PATH")
            .expect("US-009 AC: PATH must be set after injection");
        let first = std::env::split_paths(path)
            .next()
            .expect("PATH must have at least one component");
        assert_eq!(
            first,
            PathBuf::from(&bin_dir),
            "US-009 AC: PANEFLOW_BIN_DIR must be first on PATH"
        );
    }

    #[test]
    fn detached_terminal_does_not_advertise_fake_workspace_id() {
        let env = assemble_pty_env(HashMap::new(), 0, 3, None);

        assert!(
            !env.contains_key("PANEFLOW_WORKSPACE_ID"),
            "workspace id 0 is a detached sentinel and must not reach child hooks"
        );
        assert_eq!(
            env.get("PANEFLOW_SURFACE_ID").map(String::as_str),
            Some("3")
        );
    }

    #[test]
    fn user_env_is_merged_into_pty_env() {
        let mut user = HashMap::new();
        user.insert("ANTHROPIC_API_KEY".to_string(), "sk-test-123".to_string());
        user.insert("MY_CUSTOM_VAR".to_string(), "hello".to_string());
        let env = assemble_pty_env(HashMap::new(), 1, 1, Some(user));

        assert_eq!(
            env.get("ANTHROPIC_API_KEY").map(String::as_str),
            Some("sk-test-123"),
            "US-014 AC: user env var must be present in the child env"
        );
        assert_eq!(
            env.get("MY_CUSTOM_VAR").map(String::as_str),
            Some("hello"),
            "US-014 AC: a second user env var must also be present"
        );
    }

    #[test]
    fn user_path_cannot_shadow_paneflow_bin_dir() {
        let mut user = HashMap::new();
        user.insert("PATH".to_string(), "/custom/bin".to_string());
        let env = assemble_pty_env(HashMap::new(), 1, 1, Some(user));
        let Some(bin_dir) = env.get("PANEFLOW_BIN_DIR") else {
            eprintln!("skip: PANEFLOW_BIN_DIR unavailable in this environment");
            return;
        };
        let path = env.get("PATH").expect("PATH must be present");
        let mut parts = std::env::split_paths(path);
        assert_eq!(
            parts.next().as_deref(),
            Some(std::path::Path::new(bin_dir)),
            "PANEFLOW_BIN_DIR must stay first even when user env sets PATH"
        );
        assert!(
            parts.any(|part| part == std::path::Path::new("/custom/bin")),
            "user PATH entries must still be preserved after the shim prepend"
        );
    }

    #[test]
    fn protected_keys_cannot_be_overridden_by_user_env() {
        let mut user = HashMap::new();
        user.insert("TERM".to_string(), "dumb".to_string());
        user.insert("COLORTERM".to_string(), "nope".to_string());
        user.insert("TERM_PROGRAM".to_string(), "spoofed".to_string());
        user.insert("TERM_PROGRAM_VERSION".to_string(), "0.0.0".to_string());
        user.insert("SHLVL".to_string(), "99".to_string());
        user.insert("KEEP_ME".to_string(), "yes".to_string());
        let env = assemble_pty_env(HashMap::new(), 1, 1, Some(user));

        assert_eq!(
            env.get("TERM").map(String::as_str),
            Some("xterm-256color"),
            "US-014 AC: TERM must stay Paneflow-owned even if the user sets it"
        );
        assert_eq!(
            env.get("COLORTERM").map(String::as_str),
            Some("truecolor"),
            "US-014 AC: COLORTERM must stay Paneflow-owned even if the user sets it"
        );
        assert_eq!(
            env.get("TERM_PROGRAM").map(String::as_str),
            Some("paneflow"),
            "TERM_PROGRAM must stay Paneflow-owned even if the user sets it"
        );
        assert_eq!(
            env.get("TERM_PROGRAM_VERSION").map(String::as_str),
            Some(env!("CARGO_PKG_VERSION")),
            "TERM_PROGRAM_VERSION must stay Paneflow-owned even if the user sets it"
        );
        assert_eq!(
            env.get("SHLVL").map(String::as_str),
            Some("0"),
            "SHLVL must stay reset so the child shell starts at level 1"
        );
        assert_eq!(
            env.get("KEEP_ME").map(String::as_str),
            Some("yes"),
            "US-014: a non-protected user var alongside protected ones still wins"
        );
    }

    #[test]
    fn loader_influencing_env_vars_are_dropped() {
        let mut user = HashMap::new();
        user.insert("LD_PRELOAD".to_string(), "/tmp/evil.so".to_string());
        user.insert("LD_LIBRARY_PATH".to_string(), "/tmp/evil".to_string());
        user.insert("LD_AUDIT".to_string(), "/tmp/audit.so".to_string());
        user.insert(
            "DYLD_INSERT_LIBRARIES".to_string(),
            "/tmp/e.dylib".to_string(),
        );
        user.insert("KEEP_ME".to_string(), "yes".to_string());
        let env = assemble_pty_env(HashMap::new(), 1, 1, Some(user));

        assert_eq!(
            env.get("LD_PRELOAD"),
            None,
            "f010: LD_PRELOAD from untrusted env must be dropped"
        );
        assert_eq!(
            env.get("LD_LIBRARY_PATH"),
            None,
            "f010: LD_LIBRARY_PATH from untrusted env must be dropped"
        );
        assert_eq!(
            env.get("LD_AUDIT"),
            None,
            "f010: LD_AUDIT from untrusted env must be dropped"
        );
        assert_eq!(
            env.get("DYLD_INSERT_LIBRARIES"),
            None,
            "f010: DYLD_* from untrusted env must be dropped"
        );
        assert_eq!(
            env.get("KEEP_ME").map(String::as_str),
            Some("yes"),
            "f010: a benign var alongside loader vars must still pass through"
        );
    }

    #[test]
    fn claudecode_env_is_dropped_from_child_env() {
        let mut base = HashMap::new();
        base.insert("CLAUDECODE".to_string(), "1".to_string());
        let mut user = HashMap::new();
        user.insert("CLAUDECODE".to_string(), "1".to_string());
        user.insert("KEEP_ME".to_string(), "yes".to_string());

        let env = assemble_pty_env(base, 1, 1, Some(user));

        assert_eq!(
            env.get("CLAUDECODE"),
            None,
            "CLAUDECODE must never reach agent child processes"
        );
        assert_eq!(env.get("KEEP_ME").map(String::as_str), Some("yes"));
    }

    #[test]
    fn inherited_agent_session_markers_are_dropped_from_child_env() {
        let mut base = HashMap::new();
        let mut user = HashMap::new();
        for key in INHERITED_AGENT_SESSION_ENV {
            base.insert((*key).to_string(), "inherited".to_string());
            user.insert((*key).to_string(), "from-config".to_string());
        }
        user.insert("KEEP_ME".to_string(), "yes".to_string());

        let env = assemble_pty_env(base, 1, 1, Some(user));

        for key in INHERITED_AGENT_SESSION_ENV {
            assert_eq!(
                env.get(*key),
                None,
                "{key} must never reach an agent spawned in a pane"
            );
        }
        assert_eq!(
            env.get("KEEP_ME").map(String::as_str),
            Some("yes"),
            "a benign var alongside the markers must still pass through"
        );
    }

    #[test]
    fn host_terminal_markers_are_not_smuggled_through_the_assembled_env() {
        let env = assemble_pty_env(HashMap::new(), 1, 1, None);
        for key in env.keys() {
            assert!(
                !paneflow_host::env::is_inherited_host_terminal_env_key(key),
                "assemble_pty_env must never introduce the host marker {key}"
            );
        }
        assert_eq!(
            env.get("TERM_PROGRAM").map(String::as_str),
            Some("paneflow")
        );
    }
}
