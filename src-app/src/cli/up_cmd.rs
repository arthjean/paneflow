//! `paneflow up <file>` - declarative agent workspaces (US-009/US-011/US-012;
//! worktree-per-agent: EP-002 of prd-orchestration-v2).
//!
//! Loads a `paneflow.workspace.toml`, resolves each pane's `agent` to its CLI
//! launch command (verifying the binary is on PATH for an atomic failure),
//! plans (and, outside `--dry-run`, creates) one git worktree per pane that
//! asks for one, substitutes `${port_offset}` in env values, and calls the
//! `workspace.up` IPC method. `--dry-run` prints the resolved plan without
//! touching the running instance OR the filesystem - worktree planning is
//! read-only (`git worktree list`), so a locked branch is detected without
//! creating anything.
//!
//! All of this runs in the CLI process: git subprocesses can never block the
//! GPUI render thread by construction (FR-09).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use paneflow_config::schema::PaneFlowConfig;
use paneflow_ipc_client::IpcTransport;
use serde_json::{Value, json};

use super::workspace_spec::{self, PaneSpec};
use super::{CliError, EXIT_OK};
use crate::agent_launcher::TerminalAgent;
use crate::workspace::worktree;

/// Default base port for `${port_offset}` (US-008).
pub(super) const DEFAULT_PORT_BASE: u16 = 3000;
/// Ports reserved per pane referencing `${port_offset}`.
const PORT_STRIDE: u16 = 10;
/// Default wall-clock bound for a pane's `setup` command (US-007).
const DEFAULT_SETUP_TIMEOUT: Duration = Duration::from_secs(300);

/// `paneflow up <file> [--dry-run]`.
pub fn up(client: &impl IpcTransport, file: &str, dry_run: bool) -> Result<i32, CliError> {
    let src = std::fs::read_to_string(file)
        .map_err(|e| CliError::runtime(format!("cannot read '{file}': {e}")))?;
    let spec = workspace_spec::load(&src).map_err(CliError::runtime)?;

    // Phase 1 (pure): validate every `${…}` token and find which panes
    // reference `${port_offset}` - an unknown variable fails atomically
    // before any git or network side effect (US-008).
    let port_refs = validate_env_tokens(&spec.panes)?;

    // Phase 2 (read-only git): plan worktrees. Locked branches, non-repo
    // cwds and path conflicts are all caught here, before anything mutates -
    // which is also exactly what `--dry-run` reports (US-006).
    let mut worktree_plans: Vec<Option<WorktreePlan>> = Vec::with_capacity(spec.panes.len());
    for (idx, pane) in spec.panes.iter().enumerate() {
        worktree_plans.push(plan_worktree(idx, pane)?);
    }
    check_worktree_conflicts(&mut worktree_plans)?;

    // Phase 3: allocate a free port stride per referencing pane (bind-probe;
    // cross-platform - no /proc parsing, works on Windows too).
    let port_base = spec.port_base.unwrap_or(DEFAULT_PORT_BASE);
    let offsets = allocate_port_offsets(&port_refs, port_base, port_is_free);

    let config = paneflow_config::loader::load_config();
    let mut panes = Vec::with_capacity(spec.panes.len());
    for (idx, pane) in spec.panes.iter().enumerate() {
        let command = resolve_command(idx, pane, &config)?;
        let env = substitute_env(pane.env.as_ref(), offsets[idx]);
        let plan = &worktree_plans[idx];
        let cwd = match plan {
            Some(p) => Some(p.path.to_string_lossy().into_owned()),
            None => pane.cwd.clone(),
        };
        panes.push(json!({
            "cwd": cwd,
            "command": command,
            "prompt": pane.prompt,
            "focus": pane.focus,
            "env": env,
            "name": pane.name,
            "profile": if pane.agent.is_some() { "agent" } else { "normal" },
            "managed_worktree": plan.as_ref().map(|p| p.managed_json()),
        }));
    }

    let params = json!({
        "name": spec.name,
        "layout": spec.layout.as_ipc(),
        "panes": panes,
    });

    if dry_run {
        // Print the resolved plan (agents -> commands, worktree actions,
        // allocated ports) without touching the instance or the filesystem.
        super::print_json(&params)?;
        return Ok(EXIT_OK);
    }

    ensure_orchestration_gate(client)?;

    // Phase 4 (mutating, still CLI-side): create the planned worktrees, copy
    // `.env*`, run `setup`. A creation failure aborts before workspace.up so
    // no half-spawned workspace points at a missing directory.
    for plan in worktree_plans.iter().flatten() {
        execute_worktree_plan(plan)?;
    }

    let result = client
        .call("workspace.up", params)
        .map_err(CliError::runtime)?;
    super::print_json(&result)?;
    Ok(EXIT_OK)
}

