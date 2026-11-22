use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use paneflow_config::schema::{SessionId, WorkspaceId};
use paneflow_host::protocol::{ClientHello, ERR_LAUNCH_PENDING, ERR_SESSION_NOT_FOUND};
use paneflow_host::{CellSize, CreateSession, HostClient, SessionAppearance};
use paneflow_launch::cwd::{CONFINE_TIMEOUT, SpawnCwd};
use paneflow_layout::{State, SurfaceId, SurfaceKind};

const HOST_CLIENT: &str = "paneflow-serve-launch";
const NO_HELPERS: &str = "this process carries no helper binaries";

#[derive(Clone)]
pub struct LaunchSupport {
    pub helper_bin_dir: Result<PathBuf, String>,
}

impl Default for LaunchSupport {
    fn default() -> Self {
        Self {
            helper_bin_dir: Err(NO_HELPERS.to_string()),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LaunchInputs {
    pub cwd: Option<PathBuf>,
    pub confine_to: Option<PathBuf>,
    pub fallback: Option<PathBuf>,
    pub env: Option<HashMap<String, String>>,
    pub size: Option<(usize, usize)>,
    pub appearance: Option<SessionAppearance>,
    pub cell: Option<CellSize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreparedLaunch {
    pub request: CreateSession,
    pub notices: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    Unreachable(String),
    Refused(String),
}

impl LaunchError {
    pub fn message(&self) -> &str {
        match self {
            Self::Unreachable(message) | Self::Refused(message) => message,
        }
    }
}

pub trait SessionLink {
    fn exists(&mut self, session: &SessionId) -> Result<bool, LaunchError>;
    fn create(&mut self, request: &CreateSession) -> Result<(), LaunchError>;
}

pub trait SessionStarter: Send + Sync {
    fn connect(&self) -> Result<Box<dyn SessionLink>, LaunchError>;
}

pub struct HostStarter {
    endpoint: PathBuf,
}

impl HostStarter {
    pub fn shared(endpoint: PathBuf) -> Arc<dyn SessionStarter> {
        Arc::new(Self { endpoint })
    }
}

struct HostLink(HostClient);

impl SessionStarter for HostStarter {
    fn connect(&self) -> Result<Box<dyn SessionLink>, LaunchError> {
        HostClient::connect(&self.endpoint, &ClientHello::local(HOST_CLIENT))
            .map(|client| Box::new(HostLink(client)) as Box<dyn SessionLink>)
            .map_err(|error| LaunchError::Unreachable(error.to_string()))
    }
}

impl SessionLink for HostLink {
    fn exists(&mut self, session: &SessionId) -> Result<bool, LaunchError> {
        match self.0.inspect(session) {
            Ok(_) => Ok(true),
            Err(error) if error.code() == Some(ERR_SESSION_NOT_FOUND) => Ok(false),
            Err(error) if error.is_connection_loss() => {
                Err(LaunchError::Unreachable(error.to_string()))
            }
            Err(error) => Err(LaunchError::Refused(error.to_string())),
        }
    }

    fn create(&mut self, request: &CreateSession) -> Result<(), LaunchError> {
        match self.0.create(request) {
            Ok(_) => Ok(()),
            Err(error) if error.code() == Some(ERR_LAUNCH_PENDING) => Ok(()),
            Err(error) if error.is_connection_loss() => {
                Err(LaunchError::Unreachable(error.to_string()))
            }
            Err(error) => Err(LaunchError::Refused(error.to_string())),
        }
    }
}

pub fn surface_needs_orchestration(
    definition: &paneflow_config::schema::SurfaceDefinition,
) -> bool {
    let non_empty = |value: &Option<String>| value.as_deref().is_some_and(|text| !text.is_empty());
    definition.env.as_ref().is_some_and(|env| !env.is_empty())
        || non_empty(&definition.command)
        || non_empty(&definition.prompt)
}

pub fn socket_path(home: &Path) -> Option<String> {
    let process_home = paneflow_home::paneflow_home().is_some_and(|own| {
        paneflow_home::home_fingerprint(&own) == paneflow_home::home_fingerprint(home)
    });
    let path = if process_home {
        paneflow_home::ipc_endpoint_in(&paneflow_home::EndpointEnv::from_process())
            .map_or_else(|| paneflow_home::ipc_endpoint_path(home), |spec| spec.path)
    } else {
        paneflow_home::ipc_endpoint_path(home)
    };
    Some(path.display().to_string())
}

pub fn inputs_from_model(state: &State, surface: SurfaceId) -> Option<LaunchInputs> {
    let location = state.find_surface(surface)?;
    let ws = state.workspaces.get(location.workspace_index)?;
    let tab = ws.tabs.get(location.tab_index)?;
    let definition = &state.surface(surface)?.definition;
    let requested = definition
        .cwd
        .clone()
        .filter(|cwd| !cwd.is_empty())
        .map(PathBuf::from);
    let workspace_cwd = (!ws.cwd.is_empty()).then(|| PathBuf::from(&ws.cwd));
    let spawn = match tab.worktree.as_deref() {
        Some(worktree) => SpawnCwd::within(Some(Path::new(worktree)), requested),
        None => match requested {
            Some(cwd) => SpawnCwd {
                cwd: Some(cwd),
                fallback: workspace_cwd,
                ..SpawnCwd::default()
            },
            None => SpawnCwd {
                cwd: workspace_cwd,
                ..SpawnCwd::default()
            },
        },
    };
    Some(LaunchInputs {
        cwd: spawn.cwd,
        confine_to: spawn.confine_to,
        fallback: spawn.fallback,
        env: definition.env.clone(),
        ..LaunchInputs::default()
    })
}

pub fn prepare(
    home: &Path,
    support: &LaunchSupport,
    state: &State,
    surface: SurfaceId,
    session: SessionId,
    inputs: LaunchInputs,
) -> Option<PreparedLaunch> {
    let location = state.find_surface(surface)?;
    let known = state.surface(surface)?;
    if known.kind() != SurfaceKind::Terminal {
        return None;
    }
    let ws = state.workspaces.get(location.workspace_index)?;
    let config =
        paneflow_config::loader::load_config_from_path(&paneflow_home::config_path_in(home));
    let space_env = paneflow_launch::space::space_env(&config, ws.space.as_ref());
    let spawn = SpawnCwd {
        cwd: inputs.cwd,
        confine_to: inputs.confine_to,
        fallback: inputs.fallback,
    };
    let working_directory = if spawn.needs_resolving() {
        spawn.resolve(CONFINE_TIMEOUT)
    } else {
        spawn.cwd
    };
    let (launch, shell_notice) = paneflow_launch::resolve(
        paneflow_launch::LaunchSpec {
            config: &config,
            working_directory,
            workspace_id: ws.id.get(),
            surface_id: surface.get(),
            initial_size: inputs.size,
            space_env: space_env.as_ref(),
            user_env: inputs.env,
            socket_path: socket_path(home),
        },
        paneflow_launch::LaunchHome {
            shell_integration_dir: Some(paneflow_launch::LaunchHome::shell_integration_dir_in(
                home,
            )),
            helper_bin_dir: support.helper_bin_dir.clone(),
        },
    );
    let mut notices: Vec<String> = shell_notice.into_iter().collect();
    if let Some(space_env) = &space_env {
        let dirs = paneflow_launch::space::config_dirs(space_env, &launch.env);
        notices.extend(paneflow_launch::space::ensure_config_dirs(&dirs));
    }
    let durable: Option<WorkspaceId> = ws.extras.durable_id.clone();
    Some(PreparedLaunch {
        request: paneflow_launch::create_request(
            &launch,
            session,
            durable,
            inputs.appearance,
            inputs.cell,
        ),
        notices,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_config::schema::SurfaceDefinition;

    #[test]
    fn a_surface_that_names_a_command_a_prompt_or_variables_needs_orchestration() {
        let plain = SurfaceDefinition::default();
        assert!(!surface_needs_orchestration(&plain));
        for definition in [
            SurfaceDefinition {
                command: Some("make test".to_string()),
                ..SurfaceDefinition::default()
            },
            SurfaceDefinition {
                prompt: Some("review".to_string()),
                ..SurfaceDefinition::default()
            },
            SurfaceDefinition {
                env: Some(HashMap::from([("A".to_string(), "1".to_string())])),
                ..SurfaceDefinition::default()
            },
        ] {
            assert!(surface_needs_orchestration(&definition), "{definition:?}");
        }
        let empty = SurfaceDefinition {
            command: Some(String::new()),
            env: Some(HashMap::new()),
            ..SurfaceDefinition::default()
        };
        assert!(!surface_needs_orchestration(&empty));
    }
}
