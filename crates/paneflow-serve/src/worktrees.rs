use std::path::PathBuf;

use paneflow_api::methods::WorktreeRequestSpec;
use paneflow_config::schema::{PaneFlowConfig, SpaceId};
use paneflow_git::worktree_plan::{self, Plan, Request};
use paneflow_layout::State;

use crate::layout_authority::orchestration_refused;
use crate::registry::ServeError;

pub(crate) fn request_of(spec: WorktreeRequestSpec, cwd: Option<String>) -> Request {
    Request {
        branch: spec.branch,
        cwd,
        base: spec.from,
        copy_env: spec.copy_env,
        setup: spec.setup,
        setup_timeout_secs: spec.setup_timeout_secs,
        teardown: spec.teardown,
    }
}

pub(crate) struct Prepared {
    pub(crate) plans: Vec<Option<Plan>>,
    pub(crate) notices: Vec<String>,
    created: Vec<Plan>,
    armed: bool,
}

impl Prepared {
    pub(crate) fn keep(&mut self) {
        self.armed = false;
    }
}

impl Drop for Prepared {
    fn drop(&mut self) {
        if !self.armed || self.created.is_empty() {
            return;
        }
        let created: Vec<&Plan> = self.created.iter().collect();
        for notice in worktree_plan::rollback(&created) {
            log::warn!("paneflow-serve: {notice}");
        }
        log::info!(
            "paneflow-serve: rolled back {} worktree(s) the layout did not take",
            created.len()
        );
    }
}

pub(crate) fn prepare(
    config: &PaneFlowConfig,
    state: &State,
    space: Option<&SpaceId>,
    requests: Vec<Option<Request>>,
    method: &str,
    may_spawn: bool,
) -> Result<Prepared, ServeError> {
    let mut prepared = Prepared {
        plans: requests.iter().map(|_| None).collect(),
        notices: Vec::new(),
        created: Vec::new(),
        armed: true,
    };
    if requests.iter().all(Option::is_none) {
        return Ok(prepared);
    }
    paneflow_git::worktree::set_worktrees_root(config.worktrees.dir_path());
    let mut plans = Vec::with_capacity(requests.len());
    for (index, request) in requests.iter().enumerate() {
        plans.push(
            request
                .as_ref()
                .map(|request| worktree_plan::plan(index, request))
                .transpose()
                .map_err(ServeError::invalid_params)?,
        );
    }
    worktree_plan::resolve_conflicts(&mut plans).map_err(ServeError::invalid_params)?;
    if plans.iter().flatten().any(Plan::runs_setup) && !may_spawn {
        return Err(orchestration_refused(method));
    }
    publish_git_env(config, state, &plans, space);
    let done = worktree_plan::execute_all(plans.iter().flatten()).map_err(ServeError::internal)?;
    for notice in &done.notices {
        log::info!("paneflow-serve: {notice}");
    }
    prepared.plans = plans;
    prepared.notices = done.notices;
    prepared.created = done.created;
    Ok(prepared)
}

fn workspace_directories(cwd: &str, worktrees: impl Iterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if !cwd.is_empty() {
        dirs.push(PathBuf::from(cwd));
        if let Some(repo_root) = paneflow_git::repo::find_git_dir(cwd)
            .and_then(|git_dir| paneflow_git::repo::resolve_repo_root(&git_dir).0)
        {
            dirs.push(repo_root);
        }
    }
    dirs.extend(worktrees);
    dirs.sort();
    dirs.dedup();
    dirs
}

fn publish_git_env(
    config: &PaneFlowConfig,
    state: &State,
    plans: &[Option<Plan>],
    space: Option<&SpaceId>,
) {
    let mut entries: Vec<(PathBuf, Vec<(String, String)>)> = Vec::new();
    for ws in &state.workspaces {
        let env = paneflow_launch::space::subprocess_env(config, ws.space.as_ref());
        let worktrees = ws
            .tabs
            .iter()
            .filter_map(|tab| tab.worktree.as_ref().map(PathBuf::from));
        for dir in workspace_directories(&ws.cwd, worktrees) {
            entries.push((dir, env.clone()));
        }
    }
    let env = paneflow_launch::space::subprocess_env(config, space);
    for plan in plans.iter().flatten() {
        entries.push((plan.repo_root.clone(), env.clone()));
        entries.push((plan.path.clone(), env.clone()));
    }
    paneflow_git::directory_env::publish(entries);
}