fn ensure_orchestration_gate(client: &impl IpcTransport) -> Result<(), CliError> {
    let caps = client
        .call("system.capabilities", json!({}))
        .map_err(CliError::runtime)?;
    let orchestration = caps.get("orchestration").and_then(Value::as_bool);
    let scripting = caps.get("scripting").and_then(Value::as_bool);
    match (orchestration, scripting) {
        (Some(true), _) | (_, Some(true)) | (None, _) => Ok(()),
        (Some(false), _) => Err(CliError::runtime(
            "workspace up requires pane orchestration: relaunch Paneflow with \
             PANEFLOW_IPC_ORCHESTRATION=1",
        )),
    }
}

// ---------------------------------------------------------------------------
// Worktree planning + execution (EP-002 US-006/US-007)
// ---------------------------------------------------------------------------

/// Everything needed to create (or reuse) one pane's worktree.
#[derive(Debug)]
pub(super) struct WorktreePlan {
    pane_idx: usize,
    repo_root: PathBuf,
    pub(super) path: PathBuf,
    branch: String,
    /// `Some(true)` → `git worktree add -b` (new branch), `Some(false)` →
    /// existing branch, `None` → the worktree already exists (reuse, no add).
    create: Option<bool>,
    copy_env: bool,
    setup: Option<String>,
    setup_timeout: Duration,
    teardown: String,
}

impl WorktreePlan {
    fn create_branch_known(&self) -> bool {
        self.create.is_some()
    }

    /// The `managed_worktree` JSON object handed to the server (ownership
    /// record for close-time teardown, US-009). Shared by `up` and `flow`.
    pub(super) fn managed_json(&self) -> serde_json::Value {
        json!({
            "path": self.path.to_string_lossy(),
            "repo_root": self.repo_root.to_string_lossy(),
            "branch": self.branch,
            "teardown": self.teardown,
            "action": if self.create_branch_known() { "create" } else { "reuse" },
        })
    }
}

/// Resolve one pane's `worktree` field into a [`WorktreePlan`] (read-only:
/// safe under `--dry-run`). Errors are pane-indexed and atomic - nothing has
/// been created when they surface.
pub(super) fn plan_worktree(idx: usize, pane: &PaneSpec) -> Result<Option<WorktreePlan>, CliError> {
    let Some(branch) = pane.worktree.as_deref() else {
        return Ok(None);
    };
    // `worktree` requires `cwd` - enforced by spec validation.
    let cwd = expand_tilde(pane.cwd.as_deref().unwrap_or_default());
    let git_dir = crate::workspace::find_git_dir(&cwd).ok_or_else(|| {
        CliError::runtime(format!(
            "pane {idx}: cwd '{cwd}' is not inside a git repository (required by `worktree`)"
        ))
    })?;
    let (repo_root, _) = crate::workspace::resolve_repo_root(&git_dir);
    let repo_root = repo_root.ok_or_else(|| {
        CliError::runtime(format!(
            "pane {idx}: cannot resolve the repository root from '{cwd}'"
        ))
    })?;
    let legacy_path = worktree::worktree_dir(&repo_root, branch);
    let hashed_path = worktree::worktree_dir_hashed(&repo_root, branch);
    let mut path = legacy_path.clone();

    let entries = worktree::list_worktrees(&repo_root)
        .map_err(|e| CliError::runtime(format!("pane {idx}: {e}")))?;
    let mut create: Option<bool> = Some(!worktree::branch_exists(&repo_root, branch));
    for entry in &entries {
        if entry.branch.as_deref() == Some(branch) {
            if worktree::is_paneflow_worktree_dir(&repo_root, branch, &entry.path) {
                path = entry.path.clone();
                create = None;
            } else {
                // Locked: a branch can only be checked out in one worktree.
                return Err(CliError::runtime(format!(
                    "pane {idx}: branch '{branch}' already checked out at {}",
                    entry.path.display()
                )));
            }
        }
    }
    if create.is_some()
        && entries
            .iter()
            .any(|entry| entry.path == legacy_path && entry.branch.as_deref() != Some(branch))
    {
        path = hashed_path;
    }
    validate_worktree_target(idx, &mut path, &repo_root, branch, &entries, &mut create)?;
    if create.is_some() && path.exists() {
        return Err(CliError::runtime(format!(
            "pane {idx}: {} exists but is not a registered worktree; remove it first",
            path.display()
        )));
    }

    Ok(Some(WorktreePlan {
        pane_idx: idx,
        repo_root,
        path,
        branch: branch.to_string(),
        create,
        copy_env: pane.copy_env.unwrap_or(true),
        setup: pane.setup.clone(),
        setup_timeout: Duration::from_secs(
            pane.setup_timeout_secs
                .unwrap_or(DEFAULT_SETUP_TIMEOUT.as_secs()),
        ),
        teardown: pane
            .worktree_teardown
            .clone()
            .unwrap_or_else(|| "auto".to_string()),
    }))
}

