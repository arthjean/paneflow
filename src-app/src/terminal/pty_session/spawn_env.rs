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

impl SpawnParams {
    fn from_launch(launch: paneflow_launch::Launch, profile: TerminalSurfaceProfile) -> Self {
        Self {
            shell_quoting: ShellQuoting::for_shell(&launch.shell),
            shell: launch.shell,
            extra_args: launch.extra_args,
            env: launch.env,
            cwd: launch.cwd,
            cols: launch.cols,
            rows: launch.rows,
            profile,
        }
    }

    pub(in crate::terminal) fn launch(&self) -> paneflow_launch::Launch {
        paneflow_launch::Launch {
            shell: self.shell.clone(),
            extra_args: self.extra_args.clone(),
            env: self.env.clone(),
            cwd: self.cwd.clone(),
            cols: self.cols,
            rows: self.rows,
        }
    }
}

pub(crate) fn paneflow_socket_path() -> Option<String> {
    crate::runtime_paths::socket_path().map(|p| p.display().to_string())
}

pub(in crate::terminal) struct SpawnHome {
    shell_integration_dir: Option<std::path::PathBuf>,
    helper_bin_dir: anyhow::Result<std::path::PathBuf>,
}

impl SpawnHome {
    fn installed() -> Self {
        Self {
            shell_integration_dir: crate::runtime_paths::shell_integration_dir(),
            helper_bin_dir: crate::ai_hooks::extract::ensure_binaries_extracted(),
        }
    }

    #[cfg(test)]
    pub(in crate::terminal) fn at(home: &std::path::Path) -> Self {
        Self {
            shell_integration_dir: Some(crate::runtime_paths::shell_integration_dir_in(home)),
            helper_bin_dir: crate::ai_hooks::extract::extract_binaries_in_home(home),
        }
    }

    fn launch_home(self) -> paneflow_launch::LaunchHome {
        paneflow_launch::LaunchHome {
            shell_integration_dir: self.shell_integration_dir,
            helper_bin_dir: self.helper_bin_dir.map_err(|error| format!("{error:#}")),
        }
    }
}

#[cfg(test)]
fn test_spawn_home() -> std::path::PathBuf {
    std::env::temp_dir().join("paneflow-tests-home")
}

