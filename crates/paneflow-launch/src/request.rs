use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use paneflow_config::schema::{PaneFlowConfig, SessionId, WorkspaceId};
use paneflow_host::{CellSize, CreateSession, SessionAppearance};

use crate::pty_env::{assemble_pty_env, augment_wslenv, is_wsl_shell};
use crate::shell::{resolve_shell_for_spawn, setup_shell_integration};

pub const DEFAULT_SIZE: (usize, usize) = (120, 40);

pub struct LaunchHome {
    pub shell_integration_dir: Option<PathBuf>,
    pub helper_bin_dir: Result<PathBuf, String>,
}

impl LaunchHome {
    pub fn shell_integration_dir_in(data_dir: &Path) -> PathBuf {
        data_dir.join("shell")
    }
}

pub struct LaunchSpec<'a> {
    pub config: &'a PaneFlowConfig,
    pub working_directory: Option<PathBuf>,
    pub workspace_id: u64,
    pub surface_id: u64,
    pub initial_size: Option<(usize, usize)>,
    pub space_env: Option<&'a HashMap<String, String>>,
    pub user_env: Option<HashMap<String, String>>,
    pub socket_path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub shell: String,
    pub extra_args: Vec<String>,
    pub env: HashMap<String, String>,
    pub cwd: PathBuf,
    pub cols: usize,
    pub rows: usize,
}

pub fn resolve(spec: LaunchSpec<'_>, home: LaunchHome) -> (Launch, Option<String>) {
    let LaunchSpec {
        config,
        working_directory,
        workspace_id,
        surface_id,
        initial_size,
        space_env,
        user_env,
        socket_path,
    } = spec;
    let (shell, shell_notice) = {
        let configured = config
            .default_shell
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let (resolved, notice) = resolve_shell_for_spawn(configured);
        log::info!(
            target: "paneflow::terminal::backend",
            "Terminal shell resolved: {resolved:?} (default_shell={configured:?})"
        );
        (resolved, notice)
    };
    let global_env = config.terminal.as_ref().and_then(|t| t.env.clone());
    let merged_env = crate::space::layered_env([global_env, space_env.cloned(), user_env]);
    let mut env = HashMap::new();
    let extra_args = if config.shell_integration.unwrap_or(true) {
        setup_shell_integration(home.shell_integration_dir.as_deref(), &shell, &mut env)
    } else {
        vec![]
    };
    let mut env = assemble_pty_env(
        env,
        workspace_id,
        surface_id,
        socket_path,
        merged_env,
        home.helper_bin_dir,
    );
    if config
        .agents
        .as_ref()
        .is_some_and(paneflow_config::schema::AgentsConfig::resolved_claude_preassign_session_id)
    {
        env.insert(
            paneflow_agent_config::CLAUDE_PREASSIGN_SESSION_ID_ENV.into(),
            "1".into(),
        );
    }
    if is_wsl_shell(&shell) {
        augment_wslenv(&mut env);
    }
    let cwd = working_directory.unwrap_or_else(crate::cwd::implicit_launch_cwd);
    let (cols, rows) = initial_size.unwrap_or(DEFAULT_SIZE);
    (
        Launch {
            shell,
            extra_args,
            env,
            cwd,
            cols,
            rows,
        },
        shell_notice,
    )
}

pub fn create_request(
    launch: &Launch,
    session: SessionId,
    workspace: Option<WorkspaceId>,
    appearance: Option<SessionAppearance>,
    cell: Option<CellSize>,
) -> CreateSession {
    CreateSession {
        session: Some(session),
        workspace,
        cwd: Some(launch.cwd.display().to_string()),
        shell: Some(launch.shell.clone()),
        args: launch.extra_args.clone(),
        env: launch
            .env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<BTreeMap<_, _>>(),
        cols: u16::try_from(launch.cols).ok().filter(|c| *c > 0),
        rows: u16::try_from(launch.rows).ok().filter(|r| *r > 0),
        title: None,
        appearance,
        cell,
        sandbox: false,
        sandbox_unix_scope: false,
        resume: None,
    }
}