fn validate_worktree_target(
    idx: usize,
    path: &mut PathBuf,
    repo_root: &Path,
    branch: &str,
    entries: &[worktree::WorktreeEntry],
    create: &mut Option<bool>,
) -> Result<(), CliError> {
    for entry in entries {
        if entry.path != *path {
            continue;
        }
        if entry.branch.as_deref() == Some(branch) {
            *path = entry.path.clone();
            *create = None;
            return Ok(());
        }
        return Err(CliError::runtime(format!(
            "pane {idx}: {} exists but holds another branch ({}); \
             remove it or pick another worktree branch",
            path.display(),
            entry.branch.as_deref().unwrap_or("detached")
        )));
    }
    if !worktree::is_paneflow_worktree_dir(repo_root, branch, path) {
        return Err(CliError::runtime(format!(
            "pane {idx}: planned worktree path {} is outside Paneflow's worktree dirs",
            path.display()
        )));
    }
    Ok(())
}

/// Two panes must not target the same worktree path. Same-branch duplicates
/// are refused; different branches that slug to the same path are moved to a
/// hashed fallback before any mutation so `--dry-run` reports the final plan.
pub(super) fn check_worktree_conflicts(plans: &mut [Option<WorktreePlan>]) -> Result<(), CliError> {
    let mut groups: HashMap<PathBuf, Vec<usize>> = HashMap::new();
    for (idx, plan) in plans.iter().enumerate() {
        if let Some(plan) = plan {
            groups.entry(plan.path.clone()).or_default().push(idx);
        }
    }

    for indices in groups.values().filter(|indices| indices.len() > 1) {
        let mut branches: HashMap<String, usize> = HashMap::new();
        for &idx in indices {
            let plan = plans[idx].as_ref().expect("group contains plan index");
            if let Some(&first) = branches.get(&plan.branch) {
                return Err(CliError::runtime(format!(
                    "pane {} and pane {} both target worktree '{}' ({}) - \
                     a branch can only be checked out in one worktree",
                    first,
                    plan.pane_idx,
                    plan.branch,
                    plan.path.display()
                )));
            }
            branches.insert(plan.branch.clone(), plan.pane_idx);
        }

        let keep = indices
            .iter()
            .copied()
            .find(|&idx| {
                plans[idx]
                    .as_ref()
                    .is_some_and(|plan| plan.create.is_none())
            })
            .unwrap_or(indices[0]);
        for &idx in indices {
            if idx == keep {
                continue;
            }
            let plan = plans[idx].as_mut().expect("group contains plan index");
            let hashed = worktree::worktree_dir_hashed(&plan.repo_root, &plan.branch);
            if plan.path == hashed {
                continue;
            }
            plan.path = hashed;
            let entries = worktree::list_worktrees(&plan.repo_root)
                .map_err(|e| CliError::runtime(format!("pane {}: {e}", plan.pane_idx)))?;
            let mut create = plan.create;
            validate_worktree_target(
                plan.pane_idx,
                &mut plan.path,
                &plan.repo_root,
                &plan.branch,
                &entries,
                &mut create,
            )?;
            plan.create = create;
            if plan.create.is_some() && plan.path.exists() {
                return Err(CliError::runtime(format!(
                    "pane {}: {} exists but is not a registered worktree; remove it first",
                    plan.pane_idx,
                    plan.path.display()
                )));
            }
        }
    }

    let mut seen: HashMap<&PathBuf, usize> = HashMap::new();
    for plan in plans.iter().flatten() {
        if let Some(&first) = seen.get(&plan.path) {
            return Err(CliError::runtime(format!(
                "pane {} and pane {} both target worktree '{}' ({}) - \
                 a branch can only be checked out in one worktree",
                first,
                plan.pane_idx,
                plan.branch,
                plan.path.display()
            )));
        }
        seen.insert(&plan.path, plan.pane_idx);
    }
    Ok(())
}