impl TerminalState {
    #[cfg(test)]
    pub(in crate::terminal) fn resolve_spawn_params_with_profile(
        working_directory: Option<std::path::PathBuf>,
        workspace_id: u64,
        surface_id: u64,
        initial_size: Option<(usize, usize)>,
        user_env: Option<std::collections::HashMap<String, String>>,
        profile: TerminalSurfaceProfile,
    ) -> SpawnParams {
        Self::resolve_spawn_launch_in(
            &paneflow_config::loader::load_config(),
            working_directory,
            workspace_id,
            surface_id,
            initial_size,
            None,
            user_env,
            profile,
            SpawnHome::at(&test_spawn_home()),
        )
        .0
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the spawn request fields, each layer of the environment kept apart for its precedence"
    )]
    pub(in crate::terminal) fn resolve_spawn_launch(
        config: &paneflow_config::schema::PaneFlowConfig,
        working_directory: Option<std::path::PathBuf>,
        workspace_id: u64,
        surface_id: u64,
        initial_size: Option<(usize, usize)>,
        space_env: Option<&std::collections::HashMap<String, String>>,
        user_env: Option<std::collections::HashMap<String, String>>,
        profile: TerminalSurfaceProfile,
    ) -> (SpawnParams, Option<String>) {
        Self::resolve_spawn_launch_in(
            config,
            working_directory,
            workspace_id,
            surface_id,
            initial_size,
            space_env,
            user_env,
            profile,
            SpawnHome::installed(),
        )
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the spawn request fields plus the home that owns the helper files"
    )]
    fn resolve_spawn_launch_in(
        config: &paneflow_config::schema::PaneFlowConfig,
        working_directory: Option<std::path::PathBuf>,
        workspace_id: u64,
        surface_id: u64,
        initial_size: Option<(usize, usize)>,
        space_env: Option<&std::collections::HashMap<String, String>>,
        user_env: Option<std::collections::HashMap<String, String>>,
        profile: TerminalSurfaceProfile,
        home: SpawnHome,
    ) -> (SpawnParams, Option<String>) {
        let (launch, shell_notice) = paneflow_launch::resolve(
            paneflow_launch::LaunchSpec {
                config,
                working_directory,
                workspace_id,
                surface_id,
                initial_size,
                space_env,
                user_env,
                socket_path: paneflow_socket_path(),
            },
            home.launch_home(),
        );
        (SpawnParams::from_launch(launch, profile), shell_notice)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    const LAUNCH_CASES_CHILD: &str = "PANEFLOW_LAUNCH_CASES_CHILD";
    const LAUNCH_CASES_ROOT: &str = "PANEFLOW_LAUNCH_CASES_ROOT";
    const LAUNCH_CASES_RECORD: &str = "PANEFLOW_LAUNCH_CASES_RECORD";

    fn launch_cases_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/launch-requests")
    }

    fn fixed_appearance() -> paneflow_host::SessionAppearance {
        paneflow_host::SessionAppearance {
            foreground: [220, 220, 220],
            background: [20, 20, 20],
            cursor: [200, 120, 40],
            palette: (0..=255u8).map(|i| [i, 255 - i, i / 2]).collect(),
            dark: true,
        }
    }

    struct LaunchCase<'a> {
        config: &'a paneflow_config::schema::PaneFlowConfig,
        spawn: crate::workspace::SpawnCwd,
        workspace_id: u64,
        surface_id: u64,
        initial_size: Option<(usize, usize)>,
        space_env: Option<HashMap<String, String>>,
        user_env: Option<HashMap<String, String>>,
        durable: Option<paneflow_config::schema::WorkspaceId>,
        cell: Option<paneflow_host::CellSize>,
    }

    fn recorded_request(root: &Path, case: LaunchCase<'_>) -> String {
        let (mut params, _) = TerminalState::resolve_spawn_launch_in(
            case.config,
            case.spawn.cwd.clone(),
            case.workspace_id,
            case.surface_id,
            case.initial_size,
            case.space_env.as_ref(),
            case.user_env,
            TerminalSurfaceProfile::Normal,
            SpawnHome::at(&root.join("home")),
        );
        if case.spawn.needs_resolving() {
            let requested = params.cwd.clone();
            params.cwd = crate::workspace::SpawnCwd {
                cwd: Some(requested.clone()),
                ..case.spawn
            }
            .resolve(crate::workspace::CONFINE_TIMEOUT)
            .unwrap_or(requested);
        }
        let request = crate::terminal::host_link::AttachRequest {
            intent: crate::terminal::host_link::SessionIntent::Create,
            session: "0190e4d2-7b2a-7c3d-8e4f-a1b2c3d4e5f6"
                .parse()
                .expect("a fixed session id"),
            workspace: case.durable,
            params,
            appearance: fixed_appearance(),
            cell: case.cell,
            route: None,
        };
        let created = crate::terminal::host_link::create_request(&request);
        let mut text = serde_json::to_string_pretty(&created).expect("the request serializes");
        if let Some(socket) = created.env.get("PANEFLOW_SOCKET_PATH") {
            text = text.replace(socket.as_str(), "{socket}");
        }
        let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        text = text.replace(&canonical.display().to_string(), "{root}");
        text = text.replace(&root.display().to_string(), "{root}");
        text.replace(env!("CARGO_PKG_VERSION"), "{version}")
    }

    fn launch_cases(root: &Path) -> Vec<(&'static str, String)> {
        let work = root.join("work");
        let worktree = root.join("worktree");
        let outside = root.join("outside");
        for dir in [&work, &worktree, &outside] {
            std::fs::create_dir_all(dir).expect("case directory");
        }
        let bash = paneflow_config::schema::PaneFlowConfig {
            default_shell: Some("/bin/bash".to_string()),
            ..Default::default()
        };
        let agent_config = paneflow_config::schema::PaneFlowConfig {
            default_shell: Some("/bin/bash".to_string()),
            terminal: Some(paneflow_config::schema::TerminalConfig {
                env: Some(HashMap::from([("EDITOR".to_string(), "vim".to_string())])),
                ..Default::default()
            }),
            agents: Some(paneflow_config::schema::AgentsConfig {
                claude_preassign_session_id: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        let sh = paneflow_config::schema::PaneFlowConfig {
            default_shell: Some("/bin/sh".to_string()),
            shell_integration: Some(false),
            ..Default::default()
        };
        let durable: paneflow_config::schema::WorkspaceId = "0190e4d2-7b2a-7c3d-8e4f-0a0b0c0d0e0f"
            .parse()
            .expect("a fixed workspace id");
        let at = |cwd: &PathBuf| crate::workspace::SpawnCwd {
            cwd: Some(cwd.clone()),
            ..Default::default()
        };
        vec![
            (
                "shell",
                recorded_request(
                    root,
                    LaunchCase {
                        config: &bash,
                        spawn: at(&work),
                        workspace_id: 7,
                        surface_id: 11,
                        initial_size: None,
                        space_env: None,
                        user_env: None,
                        durable: None,
                        cell: None,
                    },
                ),
            ),
            (
                "agent-with-prompt",
                recorded_request(
                    root,
                    LaunchCase {
                        config: &agent_config,
                        spawn: at(&work),
                        workspace_id: 7,
                        surface_id: 12,
                        initial_size: Some((100, 30)),
                        space_env: Some(HashMap::from([(
                            "CLAUDE_CONFIG_DIR".to_string(),
                            root.join("claude").display().to_string(),
                        )])),
                        user_env: Some(HashMap::from([
                            ("ANTHROPIC_MODEL".to_string(), "opus".to_string()),
                            ("PANEFLOW_TMUX_TEAM".to_string(), "alpha".to_string()),
                        ])),
                        durable: Some(durable.clone()),
                        cell: Some(paneflow_host::CellSize {
                            width: 9,
                            height: 19,
                        }),
                    },
                ),
            ),
            (
                "managed-worktree",
                recorded_request(
                    root,
                    LaunchCase {
                        config: &bash,
                        spawn: crate::workspace::SpawnCwd::within(
                            Some(&worktree),
                            Some(outside.clone()),
                        ),
                        workspace_id: 8,
                        surface_id: 13,
                        initial_size: None,
                        space_env: None,
                        user_env: None,
                        durable: Some(durable),
                        cell: None,
                    },
                ),
            ),
            (
                "custom-environment",
                recorded_request(
                    root,
                    LaunchCase {
                        config: &sh,
                        spawn: at(&work),
                        workspace_id: 0,
                        surface_id: 14,
                        initial_size: Some((80, 24)),
                        space_env: None,
                        user_env: Some(HashMap::from([
                            ("TERM".to_string(), "dumb".to_string()),
                            ("PANEFLOW_SURFACE_ID".to_string(), "999".to_string()),
                            ("LD_PRELOAD".to_string(), "/tmp/preload.so".to_string()),
                            ("1BAD".to_string(), "x".to_string()),
                            ("CLAUDECODE".to_string(), "1".to_string()),
                            ("PATH".to_string(), "/opt/tools/bin:/usr/bin".to_string()),
                            ("LANG".to_string(), "fr_FR.UTF-8".to_string()),
                            ("MY_FLAG".to_string(), "on".to_string()),
                        ])),
                        durable: None,
                        cell: None,
                    },
                ),
            ),
        ]
    }

    #[test]
    fn launch_requests_match_the_recorded_cases() {
        if std::env::var_os(LAUNCH_CASES_CHILD).is_some() {
            let root = PathBuf::from(std::env::var_os(LAUNCH_CASES_ROOT).expect("a case root"));
            let record = std::env::var_os(LAUNCH_CASES_RECORD).is_some();
            let dir = launch_cases_dir();
            for (name, request) in launch_cases(&root) {
                let path = dir.join(format!("{name}.json"));
                if record {
                    std::fs::create_dir_all(&dir).expect("fixture directory");
                    std::fs::write(&path, format!("{request}\n")).expect("record the case");
                    continue;
                }
                let expected = std::fs::read_to_string(&path).expect("a recorded case");
                assert_eq!(
                    format!("{request}\n"),
                    expected,
                    "the {name} request differs from {}",
                    path.display()
                );
            }
            return;
        }
        let root = tempfile::tempdir_in("/tmp").expect("case root");
        let user = root.path().join("user");
        std::fs::create_dir_all(&user).expect("case user home");
        let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"));
        child.env_clear();
        for (key, value) in [
            ("PATH", std::ffi::OsString::from("/usr/bin:/bin")),
            ("HOME", user.clone().into_os_string()),
            (LAUNCH_CASES_CHILD, "1".into()),
            (LAUNCH_CASES_ROOT, root.path().as_os_str().to_owned()),
        ] {
            child.env(key, value);
        }
        if std::env::var_os(LAUNCH_CASES_RECORD).is_some() {
            child.env(LAUNCH_CASES_RECORD, "1");
        }
        child.args([
            "terminal::pty_session::spawn_env::tests::launch_requests_match_the_recorded_cases",
            "--exact",
            "--nocapture",
        ]);
        let output = child.output().expect("run the case child");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            stdout.contains("1 passed"),
            "the case child ran no test: {stdout}"
        );
    }
}
