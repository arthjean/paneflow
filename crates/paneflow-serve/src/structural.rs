use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use paneflow_api::methods::{ManagedWorktreeSpec, surface, workspace};
use paneflow_config::schema::{LayoutNode, SessionId, SpaceId, SurfaceDefinition, TabTitleSource};
use paneflow_layout::{
    Direction, Edge, NewNode, NewPane, NewSurface, NewTab, NewWorkspace, Op, PaneId, Preset, State,
    SurfaceId, TabId, WorkspaceId,
};
use serde_json::{Value, json};

use crate::layout_authority::orchestration_refused;
use crate::registry::{Call, ServeError};
use crate::server::Worker;
use crate::typing::Typed;

const MAX_NAME_LEN: usize = 64;
const DEFAULT_SPACE_NAME: &str = "Default";

fn load_config(worker: &Worker) -> paneflow_config::schema::PaneFlowConfig {
    paneflow_config::loader::load_config_from_path(&paneflow_home::config_path_in(&worker.home))
}

fn invalid(message: impl Into<String>) -> ServeError {
    ServeError::invalid_params(message)
}

fn non_empty(value: Option<&String>) -> bool {
    value.is_some_and(|text| !text.is_empty())
}

fn env_has_strings(env: Option<&paneflow_api::methods::EnvMap>) -> bool {
    env.is_some_and(|env| !env.is_empty())
}

fn definition_needs_orchestration(definition: &SurfaceDefinition) -> bool {
    definition.env.as_ref().is_some_and(|env| !env.is_empty())
        || definition.session.is_some()
        || non_empty(definition.scrollback.as_ref())
        || non_empty(definition.command.as_ref())
        || non_empty(definition.prompt.as_ref())
}

fn layout_needs_orchestration(node: &LayoutNode) -> bool {
    match node {
        LayoutNode::Pane { surfaces } => surfaces.iter().any(definition_needs_orchestration),
        LayoutNode::Split { children, .. } => children.iter().any(layout_needs_orchestration),
    }
}

fn strip_invisible(text: &str) -> String {
    text.chars()
        .filter(|c| {
            !matches!(
                *c as u32,
                0x200B..=0x200F | 0x202A..=0x202E | 0x2066..=0x2069 | 0xFEFF
            )
        })
        .collect()
}

pub fn sanitize_pane_name(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME_LEN)
        .collect();
    let cleaned = strip_invisible(&cleaned).trim().to_string();
    (!cleaned.is_empty()).then_some(cleaned)
}

fn label_of(label: Option<&String>, name: Option<&String>) -> Option<String> {
    label.or(name).and_then(|raw| sanitize_pane_name(raw))
}

pub fn canonical_cwd(raw: &str) -> Result<PathBuf, ServeError> {
    paneflow_launch::cwd::canonical_dir(raw).map_err(invalid)
}

fn canonical_dir(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path)
        .ok()
        .map(paneflow_launch::cwd::strip_verbatim_prefix)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    pub record: paneflow_config::schema::ManagedWorktreeDef,
}

fn worktree_of(spec: Option<&ManagedWorktreeSpec>) -> Option<Worktree> {
    let spec = spec?;
    let record = paneflow_git::worktree::record(
        spec.path.as_deref()?,
        spec.repo_root.as_deref().unwrap_or_default(),
        spec.branch.as_deref().unwrap_or_default(),
        spec.teardown.as_deref().unwrap_or_default(),
    )?;
    Some(Worktree {
        record: paneflow_config::schema::ManagedWorktreeDef {
            path: record.path.display().to_string(),
            repo_root: record.repo_root.display().to_string(),
            branch: record.branch.clone(),
            teardown: record.teardown.to_string(),
        },
        path: record.path,
        branch: record.branch,
    })
}