/// Create the worktree and bootstrap its environment. `.env*` copy and
/// `setup` run on CREATION only - a reused worktree already had its bootstrap
/// (and re-running an install behind the user's back would be a surprise).
/// `setup` failure warns and continues (US-007 AC4): the human can fix it in
/// the pane; a broken install must not block the agent launch.
pub(super) fn execute_worktree_plan(plan: &WorktreePlan) -> Result<(), CliError> {
    let Some(create_branch) = plan.create else {
        return Ok(()); // reuse - nothing to do
    };
    worktree::add_worktree(&plan.repo_root, &plan.path, &plan.branch, create_branch)
        .map_err(|e| CliError::runtime(format!("pane {}: {e}", plan.pane_idx)))?;

    if plan.copy_env {
        let copied = worktree::copy_env_files(&plan.repo_root, &plan.path);
        if !copied.is_empty() {
            eprintln!(
                "pane {}: copied {} into {}",
                plan.pane_idx,
                copied.join(", "),
                plan.path.display()
            );
        }
    }

    if let Some(setup) = plan.setup.as_deref().filter(|s| !s.is_empty()) {
        #[cfg(unix)]
        let mut cmd = {
            let mut c = std::process::Command::new("sh");
            c.arg("-c").arg(setup);
            c
        };
        #[cfg(windows)]
        let mut cmd = {
            let mut c = std::process::Command::new("cmd");
            c.arg("/C").arg(setup);
            c
        };
        cmd.current_dir(&plan.path);
        match paneflow_process::run_with_timeout(cmd, plan.setup_timeout, 256 * 1024) {
            Ok(out) if out.status.success() => {}
            Ok(out) => eprintln!(
                "pane {}: setup failed in {} (exit {}) - agent started anyway",
                plan.pane_idx,
                plan.path.display(),
                out.status.code().unwrap_or(-1)
            ),
            Err(e) => eprintln!(
                "pane {}: setup failed in {} ({e}) - agent started anyway",
                plan.pane_idx,
                plan.path.display()
            ),
        }
    }
    Ok(())
}

/// Expand a leading `~` / `~/` to the home directory (the server does this
/// via `canonicalize_workspace_cwd`, but worktree planning needs the real
/// path CLI-side, before any IPC round-trip).
fn expand_tilde(raw: &str) -> String {
    if raw == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.to_string_lossy().into_owned();
        }
    } else if let Some(rest) = raw.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest).to_string_lossy().into_owned();
    }
    raw.to_string()
}

// ---------------------------------------------------------------------------
// `${port_offset}` (EP-002 US-008)
// ---------------------------------------------------------------------------

/// Validate every `${…}` token across all panes' env values. Returns, per
/// pane, whether it references `${port_offset}`. The only supported variable
/// is `port_offset` - anything else (or an unclosed `${`) is an atomic
/// validation error naming the supported set.
pub(super) fn validate_env_tokens(panes: &[PaneSpec]) -> Result<Vec<bool>, CliError> {
    let mut refs = Vec::with_capacity(panes.len());
    for (idx, pane) in panes.iter().enumerate() {
        refs.push(validate_pane_env_tokens(idx, pane)?);
    }
    Ok(refs)
}

pub(super) fn validate_pane_env_tokens(idx: usize, pane: &PaneSpec) -> Result<bool, CliError> {
    let mut references = false;
    if let Some(env) = pane.env.as_ref() {
        for (key, value) in env {
            for token in extract_tokens(value)
                .map_err(|e| CliError::runtime(format!("pane {idx}: env `{key}`: {e}")))?
            {
                if token == "port_offset" {
                    references = true;
                } else {
                    return Err(CliError::runtime(format!(
                        "pane {idx}: env `{key}` references unknown variable \
                         '${{{token}}}' (supported: ${{port_offset}})"
                    )));
                }
            }
        }
    }
    Ok(references)
}

