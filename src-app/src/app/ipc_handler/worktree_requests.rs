use paneflow_api::methods::WorktreeRequestSpec;
use paneflow_git::worktree_plan::{self, Plan, Request};

use super::jsonrpc::JsonRpcError;
use crate::workspace::worktree::ManagedWorktree;

pub(super) fn parse(
    value: Option<&serde_json::Value>,
    cwd: Option<&str>,
) -> Result<Option<Request>, JsonRpcError> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let spec: WorktreeRequestSpec = serde_json::from_value(value.clone())
        .map_err(|error| JsonRpcError::invalid_params(format!("invalid worktree: {error}")))?;
    Ok(Some(Request {
        branch: spec.branch,
        cwd: cwd.map(str::to_owned),
        base: spec.from,
        copy_env: spec.copy_env,
        setup: spec.setup,
        setup_timeout_secs: spec.setup_timeout_secs,
        teardown: spec.teardown,
    }))
}

pub(super) fn runs_setup(request: &Request) -> bool {
    request
        .setup
        .as_deref()
        .is_some_and(|setup| !setup.is_empty())
}

#[derive(Debug, Default)]
pub(super) struct Prepared {
    pub(super) worktrees: Vec<Option<(Plan, ManagedWorktree)>>,
    pub(super) created: Vec<Plan>,
    pub(super) notices: Vec<String>,
}

pub(super) fn prepare(requests: Vec<Option<Request>>) -> Result<Prepared, JsonRpcError> {
    let mut prepared = Prepared {
        worktrees: requests.iter().map(|_| None).collect(),
        ..Prepared::default()
    };
    if requests.iter().all(Option::is_none) {
        return Ok(prepared);
    }
    let mut plans = Vec::with_capacity(requests.len());
    for (index, request) in requests.iter().enumerate() {
        plans.push(
            request
                .as_ref()
                .map(|request| worktree_plan::plan(index, request))
                .transpose()
                .map_err(JsonRpcError::invalid_params)?,
        );
    }
    worktree_plan::resolve_conflicts(&mut plans).map_err(JsonRpcError::invalid_params)?;
    let done =
        worktree_plan::execute_all(plans.iter().flatten()).map_err(JsonRpcError::internal)?;
    prepared.created = done.created;
    prepared.notices = done.notices;
    for (slot, plan) in prepared.worktrees.iter_mut().zip(plans) {
        let Some(plan) = plan else {
            continue;
        };
        let Some(managed) = crate::workspace::worktree::managed_worktree_from_record(
            &plan.path.to_string_lossy(),
            &plan.repo_root.to_string_lossy(),
            &plan.branch,
            &plan.teardown,
        ) else {
            rollback(std::mem::take(&mut prepared.created));
            return Err(JsonRpcError::internal(format!(
                "pane {}: the worktree at {} carries no Paneflow owner marker",
                plan.pane_idx,
                plan.path.display()
            )));
        };
        *slot = Some((plan, managed));
    }
    Ok(prepared)
}

pub(super) fn rollback(created: Vec<Plan>) {
    let created: Vec<&Plan> = created.iter().collect();
    for notice in worktree_plan::rollback(&created) {
        log::warn!("paneflow: {notice}");
    }
}

pub(super) fn failed(reply: &serde_json::Value) -> bool {
    reply
        .get(super::jsonrpc::JSONRPC_ERROR_KEY)
        .is_some_and(|error| !error.is_null())
}
