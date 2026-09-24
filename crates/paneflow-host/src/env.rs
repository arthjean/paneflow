pub const INHERITED_AGENT_SESSION_ENV: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
];

pub const INHERITED_HOST_TERMINAL_ENV: &[&str] = &[
    "WT_SESSION",
    "WT_PROFILE_ID",
    "TMUX",
    "TMUX_PANE",
    "STY",
    "ZELLIJ",
    "ZELLIJ_SESSION_NAME",
    "ZELLIJ_PANE_ID",
    "KITTY_WINDOW_ID",
    "KITTY_LISTEN_ON",
    "TERMINAL_EMULATOR",
    "VTE_VERSION",
    "ITERM_SESSION_ID",
    "LC_TERMINAL",
    "LC_TERMINAL_VERSION",
    "ALACRITTY_WINDOW_ID",
    "ALACRITTY_SOCKET",
];

pub const PANE_CONTEXT_ENV: &[&str] = &[
    "PANEFLOW_SURFACE_ID",
    "PANEFLOW_WORKSPACE_ID",
    "PANEFLOW_WORKSPACE_UUID",
    "PANEFLOW_SESSION_ID",
    "PANEFLOW_SESSION_DIR",
    "PANEFLOW_HOST_ENDPOINT",
    "PANEFLOW_RUNTIME_GENERATION",
    "PANEFLOW_BIN_DIR",
    "PANEFLOW_AI_TOOL",
    "PANEFLOW_AI_PID",
];

const CONEMU_ENV_PREFIX: &str = "conemu";

pub fn is_loader_influencing_env_key(key: &str) -> bool {
    key.starts_with("LD_") || key.starts_with("DYLD_")
}

pub fn is_inherited_agent_session_env_key(key: &str) -> bool {
    INHERITED_AGENT_SESSION_ENV.contains(&key)
        || paneflow_agent_config::RUNTIMES
            .iter()
            .any(|runtime| runtime.environment.strip_inherited.contains(&key))
}

pub fn is_forbidden_child_env_key(key: &str) -> bool {
    is_inherited_agent_session_env_key(key) || is_loader_influencing_env_key(key)
}

pub fn is_inherited_host_terminal_env_key(key: &str) -> bool {
    INHERITED_HOST_TERMINAL_ENV
        .iter()
        .any(|known| key.eq_ignore_ascii_case(known))
        || key.len() > CONEMU_ENV_PREFIX.len()
            && key[..CONEMU_ENV_PREFIX.len()].eq_ignore_ascii_case(CONEMU_ENV_PREFIX)
}

pub fn inherited_env_keys_to_strip() -> Vec<std::ffi::OsString> {
    std::env::vars_os()
        .map(|(key, _)| key)
        .filter(|key| {
            key.to_str().is_some_and(|key| {
                is_inherited_host_terminal_env_key(key) || is_inherited_agent_session_env_key(key)
            })
        })
        .collect()
}

pub fn is_valid_env_name(key: &str) -> bool {
    !key.is_empty() && !key.contains('=') && !key.contains('\0')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_terminal_markers_are_recognized_whatever_their_casing() {
        for key in INHERITED_HOST_TERMINAL_ENV {
            assert!(is_inherited_host_terminal_env_key(key));
            assert!(is_inherited_host_terminal_env_key(&key.to_lowercase()));
        }
        for key in ["ConEmuANSI", "ConEmuPID", "ConEmuTask", "CONEMUBUILD"] {
            assert!(is_inherited_host_terminal_env_key(key));
        }
    }

    #[test]
    fn host_terminal_matcher_does_not_swallow_unrelated_names() {
        for key in [
            "conemu",
            "CONEMU",
            "TERM",
            "TERM_PROGRAM",
            "TMUXINATOR_CONFIG",
            "STYLE",
            "PATH",
            "KITTY_WINDOW_IDS",
            "PANEFLOW_SURFACE_ID",
            "PANEFLOW_SESSION_ID",
        ] {
            assert!(!is_inherited_host_terminal_env_key(key), "{key}");
        }
    }

    #[test]
    fn the_strip_list_covers_both_families_it_claims_to() {
        for key in INHERITED_AGENT_SESSION_ENV {
            assert!(
                is_inherited_agent_session_env_key(key) || is_inherited_host_terminal_env_key(key)
            );
            assert!(is_forbidden_child_env_key(key));
        }
        for runtime in paneflow_agent_config::RUNTIMES {
            for key in runtime.environment.strip_inherited {
                assert!(
                    is_forbidden_child_env_key(key),
                    "{} declares {key} in strip_inherited",
                    runtime.slug
                );
            }
        }
        assert!(is_forbidden_child_env_key("LD_PRELOAD"));
        assert!(is_forbidden_child_env_key("DYLD_INSERT_LIBRARIES"));
        assert!(!is_forbidden_child_env_key("PANEFLOW_SESSION_ID"));
        assert!(!is_inherited_host_terminal_env_key("PANEFLOW_SESSION_ID"));
    }

    #[test]
    fn env_names_reject_separators_and_nul() {
        assert!(is_valid_env_name("KEEP_ME"));
        assert!(!is_valid_env_name(""));
        assert!(!is_valid_env_name("A=B"));
        assert!(!is_valid_env_name("A\0B"));
    }
}