/// All `${…}` token names in a string. Unclosed `${` is an error.
pub(super) fn extract_tokens(value: &str) -> Result<Vec<&str>, String> {
    let mut tokens = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            return Err("unclosed `${`".to_string());
        };
        tokens.push(&after[..end]);
        rest = &after[end + 1..];
    }
    Ok(tokens)
}

/// Allocate one free port stride per referencing pane: the k-th referencing
/// pane gets the k-th stride of `PORT_STRIDE` above `port_base` whose full
/// port range probes free. `is_free` is injected so the policy is unit-testable
/// without binding sockets.
pub(super) fn allocate_port_offsets(
    refs: &[bool],
    port_base: u16,
    is_free: impl Fn(u16) -> bool,
) -> Vec<Option<u16>> {
    let mut next_stride: u16 = 0;
    refs.iter()
        .map(|wants| {
            if !*wants {
                return None;
            }
            loop {
                let candidate = next_stride
                    .checked_mul(PORT_STRIDE)
                    .and_then(|offset| port_base.checked_add(offset))
                    .unwrap_or(u16::MAX);
                next_stride = next_stride.saturating_add(1);
                if candidate == u16::MAX || port_stride_is_free(candidate, &is_free) {
                    return Some(candidate);
                }
            }
        })
        .collect()
}

fn port_stride_is_free(port_base: u16, is_free: &impl Fn(u16) -> bool) -> bool {
    (0..PORT_STRIDE).all(|offset| port_base.checked_add(offset).is_some_and(is_free))
}

/// Bind-probe: can we listen on this port right now? Cross-platform (real
/// check on Windows too, unlike the /proc-based scan in `workspace::ports`).
pub(super) fn port_is_free(port: u16) -> bool {
    use std::io::ErrorKind;

    if std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).is_err() {
        return false;
    }
    match std::net::TcpListener::bind((std::net::Ipv6Addr::LOCALHOST, port)) {
        Ok(_) => true,
        Err(e)
            if matches!(
                e.kind(),
                ErrorKind::AddrNotAvailable | ErrorKind::Unsupported
            ) =>
        {
            true
        }
        Err(_) => false,
    }
}

/// Substitute `${port_offset}` in env values. Values without the token pass
/// through untouched; `offset == None` (pane doesn't reference it) is a no-op.
pub(super) fn substitute_env(
    env: Option<&HashMap<String, String>>,
    offset: Option<u16>,
) -> Option<HashMap<String, String>> {
    let env = env?;
    let Some(offset) = offset else {
        return Some(env.clone());
    };
    Some(
        env.iter()
            .map(|(k, v)| (k.clone(), v.replace("${port_offset}", &offset.to_string())))
            .collect(),
    )
}

/// Resolve a pane's launch command: an `agent` maps to its CLI launch command
/// (and the binary is verified on PATH for an atomic failure before any pane
/// spawns, US-012); a raw `command` passes through; neither leaves a bare shell.
pub(super) fn resolve_command(
    idx: usize,
    pane: &PaneSpec,
    config: &PaneFlowConfig,
) -> Result<Option<String>, CliError> {
    let Some(agent) = pane.agent.as_deref() else {
        return Ok(pane.command.clone());
    };
    let resolved = resolve_agent(agent)
        .ok_or_else(|| CliError::runtime(format!("pane {idx}: unknown agent '{agent}'")))?;
    if !resolved.is_installed() {
        return Err(CliError::runtime(format!(
            "pane {idx}: agent '{agent}' ({}) not found on PATH",
            resolved.binary()
        )));
    }
    Ok(Some(resolved.launch_command(config)))
}