fn outside_worktree(cwd: &Path, worktree: &Path) -> ServeError {
    invalid(format!(
        "cwd {} is outside the worktree {} bound to this tab",
        cwd.display(),
        worktree.display()
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Tab(TabId),
    NewTab(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub target: Target,
    pub cwd: Option<PathBuf>,
}

pub fn decide_split(
    cwd: Option<&str>,
    worktree: Option<&Path>,
    anchor_tab: TabId,
    tabs: &[(TabId, Option<PathBuf>)],
    fallback: Option<PathBuf>,
) -> Result<Decision, ServeError> {
    let explicit = cwd.map(canonical_cwd).transpose()?;
    if let Some(worktree) = worktree {
        let root = canonical_dir(worktree).ok_or_else(|| {
            invalid(format!(
                "managed_worktree path {} does not exist",
                worktree.display()
            ))
        })?;
        if let Some(cwd) = &explicit
            && !cwd.starts_with(&root)
        {
            return Err(outside_worktree(cwd, worktree));
        }
        let target = tabs
            .iter()
            .find(|(_, bound)| bound.as_deref().and_then(canonical_dir).as_ref() == Some(&root))
            .map_or_else(
                || Target::NewTab(worktree.to_path_buf()),
                |(id, _)| Target::Tab(*id),
            );
        return Ok(Decision {
            target,
            cwd: Some(explicit.unwrap_or_else(|| worktree.to_path_buf())),
        });
    }
    let anchor = Target::Tab(anchor_tab);
    let bound = tabs
        .iter()
        .find(|(id, _)| *id == anchor_tab)
        .and_then(|(_, worktree)| worktree.clone());
    let Some(bound) = bound else {
        return Ok(Decision {
            target: anchor,
            cwd: explicit.or(fallback),
        });
    };
    let root = canonical_dir(&bound);
    let inside = |cwd: &Path| root.as_ref().is_some_and(|root| cwd.starts_with(root));
    match explicit {
        Some(cwd) if inside(&cwd) => Ok(Decision {
            target: anchor,
            cwd: Some(cwd),
        }),
        Some(cwd) => Err(outside_worktree(&cwd, &bound)),
        None => Ok(Decision {
            target: anchor,
            cwd: Some(bound),
        }),
    }
}

pub fn target_space(
    config: &paneflow_config::schema::PaneFlowConfig,
    active: Option<&SpaceId>,
    requested: Option<&str>,
) -> Result<Option<SpaceId>, ServeError> {
    let Some(value) = requested else {
        return Ok(active.cloned());
    };
    let folded = value.trim().to_lowercase();
    if folded == DEFAULT_SPACE_NAME.to_lowercase() {
        return Ok(None);
    }
    config
        .spaces
        .iter()
        .find(|space| {
            space.id.as_str() == value.trim() || space.name.trim().to_lowercase() == folded
        })
        .map(|space| Some(space.id.clone()))
        .ok_or_else(|| invalid(format!("unknown space: {value}")))
}

fn terminal_definition(
    cwd: Option<&Path>,
    env: Option<HashMap<String, String>>,
    command: Option<String>,
    prompt: Option<String>,
    name: Option<String>,
) -> SurfaceDefinition {
    SurfaceDefinition {
        surface_type: Some("terminal".to_string()),
        cwd: cwd.map(|cwd| cwd.display().to_string()),
        env: env.filter(|env| !env.is_empty()),
        command: command.filter(|text| !text.is_empty()),
        prompt: prompt.filter(|text| !text.is_empty()),
        custom_name: name,
        ..SurfaceDefinition::default()
    }
}

fn env_map(env: Option<paneflow_api::methods::EnvMap>) -> Option<HashMap<String, String>> {
    env.map(|env| env.into_iter().collect())
}

pub fn inherit_sandbox(
    worker: &Worker,
    principal: &paneflow_api::Principal,
    env: Option<HashMap<String, String>>,
) -> Option<HashMap<String, String>> {
    let paneflow_api::Principal::Pane { session, .. } = principal else {
        return env;
    };
    let Some(parent) = worker.lock_state().sandbox_request_of(session) else {
        return env;
    };
    let mut env = env.unwrap_or_default();
    let key = paneflow_config::schema::SANDBOX_REQUEST_ENV;
    let stronger = env
        .get(key)
        .is_some_and(|asked| asked == paneflow_config::schema::SANDBOX_REQUEST_PANEFLOW_UNIX_SCOPE);
    if !stronger {
        env.insert(key.to_string(), parent.to_string());
    }
    Some(env)
}

fn inherit_sandbox_in_layout(
    worker: &Worker,
    principal: &paneflow_api::Principal,
    node: LayoutNode,
) -> LayoutNode {
    match node {
        LayoutNode::Pane { surfaces } => {
            let surfaces = if surfaces.is_empty() {
                vec![terminal_definition(None, None, None, None, None)]
            } else {
                surfaces
            };
            LayoutNode::Pane {
                surfaces: surfaces
                    .into_iter()
                    .map(|definition| {
                        let terminal = definition
                            .surface_type
                            .as_deref()
                            .is_none_or(|kind| kind == "terminal");
                        if !terminal {
                            return definition;
                        }
                        SurfaceDefinition {
                            env: inherit_sandbox(worker, principal, definition.env.clone()),
                            ..definition
                        }
                    })
                    .collect(),
            }
        }
        LayoutNode::Split {
            direction,
            ratio,
            ratios,
            children,
        } => LayoutNode::Split {
            direction,
            ratio,
            ratios,
            children: children
                .into_iter()
                .map(|child| inherit_sandbox_in_layout(worker, principal, child))
                .collect(),
        },
    }
}

struct Built {
    state: State,
    launch_error: Option<ServeError>,
}

fn answer(launch_error: Option<ServeError>, value: Value) -> Result<Value, ServeError> {
    match launch_error {
        Some(error) => Err(error),
        None => Ok(value),
    }
}

fn index_out_of_bounds(index: usize, len: usize) -> ServeError {
    invalid(format!("index {index} is out of bounds ({len} workspaces)"))
}

fn apply(worker: &Arc<Worker>, op: Op) -> Result<Built, ServeError> {
    let value = serde_json::to_value(&op)
        .map_err(|error| ServeError::internal(format!("the op does not encode: {error}")))?;
    let outcome = worker.layout.apply(
        crate::layout_authority::ConnectionId::INTERNAL,
        value,
        None,
        crate::state::now_ms(),
        true,
    )?;
    worker.append_layout_events(outcome.events);
    Ok(Built {
        state: worker.layout.current_state(),
        launch_error: outcome.launch_error,
    })
}

fn session_of(state: &State, surface: SurfaceId) -> Option<SessionId> {
    state.surface(surface)?.definition.session.clone()
}

fn type_into(
    worker: &Arc<Worker>,
    state: &State,
    surface: SurfaceId,
    command: Option<String>,
    prompt: Option<String>,
) {
    let Some(session) = session_of(state, surface) else {
        return;
    };
    if let Some(typed) = Typed::new(session, command, prompt) {
        crate::typing::type_when_settled(worker.core_endpoint.clone(), typed);
    }
}

fn anchor_pane(
    state: &State,
    surface: Option<u64>,
) -> Result<(usize, TabId, Option<PaneId>), ServeError> {
    if let Some(surface) = surface {
        let location = state
            .find_surface(SurfaceId(surface))
            .ok_or_else(|| invalid(format!("surface_id {surface} not found")))?;
        return Ok((location.workspace_index, location.tab, Some(location.pane)));
    }
    let index = state.active_workspace;
    let ws = state
        .workspaces
        .get(index)
        .ok_or_else(|| invalid("No active workspace"))?;
    let tab = ws
        .active_tab()
        .ok_or_else(|| invalid("No active workspace"))?;
    let pane = tab.focused.or_else(|| {
        tab.root
            .as_ref()
            .and_then(|root| root.first_pane())
            .map(|pane| pane.id)
    });
    Ok((index, tab.id, pane))
}

fn caller_workspace(state: &State, principal: &paneflow_api::Principal) -> Option<u64> {
    let paneflow_api::Principal::Pane { session, .. } = principal else {
        return None;
    };
    crate::agent_asks::place_of_session(state, session).map(|place| place.workspace)
}

fn spawn_outside_workspace(method: &str, authority: &crate::authority::Authority) -> ServeError {
    ServeError {
        code: crate::protocol::ERR_INVALID_PARAMS,
        message: format!(
            "{method} refused: the calling pane's spawn grant covers its own workspace; \
             launching a command, a prompt or an env in another workspace needs an instance grant"
        ),
        data: Some(paneflow_api::authorize::refusal_data(
            &authority.principal,
            paneflow_api::Refusal::ScopeRefused,
        )),
    }
}

fn workspace_pane_count(state: &State, index: usize) -> usize {
    state.workspaces.get(index).map_or(0, |ws| {
        ws.tabs.iter().map(paneflow_layout::Tab::pane_count).sum()
    })
}

pub(crate) fn surface_split(
    worker: &Arc<Worker>,
    call: Call<'_, surface::SplitParams>,
) -> Result<Value, ServeError> {
    let params = call.params;
    let (direction, direction_name) = match params.direction {
        surface::SplitDirection::Horizontal => (Direction::Horizontal, "horizontal"),
        surface::SplitDirection::Vertical => (Direction::Vertical, "vertical"),
    };
    let spawning = non_empty(params.command.as_ref())
        || non_empty(params.prompt.as_ref())
        || env_has_strings(params.env.as_ref());
    if spawning && !call.authority.may_spawn() {
        return Err(orchestration_refused("surface.split"));
    }
    if params.worktree.is_some() && params.managed_worktree.is_some() {
        return Err(invalid("worktree and managed_worktree are exclusive"));
    }
    let state = worker.layout.current_state();
    let (ws_index, anchor_tab, anchor) = anchor_pane(&state, params.surface_id)?;
    let ws = &state.workspaces[ws_index];
    let outside_own_workspace = call.authority.spawns_in_own_workspace_only()
        && caller_workspace(&state, &call.authority.principal) != Some(ws.id.get());
    if spawning && outside_own_workspace {
        return Err(spawn_outside_workspace("surface.split", call.authority));
    }
    let request = params
        .worktree
        .clone()
        .map(|request| crate::worktrees::request_of(request, params.cwd.clone()));
    let mut prepared = crate::worktrees::prepare(
        &load_config(worker),
        &state,
        ws.space.as_ref(),
        vec![request],
        "surface.split",
        call.authority.may_spawn() && !outside_own_workspace,
    )?;
    let created = match prepared.plans.first().and_then(Option::as_ref) {
        Some(plan) => Some((created_worktree(plan)?, created_worktree_cwd(plan)?)),
        None => None,
    };
    let worktree = match &created {
        Some((worktree, _)) => Some(worktree.clone()),
        None => worktree_of(params.managed_worktree.as_ref()),
    };
    let split_cwd = match &created {
        Some((_, cwd)) => Some(cwd.display().to_string()),
        None => params.cwd.clone(),
    };
    let tabs: Vec<(TabId, Option<PathBuf>)> = ws
        .tabs
        .iter()
        .map(|tab| (tab.id, tab.worktree.as_ref().map(PathBuf::from)))
        .collect();
    let fallback = (!ws.cwd.is_empty()).then(|| PathBuf::from(&ws.cwd));
    let decision = decide_split(
        split_cwd.as_deref(),
        worktree.as_ref().map(|worktree| worktree.path.as_path()),
        anchor_tab,
        &tabs,
        fallback,
    )?;
    let workspace_id = ws.id;
    let surface = SurfaceId(worker.layout.reserve_id());
    let pane = PaneId(worker.layout.reserve_id());
    let definition = terminal_definition(
        decision.cwd.as_deref(),
        inherit_sandbox(worker, &call.authority.principal, env_map(params.env)),
        params.command.clone(),
        None,
        label_of(params.label.as_ref(), params.name.as_ref()),
    );
    let new_pane = NewPane {
        id: pane,
        surfaces: vec![NewSurface {
            id: surface,
            definition,
        }],
        active: 0,
    };
    let op = match decision.target {
        Target::Tab(tab) => {
            let target = match anchor.filter(|_| tab == anchor_tab) {
                Some(pane) => pane,
                None => state
                    .tab(tab)
                    .and_then(|tab| {
                        tab.focused
                            .or_else(|| tab.root.as_ref()?.first_pane().map(|pane| pane.id))
                    })
                    .ok_or_else(|| invalid("the target tab holds no pane"))?,
            };
            Op::Split {
                target,
                direction,
                edge: Edge::After,
                pane: new_pane,
                focus: false,
            }
        }
        Target::NewTab(path) => Op::OpenTab {
            workspace: workspace_id,
            tab: NewTab {
                id: TabId(worker.layout.reserve_id()),
                title: worktree
                    .as_ref()
                    .map(|worktree| worktree.branch.clone())
                    .unwrap_or_default(),
                title_source: TabTitleSource::Preset,
                worktree: Some(path.display().to_string()),
                root: Some(NewNode::Pane(new_pane)),
            },
            fill_empty_active: false,
        },
    };
    let mut built = apply(worker, op)?;
    prepared.keep();
    if let Some(worktree) = &worktree {
        record_unlisted_worktree(worker, workspace_id, &worktree.record);
    }
    if let (
        paneflow_api::Principal::Pane {
            session: parent, ..
        },
        Some(child),
    ) = (&call.authority.principal, session_of(&built.state, surface))
    {
        worker.pane_grants.record_spawn(parent, child.as_str());
    }
    let launch_error = built.launch_error.take();
    type_into(worker, &built.state, surface, params.command, params.prompt);
    let ws_index = built
        .state
        .workspace_index(workspace_id)
        .unwrap_or(ws_index);
    let mut done = json!({
        "split": true,
        "direction": direction_name,
        "panes": workspace_pane_count(&built.state, ws_index),
        "surface_id": surface.get(),
    });
    if !prepared.notices.is_empty() {
        done["notices"] = json!(std::mem::take(&mut prepared.notices));
    }
    answer(launch_error, done)
}

pub(crate) fn window_spawner(
    spawned_by: Option<String>,
    principal: &paneflow_api::Principal,
) -> Result<Option<String>, ServeError> {
    let Some(parent) = spawned_by else {
        return Ok(None);
    };
    if *principal != paneflow_api::Principal::Window {
        return Err(invalid(
            "spawned_by is accepted only from the attached Paneflow window",
        ));
    }
    parent
        .parse::<SessionId>()
        .map(|session| Some(session.to_string()))
        .map_err(|_| invalid("spawned_by is not a session id"))
}

pub(crate) fn created_ids(op: &Value) -> Vec<u64> {
    serde_json::from_value::<Op>(op.clone())
        .map(|op| op.new_ids())
        .unwrap_or_default()
}

pub(crate) fn record_window_spawns(worker: &Worker, parent: &str, created: &[u64]) {
    let state = worker.layout.current_state();
    for id in created {
        if let Some(child) = session_of(&state, SurfaceId(*id)) {
            worker.pane_grants.record_spawn(parent, child.as_str());
        }
    }
}

fn surface_or_focused(state: &State, surface: Option<u64>) -> Result<SurfaceId, ServeError> {
    if let Some(surface) = surface {
        return state
            .find_surface(SurfaceId(surface))
            .map(|_| SurfaceId(surface))
            .ok_or_else(|| invalid(format!("surface_id {surface} not found")));
    }
    let (_, tab, pane) = anchor_pane(state, None)?;
    let pane = pane
        .and_then(|pane| state.pane(pane))
        .or_else(|| state.tab(tab)?.root.as_ref()?.first_pane())
        .ok_or_else(|| invalid("no surface is focused"))?;
    pane.active_surface()
        .map(|surface| surface.id)
        .ok_or_else(|| invalid("no surface is focused"))
}

pub(crate) fn surface_rename(
    worker: &Arc<Worker>,
    call: Call<'_, surface::RenameParams>,
) -> Result<Value, ServeError> {
    let state = worker.layout.current_state();
    let surface = surface_or_focused(&state, call.params.surface_id)?;
    let name = call
        .params
        .name
        .as_deref()
        .or(call.params.new_name.as_deref())
        .and_then(sanitize_pane_name);
    apply(
        worker,
        Op::RenameSurface {
            surface,
            name: name.clone(),
        },
    )?;
    Ok(json!({"renamed": true, "name": name}))
}

pub(crate) fn surface_focus(
    worker: &Arc<Worker>,
    call: Call<'_, surface::FocusParams>,
) -> Result<Value, ServeError> {
    let Some(sid) = call.params.surface_id else {
        return Err(invalid("missing 'surface_id' parameter"));
    };
    let state = worker.layout.current_state();
    let location = state
        .find_surface(SurfaceId(sid))
        .ok_or_else(|| invalid(format!("surface_id {sid} not found")))?;
    let mut ops = vec![
        Op::SelectWorkspace {
            workspace: location.workspace,
            focus: true,
        },
        Op::SelectTab { tab: location.tab },
    ];
    if state
        .tab(location.tab)
        .and_then(|tab| tab.zoomed)
        .is_some_and(|zoomed| zoomed != location.pane)
    {
        ops.push(Op::SetZoom {
            tab: location.tab,
            pane: None,
        });
    }
    ops.push(Op::FocusPane {
        pane: location.pane,
    });
    ops.push(Op::SelectSurface {
        surface: SurfaceId(sid),
    });
    for op in ops {
        apply(worker, op)?;
    }
    Ok(json!({
        "focused": true,
        "surface_id": sid,
        "workspace": location.workspace_index,
        "scope": "workspace",
    }))
}

pub(crate) fn workspace_select(
    worker: &Arc<Worker>,
    call: Call<'_, workspace::IndexParams>,
) -> Result<Value, ServeError> {
    let Some(index) = call.params.index else {
        return Err(invalid("missing 'index' parameter"));
    };
    let state = worker.layout.current_state();
    let ws = state
        .workspaces
        .get(index)
        .ok_or_else(|| index_out_of_bounds(index, state.workspaces.len()))?;
    apply(
        worker,
        Op::SelectWorkspace {
            workspace: ws.id,
            focus: true,
        },
    )?;
    Ok(json!({"selected": index}))
}

pub(crate) fn workspace_close(
    worker: &Arc<Worker>,
    call: Call<'_, workspace::IndexParams>,
) -> Result<Value, ServeError> {
    let Some(index) = call.params.index else {
        return Err(invalid("missing 'index' parameter"));
    };
    let state = worker.layout.current_state();
    if state.workspaces.len() <= 1 {
        return Err(invalid("Cannot close the last workspace"));
    }
    let ws = state
        .workspaces
        .get(index)
        .ok_or_else(|| index_out_of_bounds(index, state.workspaces.len()))?;
    let worktrees: Vec<paneflow_git::worktree::ManagedWorktree> = ws
        .extras
        .managed_worktrees
        .iter()
        .map(|def| paneflow_git::worktree::ManagedWorktree {
            path: PathBuf::from(&def.path),
            repo_root: PathBuf::from(&def.repo_root),
            branch: def.branch.clone(),
            teardown: paneflow_git::worktree::TeardownPolicy::from_record(&def.teardown),
        })
        .collect();
    let sessions: Vec<SessionId> = ws
        .tabs
        .iter()
        .filter_map(|tab| tab.root.as_ref())
        .flat_map(|root| root.panes())
        .flat_map(|pane| pane.surfaces.iter().filter_map(|s| s.session().cloned()))
        .collect();
    apply(
        worker,
        Op::CloseWorkspace {
            workspace: ws.id,
            focus: true,
        },
    )?;
    tear_down_closed_worktrees(worker, worktrees, sessions);
    Ok(json!({"closed": index}))
}

const WORKTREE_HOST_CLIENT: &str = "paneflow-serve-worktrees";

fn live_host_sessions(endpoint: &Path) -> paneflow_git::worktree::SessionListing<SessionId> {
    let hello = paneflow_host::protocol::ClientHello::control(WORKTREE_HOST_CLIENT);
    let mut client = match paneflow_host::HostClient::connect(endpoint, &hello) {
        Ok(client) => client,
        Err(error) if error.is_endpoint_absent() => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let rows = client.list(None).map_err(|error| error.to_string())?;
    Ok(Some(
        rows.into_iter()
            .filter(|row| row.live)
            .map(|row| (row.session, PathBuf::from(row.cwd)))
            .collect(),
    ))
}

fn listed_by_a_workspace(state: &State, path: &str) -> bool {
    state
        .workspaces
        .iter()
        .flat_map(|ws| ws.extras.managed_worktrees.iter())
        .any(|known| known.path == path)
}

fn record_unlisted_worktree(
    worker: &Arc<Worker>,
    workspace: WorkspaceId,
    record: &paneflow_config::schema::ManagedWorktreeDef,
) {
    if listed_by_a_workspace(&worker.layout.current_state(), &record.path) {
        return;
    }
    if let Err(error) = apply(
        worker,
        Op::RecordWorktree {
            workspace,
            worktree: record.clone(),
        },
    ) {
        log::warn!(
            "paneflow-serve: cannot record the worktree {}: {}",
            record.path,
            error.message
        );
    }
}

fn checkouts_in_use(state: &State) -> Vec<PathBuf> {
    state
        .workspaces
        .iter()
        .flat_map(|ws| {
            ws.tabs
                .iter()
                .filter_map(|tab| tab.worktree.clone())
                .chain(std::iter::once(ws.cwd.clone()))
        })
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn worktree_heir(
    state: &State,
    wt: &paneflow_git::worktree::ManagedWorktree,
) -> Option<WorkspaceId> {
    let path = wt.path.display().to_string();
    state
        .workspaces
        .iter()
        .find(|ws| {
            ws.tabs
                .iter()
                .any(|tab| tab.worktree.as_deref() == Some(path.as_str()))
        })
        .or_else(|| {
            state
                .workspaces
                .iter()
                .find(|ws| !ws.cwd.is_empty() && Path::new(&ws.cwd).starts_with(&wt.repo_root))
        })
        .or_else(|| state.workspaces.get(state.active_workspace))
        .or_else(|| state.workspaces.first())
        .map(|ws| ws.id)
}

fn tear_down_closed_worktrees(
    worker: &Arc<Worker>,
    worktrees: Vec<paneflow_git::worktree::ManagedWorktree>,
    sessions: Vec<SessionId>,
) {
    use paneflow_git::worktree;
    if worktrees.is_empty() {
        return;
    }
    let worker = Arc::clone(worker);
    let spawned = std::thread::Builder::new()
        .name("paneflow-serve-worktree-teardown".into())
        .spawn(move || {
            let endpoint = worker.core_endpoint.clone();
            let seen = worktree::after_sessions_end(
                &sessions,
                || live_host_sessions(&endpoint),
                || std::thread::sleep(worktree::SESSION_END_PAUSE),
                worktree::SESSION_END_POLLS,
            );
            let auto_remove = load_config(&worker).worktrees.auto_remove_enabled();
            let checkouts = checkouts_in_use(&worker.layout.current_state());
            let outcome = worktree::close_teardown(worktrees, auto_remove, seen, checkouts);
            for message in &outcome.messages {
                log::warn!("paneflow-serve: {message}");
            }
            for wt in outcome.kept {
                let state = worker.layout.current_state();
                let Some(heir) = worktree_heir(&state, &wt) else {
                    log::warn!(
                        "paneflow-serve: worktree kept ({}) but no open workspace can list it",
                        wt.path.display()
                    );
                    continue;
                };
                let record = paneflow_config::schema::ManagedWorktreeDef {
                    path: wt.path.display().to_string(),
                    repo_root: wt.repo_root.display().to_string(),
                    branch: wt.branch,
                    teardown: wt.teardown.as_str().to_string(),
                };
                record_unlisted_worktree(&worker, heir, &record);
            }
        });
    if let Err(error) = spawned {
        log::warn!("paneflow-serve: cannot start the worktree teardown: {error}");
    }
}

fn layout_param(
    raw: Option<paneflow_api::methods::LayoutDocument>,
) -> Result<Option<LayoutNode>, ServeError> {
    raw.map(|document| {
        serde_json::from_value::<LayoutNode>(Value::Object(document.0))
            .map_err(|error| invalid(format!("invalid layout: {error}")))
    })
    .transpose()
}

pub struct Planted {
    pub node: NewNode,
    pub terminals: Vec<(SurfaceId, Option<String>, Option<String>)>,
}

pub fn plant(node: &LayoutNode, next: &mut dyn FnMut() -> u64) -> Planted {
    let mut terminals = Vec::new();
    let node = plant_into(node, next, &mut terminals);
    Planted { node, terminals }
}

fn plant_into(
    node: &LayoutNode,
    next: &mut dyn FnMut() -> u64,
    terminals: &mut Vec<(SurfaceId, Option<String>, Option<String>)>,
) -> NewNode {
    match node {
        LayoutNode::Pane { surfaces } => {
            let definitions: Vec<SurfaceDefinition> = if surfaces.is_empty() {
                vec![terminal_definition(None, None, None, None, None)]
            } else {
                surfaces.clone()
            };
            let pane = PaneId(next());
            let active = definitions
                .iter()
                .position(|definition| definition.focus == Some(true))
                .unwrap_or(0);
            let surfaces = definitions
                .into_iter()
                .map(|mut definition| {
                    definition.session = None;
                    let id = SurfaceId(next());
                    let terminal = definition
                        .surface_type
                        .as_deref()
                        .is_none_or(|kind| kind == "terminal");
                    if terminal {
                        terminals.push((id, definition.command.clone(), definition.prompt.clone()));
                    }
                    NewSurface { id, definition }
                })
                .collect();
            NewNode::Pane(NewPane {
                id: pane,
                surfaces,
                active,
            })
        }
        LayoutNode::Split {
            direction,
            children,
            ..
        } => NewNode::Split {
            direction: Direction::from_layout_str(direction),
            ratios: node
                .resolved_ratios()
                .into_iter()
                .map(|ratio| ratio as f32)
                .collect(),
            children: children
                .iter()
                .map(|child| plant_into(child, next, terminals))
                .collect(),
        },
    }
}

fn type_planted(
    worker: &Arc<Worker>,
    state: &State,
    terminals: Vec<(SurfaceId, Option<String>, Option<String>)>,
) {
    for (surface, command, prompt) in terminals {
        type_into(worker, state, surface, command, prompt);
    }
}

pub(crate) fn workspace_create(
    worker: &Arc<Worker>,
    call: Call<'_, workspace::CreateParams>,
) -> Result<Value, ServeError> {
    let params = call.params;
    let name = params.name.unwrap_or_else(|| "Terminal".to_string());
    let layout = layout_param(params.layout)?;
    if layout.as_ref().is_some_and(layout_needs_orchestration) && !call.authority.may_spawn() {
        return Err(orchestration_refused("workspace.create"));
    }
    let layout =
        layout.map(|layout| inherit_sandbox_in_layout(worker, &call.authority.principal, layout));
    let cwd = params.cwd.as_deref().map(canonical_cwd).transpose()?;
    let config = load_config(worker);
    let state = worker.layout.current_state();
    let space = target_space(
        &config,
        state.active_space.as_ref(),
        params.space.as_deref(),
    )?;
    let mut next = || worker.layout.reserve_id();
    let planted = match &layout {
        Some(layout) => {
            let needed = layout.leaf_count();
            if needed == 0 || needed > paneflow_layout::MAX_PANES {
                return Err(invalid(format!(
                    "layout could not be applied: layout must hold between 1 and {} panes",
                    paneflow_layout::MAX_PANES
                )));
            }
            plant(layout, &mut next)
        }
        None => plant(
            &LayoutNode::Pane {
                surfaces: vec![terminal_definition(
                    cwd.as_deref(),
                    inherit_sandbox(worker, &call.authority.principal, None),
                    None,
                    None,
                    None,
                )],
            },
            &mut next,
        ),
    };
    let workspace_id = WorkspaceId(worker.layout.reserve_id());
    let tab = TabId(worker.layout.reserve_id());
    let workspace_cwd = cwd.unwrap_or_else(paneflow_launch::cwd::implicit_launch_cwd);
    let mut built = apply(
        worker,
        Op::CreateWorkspace {
            workspace: NewWorkspace {
                id: workspace_id,
                title: name.clone(),
                cwd: workspace_cwd.display().to_string(),
                space,
                durable_id: Some(paneflow_config::schema::WorkspaceId::new()),
                custom_buttons: Vec::new(),
                managed_worktrees: Vec::new(),
                tabs: vec![NewTab {
                    id: tab,
                    title: String::new(),
                    title_source: TabTitleSource::Preset,
                    worktree: None,
                    root: Some(planted.node),
                }],
                active_tab: 0,
            },
            select: false,
        },
    )?;
    let index = built
        .state
        .workspace_index(workspace_id)
        .ok_or_else(|| ServeError::internal("the new workspace is missing from the model"))?;
    let panes = if layout.is_some() {
        workspace_pane_count(&built.state, index)
    } else {
        1
    };
    let launch_error = built.launch_error.take();
    type_planted(worker, &built.state, planted.terminals);
    answer(
        launch_error,
        json!({"index": index, "title": name, "panes": panes}),
    )
}

pub(crate) fn workspace_restore_layout(
    worker: &Arc<Worker>,
    call: Call<'_, workspace::RestoreLayoutParams>,
) -> Result<Value, ServeError> {
    let Some(layout) = layout_param(call.params.layout)? else {
        return Err(invalid("missing 'layout' parameter"));
    };
    if layout_needs_orchestration(&layout) && !call.authority.may_spawn() {
        return Err(orchestration_refused("workspace.restore_layout"));
    }
    let layout = inherit_sandbox_in_layout(worker, &call.authority.principal, layout);
    let needed = layout.leaf_count();
    if needed == 0 || needed > paneflow_layout::MAX_PANES {
        return Err(invalid(format!(
            "layout must hold between 1 and {} panes",
            paneflow_layout::MAX_PANES
        )));
    }
    let state = worker.layout.current_state();
    let index = state.active_workspace;
    let tab = state
        .workspaces
        .get(index)
        .and_then(paneflow_layout::Workspace::active_tab)
        .map(|tab| tab.id)
        .ok_or_else(|| invalid("No active workspace"))?;
    let mut next = || worker.layout.reserve_id();
    let planted = plant(&layout, &mut next);
    let mut built = apply(
        worker,
        Op::RestoreLayout {
            tab,
            layout: planted.node,
        },
    )?;
    let launch_error = built.launch_error.take();
    type_planted(worker, &built.state, planted.terminals);
    answer(
        launch_error,
        json!({"restored": true, "panes": workspace_pane_count(&built.state, index)}),
    )
}

struct PlannedPane {
    cwd: Option<PathBuf>,
    command: Option<String>,
    prompt: Option<String>,
    env: Option<HashMap<String, String>>,
    focus: bool,
    label: Option<String>,
    worktree: Option<Worktree>,
}

fn claim_unique(taken: &mut HashSet<String>, label: &str) -> String {
    if taken.insert(label.to_string()) {
        return label.to_string();
    }
    let mut suffix = 2;
    loop {
        let candidate = format!("{label}-{suffix}");
        if taken.insert(candidate.clone()) {
            return candidate;
        }
        suffix += 1;
    }
}

fn equal_node(direction: Direction, panes: Vec<NewPane>) -> Option<NewNode> {
    match panes.len() {
        0 => None,
        1 => panes.into_iter().next().map(NewNode::Pane),
        count => Some(NewNode::Split {
            direction,
            ratios: vec![1.0 / count as f32; count],
            children: panes.into_iter().map(NewNode::Pane).collect(),
        }),
    }
}

pub(crate) fn workspace_up(
    worker: &Arc<Worker>,
    call: Call<'_, workspace::UpParams>,
) -> Result<Value, ServeError> {
    let params = call.params;
    let name = params.name.unwrap_or_else(|| "Workspace".to_string());
    let preset = params.layout.unwrap_or_else(|| "even_h".to_string());
    if params.panes.is_empty() {
        return Err(invalid("`panes` must be a non-empty array"));
    }
    if params.panes.len() > paneflow_layout::MAX_PANES {
        return Err(invalid(format!(
            "layout exceeds maximum pane count ({})",
            paneflow_layout::MAX_PANES
        )));
    }
    let spawning = params.panes.iter().any(|pane| {
        non_empty(pane.command.as_ref())
            || non_empty(pane.prompt.as_ref())
            || env_has_strings(pane.env.as_ref())
    });
    if spawning && !call.authority.may_spawn() {
        return Err(orchestration_refused("workspace.up"));
    }
    let mut taken = HashSet::new();
    let mut planned = Vec::with_capacity(params.panes.len());
    let mut requests = Vec::with_capacity(params.panes.len());
    for (i, spec) in params.panes.into_iter().enumerate() {
        if spec.worktree.is_some() && spec.managed_worktree.is_some() {
            return Err(invalid(format!(
                "pane {i}: worktree and managed_worktree are exclusive"
            )));
        }
        requests.push(
            spec.worktree
                .clone()
                .map(|request| crate::worktrees::request_of(request, spec.cwd.clone())),
        );
        let cwd = spec
            .cwd
            .as_deref()
            .filter(|_| spec.worktree.is_none())
            .map(canonical_cwd)
            .transpose()
            .map_err(|error| invalid(format!("pane {i}: {}", error.message)))?;
        let label = label_of(spec.label.as_ref(), spec.name.as_ref()).map(|label| {
            let unique = claim_unique(&mut taken, &label);
            if unique != label {
                log::warn!("workspace.up: duplicate label '{label}' in batch, using '{unique}'");
            }
            unique
        });
        planned.push(PlannedPane {
            cwd,
            command: spec.command,
            prompt: spec.prompt,
            env: inherit_sandbox(worker, &call.authority.principal, env_map(spec.env)),
            focus: spec.focus.unwrap_or(false),
            label,
            worktree: worktree_of(spec.managed_worktree.as_ref()),
        });
    }
    let config = load_config(worker);
    let state = worker.layout.current_state();
    let space = target_space(
        &config,
        state.active_space.as_ref(),
        params.space.as_deref(),
    )?;
    let mut prepared = crate::worktrees::prepare(
        &config,
        &state,
        space.as_ref(),
        requests,
        "workspace.up",
        call.authority.may_spawn(),
    )?;
    for (i, plan) in prepared.plans.iter().enumerate() {
        let Some(plan) = plan else {
            continue;
        };
        planned[i].cwd = Some(created_worktree_cwd(plan)?);
        planned[i].worktree = Some(created_worktree(plan)?);
    }
    let labels: Vec<Value> = planned
        .iter()
        .map(|pane| pane.label.clone().map_or(Value::Null, Value::String))
        .collect();
    let requested_focus = planned.iter().position(|pane| pane.focus);
    let focus_index = requested_focus.unwrap_or(0);
    let mut groups: Vec<(Option<Worktree>, Vec<usize>)> = Vec::new();
    for (i, pane) in planned.iter().enumerate() {
        match &pane.worktree {
            None => match groups.iter_mut().find(|(worktree, _)| worktree.is_none()) {
                Some((_, members)) => members.push(i),
                None => groups.insert(0, (None, vec![i])),
            },
            Some(worktree) => match groups.iter_mut().find(|(known, _)| {
                known
                    .as_ref()
                    .is_some_and(|known| known.path == worktree.path)
            }) {
                Some((_, members)) => members.push(i),
                None => groups.push((Some(worktree.clone()), vec![i])),
            },
        }
    }
    let managed_worktrees: Vec<paneflow_config::schema::ManagedWorktreeDef> = groups
        .iter()
        .filter_map(|(worktree, _)| worktree.as_ref().map(|worktree| worktree.record.clone()))
        .filter(|record| !listed_by_a_workspace(&state, &record.path))
        .collect();
    let surfaces: Vec<SurfaceId> = planned
        .iter()
        .map(|_| SurfaceId(worker.layout.reserve_id()))
        .collect();
    let panes: Vec<PaneId> = planned
        .iter()
        .map(|_| PaneId(worker.layout.reserve_id()))
        .collect();
    let mut tabs = Vec::with_capacity(groups.len());
    let mut active_tab = 0;
    let mut presets = Vec::new();
    for (tab_index, (worktree, members)) in groups.iter().enumerate() {
        if members.contains(&focus_index) {
            active_tab = tab_index;
        }
        let group: Vec<NewPane> = members
            .iter()
            .map(|i| {
                let pane = &planned[*i];
                NewPane {
                    id: panes[*i],
                    surfaces: vec![NewSurface {
                        id: surfaces[*i],
                        definition: terminal_definition(
                            pane.cwd.as_deref(),
                            pane.env.clone(),
                            pane.command.clone(),
                            None,
                            pane.label.clone(),
                        ),
                    }],
                    active: 0,
                }
            })
            .collect();
        let direction = if preset == "even_v" {
            Direction::Horizontal
        } else {
            Direction::Vertical
        };
        let root = equal_node(direction, group)
            .ok_or_else(|| invalid("could not build layout from panes"))?;
        let tab = TabId(worker.layout.reserve_id());
        let local_focus = members
            .iter()
            .find(|i| **i == focus_index)
            .or_else(|| members.first())
            .map(|i| panes[*i]);
        match preset.as_str() {
            "tiled" => presets.push((tab, Preset::Tiled, local_focus)),
            "main_vertical" => presets.push((tab, Preset::MainVertical, local_focus)),
            _ => {}
        }
        tabs.push(NewTab {
            id: tab,
            title: worktree
                .as_ref()
                .map(|worktree| worktree.branch.clone())
                .unwrap_or_default(),
            title_source: TabTitleSource::Preset,
            worktree: worktree
                .as_ref()
                .map(|worktree| worktree.path.display().to_string()),
            root: Some(root),
        });
    }
    let workspace_cwd = groups
        .iter()
        .find(|(worktree, _)| worktree.is_none())
        .and_then(|(_, members)| members.iter().find_map(|i| planned[*i].cwd.clone()))
        .or_else(|| planned.iter().find_map(|pane| pane.cwd.clone()))
        .unwrap_or_else(paneflow_launch::cwd::implicit_launch_cwd);
    let workspace_id = WorkspaceId(worker.layout.reserve_id());
    let launch_error = apply(
        worker,
        Op::CreateWorkspace {
            workspace: NewWorkspace {
                id: workspace_id,
                title: name.clone(),
                cwd: workspace_cwd.display().to_string(),
                space,
                durable_id: Some(paneflow_config::schema::WorkspaceId::new()),
                custom_buttons: Vec::new(),
                managed_worktrees,
                tabs,
                active_tab,
            },
            select: false,
        },
    )?
    .launch_error;
    prepared.keep();
    for (tab, preset, focus) in presets {
        if let Some(pane) = focus {
            apply(worker, Op::FocusPane { pane })?;
        }
        apply(worker, Op::ApplyPreset { tab, preset })?;
    }
    apply(
        worker,
        Op::SelectWorkspace {
            workspace: workspace_id,
            focus: true,
        },
    )?;
    let focused = requested_focus.map(|i| panes[i]);
    let built = match focused {
        Some(pane) => apply(worker, Op::FocusPane { pane })?,
        None => Built {
            state: worker.layout.current_state(),
            launch_error: None,
        },
    };
    let index = built
        .state
        .workspace_index(workspace_id)
        .ok_or_else(|| ServeError::internal("the new workspace is missing from the model"))?;
    for (i, pane) in planned.into_iter().enumerate() {
        type_into(worker, &built.state, surfaces[i], pane.command, pane.prompt);
    }
    let mut opened = json!({
        "index": index,
        "title": name,
        "panes": workspace_pane_count(&built.state, built.state.active_workspace),
        "surface_ids": surfaces.iter().map(|surface| surface.get()).collect::<Vec<_>>(),
        "labels": labels,
    });
    if !prepared.notices.is_empty() {
        opened["notices"] = json!(std::mem::take(&mut prepared.notices));
    }
    answer(launch_error, opened)
}

fn created_worktree_cwd(plan: &paneflow_git::worktree_plan::Plan) -> Result<PathBuf, ServeError> {
    canonical_cwd(&plan.path.display().to_string())
        .map_err(|error| invalid(format!("pane {}: {}", plan.pane_idx, error.message)))
}

fn created_worktree(plan: &paneflow_git::worktree_plan::Plan) -> Result<Worktree, ServeError> {
    worktree_of(Some(&ManagedWorktreeSpec {
        path: Some(plan.path.display().to_string()),
        repo_root: Some(plan.repo_root.display().to_string()),
        branch: Some(plan.branch.clone()),
        teardown: Some(plan.teardown.clone()),
    }))
    .ok_or_else(|| {
        ServeError::internal(format!(
            "pane {}: the worktree at {} carries no Paneflow owner marker",
            plan.pane_idx,
            plan.path.display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed_stripped_of_controls_and_invisibles_and_capped() {
        assert_eq!(
            sanitize_pane_name("  build logs  ").as_deref(),
            Some("build logs")
        );
        assert_eq!(
            sanitize_pane_name("ab\ncd\u{7}ef").as_deref(),
            Some("abcdef")
        );
        assert_eq!(
            sanitize_pane_name("build\u{202E}codex\u{200D}").as_deref(),
            Some("buildcodex")
        );
        assert_eq!(sanitize_pane_name("   "), None);
        assert_eq!(
            sanitize_pane_name(&"x".repeat(200)).map(|name| name.len()),
            Some(64)
        );
    }

    #[test]
    fn a_cwd_outside_the_tab_worktree_is_refused_and_a_missing_cwd_starts_in_it() {
        let root = tempfile::tempdir().unwrap();
        let worktree = root.path().join("tree");
        let outside = root.path().join("other");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let tabs = vec![(TabId(1), Some(worktree.clone()))];
        let refused = decide_split(outside.to_str(), None, TabId(1), &tabs, None)
            .expect_err("outside the worktree");
        assert!(
            refused.message.contains("is outside the worktree"),
            "{}",
            refused.message
        );
        let inside = decide_split(None, None, TabId(1), &tabs, None).unwrap();
        assert_eq!(inside.target, Target::Tab(TabId(1)));
        assert_eq!(inside.cwd, Some(worktree.clone()));
        let elsewhere = decide_split(None, Some(&outside), TabId(1), &tabs, None).unwrap();
        assert_eq!(elsewhere.target, Target::NewTab(outside.clone()));
        let bound = decide_split(None, Some(&worktree), TabId(1), &tabs, None).unwrap();
        assert_eq!(bound.target, Target::Tab(TabId(1)));
    }

    #[test]
    fn a_planted_layout_gets_fresh_ids_and_drops_sessions() {
        let layout: LayoutNode = serde_json::from_value(json!({
            "type": "split", "direction": "vertical", "children": [
                {"type": "pane", "surfaces": [{"type": "terminal", "command": "make", "session": "0190e4d2-7b2a-7c3d-8e4f-a1b2c3d4e5f6"}]},
                {"type": "pane", "surfaces": []}
            ]
        }))
        .unwrap();
        let mut counter = 100;
        let planted = plant(&layout, &mut || {
            counter += 1;
            counter
        });
        let NewNode::Split {
            children,
            direction,
            ..
        } = &planted.node
        else {
            panic!("a split")
        };
        assert_eq!(*direction, Direction::Vertical);
        assert_eq!(children.len(), 2);
        assert_eq!(planted.terminals.len(), 2);
        assert_eq!(planted.terminals[0].1.as_deref(), Some("make"));
        let NewNode::Pane(first) = &children[0] else {
            panic!("a pane")
        };
        assert_eq!(first.surfaces[0].definition.session, None);
    }
}