/// Map a friendly agent name from the spec to a [`TerminalAgent`]. Accepts
/// hyphen or underscore separators and the bare `claude` alias for Claude Code.
fn resolve_agent(name: &str) -> Option<TerminalAgent> {
    let normalized = name.trim().to_lowercase().replace('-', "_");
    let tag = match normalized.as_str() {
        "claude" => "claude_code",
        other => other,
    };
    TerminalAgent::from_tag(tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_agent_accepts_aliases() {
        assert_eq!(resolve_agent("claude"), Some(TerminalAgent::ClaudeCode));
        assert_eq!(
            resolve_agent("claude-code"),
            Some(TerminalAgent::ClaudeCode)
        );
        assert_eq!(
            resolve_agent("Claude_Code"),
            Some(TerminalAgent::ClaudeCode)
        );
        assert_eq!(resolve_agent("codex"), Some(TerminalAgent::Codex));
        assert_eq!(resolve_agent("gemini"), Some(TerminalAgent::Gemini));
        assert_eq!(resolve_agent("nope"), None);
    }

    fn pane(agent: Option<&str>, command: Option<&str>) -> PaneSpec {
        PaneSpec {
            cwd: None,
            agent: agent.map(str::to_string),
            command: command.map(str::to_string),
            prompt: None,
            focus: None,
            env: None,
            name: None,
            worktree: None,
            copy_env: None,
            setup: None,
            setup_timeout_secs: None,
            worktree_teardown: None,
        }
    }

    fn pane_with_env(pairs: &[(&str, &str)]) -> PaneSpec {
        let mut p = pane(None, Some("true"));
        p.env = Some(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        );
        p
    }

    // --- ${port_offset} (US-008) ---

    #[test]
    fn extract_tokens_finds_all_and_rejects_unclosed() {
        assert_eq!(extract_tokens("${a} x ${b}").expect("ok"), vec!["a", "b"]);
        assert!(extract_tokens("plain").expect("ok").is_empty());
        assert!(extract_tokens("${unclosed").is_err());
    }

    #[test]
    fn validate_env_tokens_flags_referencing_panes_and_rejects_unknown() {
        let panes = vec![
            pane_with_env(&[("PORT", "${port_offset}")]),
            pane(None, Some("true")),
        ];
        assert_eq!(validate_env_tokens(&panes).expect("ok"), vec![true, false]);

        let bad = vec![pane_with_env(&[("X", "${typo}")])];
        let err = validate_env_tokens(&bad).unwrap_err();
        assert_eq!(err.code, crate::cli::EXIT_RUNTIME);
        assert!(err.message.contains("typo"), "got: {}", err.message);
        assert!(
            err.message.contains("port_offset"),
            "must name the supported set: {}",
            err.message
        );
    }

    #[test]
    fn allocate_port_offsets_skips_busy_strides() {
        // Pane 0 and 2 reference the variable; 3010 is "busy" → pane 2 jumps
        // to 3020 (US-008 AC2). Non-referencing panes get None.
        let refs = vec![true, false, true];
        let offsets = allocate_port_offsets(&refs, 3000, |p| p != 3010);
        assert_eq!(offsets, vec![Some(3000), None, Some(3020)]);
    }

    #[test]
    fn allocate_port_offsets_checks_the_whole_stride() {
        let refs = vec![true, true];
        let offsets = allocate_port_offsets(&refs, 3000, |p| p != 3005);
        assert_eq!(offsets, vec![Some(3010), Some(3020)]);
    }

    #[test]
    fn substitute_env_replaces_only_the_supported_token() {
        let mut env = HashMap::new();
        env.insert("PORT".to_string(), "${port_offset}".to_string());
        env.insert("PLAIN".to_string(), "untouched".to_string());
        let out = substitute_env(Some(&env), Some(3010)).expect("some");
        assert_eq!(out["PORT"], "3010");
        assert_eq!(out["PLAIN"], "untouched", "passthrough exact (AC4)");
        // No offset allocated (pane doesn't reference it): exact clone.
        let out = substitute_env(Some(&env), None).expect("some");
        assert_eq!(out["PORT"], "${port_offset}");
    }

    // --- worktree planning (US-006) ---

    #[test]
    fn plan_worktree_none_without_field() {
        assert!(
            plan_worktree(0, &pane(None, Some("true")))
                .expect("ok")
                .is_none()
        );
    }

    #[test]
    fn plan_worktree_outside_a_repo_is_an_atomic_error() {
        // US-006 AC5: a cwd outside any git repository fails the plan phase
        // (before anything is created), with a pane-indexed message.
        let mut p = pane(Some("claude"), None);
        p.cwd = Some("/".to_string());
        p.worktree = Some("feat/x".to_string());
        let err = plan_worktree(3, &p).unwrap_err();
        assert_eq!(err.code, crate::cli::EXIT_RUNTIME);
        assert!(err.message.contains("pane 3"), "got: {}", err.message);
        assert!(
            err.message.contains("not inside a git repository"),
            "got: {}",
            err.message
        );
    }

    fn dummy_plan(idx: usize, path: &str, branch: &str) -> WorktreePlan {
        WorktreePlan {
            pane_idx: idx,
            repo_root: PathBuf::from("/r"),
            path: PathBuf::from(path),
            branch: branch.to_string(),
            create: Some(true),
            copy_env: true,
            setup: None,
            setup_timeout: Duration::from_secs(1),
            teardown: "auto".to_string(),
        }
    }

    #[test]
    fn duplicate_worktree_paths_are_an_atomic_validation_error() {
        // NFR fail-atomique: two panes on the same branch must be refused at
        // the plan stage (both panes cited), never discovered mid-execution
        // after the first worktree was already created.
        let mut plans = vec![
            Some(dummy_plan(0, "/r.worktrees/feat-x", "feat/x")),
            None,
            Some(dummy_plan(2, "/r.worktrees/feat-x", "feat/x")),
        ];
        let err = check_worktree_conflicts(&mut plans).unwrap_err();
        assert_eq!(err.code, crate::cli::EXIT_RUNTIME);
        assert!(
            err.message.contains("pane 0") && err.message.contains("pane 2"),
            "both panes cited: {}",
            err.message
        );

        let mut distinct = vec![
            Some(dummy_plan(0, "/r.worktrees/feat-x", "feat/x")),
            Some(dummy_plan(1, "/r.worktrees/feat-y", "feat/y")),
        ];
        assert!(check_worktree_conflicts(&mut distinct).is_ok());
    }

    #[test]
    fn slug_collision_branches_move_to_hashed_fallback() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo_root = tmp.path().join("repo");
        std::fs::create_dir(&repo_root).expect("repo dir");
        if !test_git(&repo_root, &["init"]) {
            return;
        }

        let branch_a = "feat/a b";
        let branch_b = "feat/a-b";
        let legacy = worktree::worktree_dir(&repo_root, branch_a);
        assert_eq!(legacy, worktree::worktree_dir(&repo_root, branch_b));
        let hashed_b = worktree::worktree_dir_hashed(&repo_root, branch_b);

        let mut plans = vec![
            Some(WorktreePlan {
                pane_idx: 0,
                repo_root: repo_root.clone(),
                path: legacy.clone(),
                branch: branch_a.to_string(),
                create: Some(true),
                copy_env: true,
                setup: None,
                setup_timeout: Duration::from_secs(1),
                teardown: "auto".to_string(),
            }),
            Some(WorktreePlan {
                pane_idx: 1,
                repo_root: repo_root.clone(),
                path: legacy.clone(),
                branch: branch_b.to_string(),
                create: Some(true),
                copy_env: true,
                setup: None,
                setup_timeout: Duration::from_secs(1),
                teardown: "auto".to_string(),
            }),
        ];

        check_worktree_conflicts(&mut plans).expect("slug collision resolves");
        assert_eq!(plans[0].as_ref().unwrap().path, legacy);
        assert_eq!(plans[1].as_ref().unwrap().path, hashed_b);
    }

    #[test]
    fn expand_tilde_expands_home_prefix_only() {
        let home = dirs::home_dir()
            .expect("home")
            .to_string_lossy()
            .into_owned();
        assert_eq!(expand_tilde("~"), home);
        assert!(expand_tilde("~/dev").starts_with(&home));
        assert_eq!(expand_tilde("/abs/path"), "/abs/path");
        assert_eq!(expand_tilde("rel/~nothome"), "rel/~nothome");
    }

    #[test]
    fn resolve_command_rejects_unknown_agent() {
        // US-007/US-012: an unrecognized agent fails with a pane-indexed,
        // non-zero error before any pane spawns - not a silent fallthrough.
        let cfg = PaneFlowConfig::default();
        let err = resolve_command(2, &pane(Some("cloode"), None), &cfg).unwrap_err();
        assert_eq!(err.code, crate::cli::EXIT_RUNTIME);
        assert!(err.message.contains("pane 2"), "got: {}", err.message);
        assert!(err.message.contains("cloode"), "got: {}", err.message);
    }

    #[test]
    fn resolve_command_passes_raw_command_through() {
        // A raw `command` (no `agent`) is used verbatim, with no PATH check.
        let cfg = PaneFlowConfig::default();
        let resolved = resolve_command(0, &pane(None, Some("cargo watch")), &cfg).expect("ok");
        assert_eq!(resolved.as_deref(), Some("cargo watch"));
    }

    fn test_git(cwd: &std::path::Path, args: &[&str]) -> bool {
        std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }
}
