use std::collections::HashMap;

use paneflow_config::schema::PaneFlowConfig;
use paneflow_ipc_client::IpcTransport;
use serde_json::{Value, json};

use super::workspace_spec::{self, PaneSpec};
use super::{CliError, EXIT_OK};
use crate::agent_launcher::TerminalAgent;
use crate::workspace::worktree;

pub(super) const DEFAULT_PORT_BASE: u16 = 3000;
const PORT_STRIDE: u16 = 10;

pub fn up(client: &impl IpcTransport, file: &str, dry_run: bool) -> Result<i32, CliError> {
    let src = std::fs::read_to_string(file)
        .map_err(|e| CliError::runtime(format!("cannot read '{file}': {e}")))?;
    let spec = workspace_spec::load(&src).map_err(CliError::runtime)?;

    let port_refs = validate_env_tokens(&spec.panes)?;

    let mut worktree_plans: Vec<Option<WorktreePlan>> = Vec::with_capacity(spec.panes.len());
    for (idx, pane) in spec.panes.iter().enumerate() {
        worktree_plans.push(plan_worktree(idx, pane)?);
    }
    check_worktree_conflicts(&mut worktree_plans)?;

    let port_base = spec.port_base.unwrap_or(DEFAULT_PORT_BASE);
    let offsets = allocate_port_offsets(&port_refs, port_base, port_is_free)?;

    let config = paneflow_config::loader::load_config();
    check_space(&config, spec.space.as_deref())?;
    worktree::set_worktrees_root(config.worktrees.dir_path());
    let mut panes = Vec::with_capacity(spec.panes.len());
    for (idx, pane) in spec.panes.iter().enumerate() {
        let command = resolve_command(idx, pane, &config)?;
        let env = sandbox_env(
            idx,
            pane,
            &config,
            substitute_env(pane.env.as_ref(), offsets[idx]),
        )?;
        let mut pane_params = json!({
            "cwd": pane.cwd,
            "command": command,
            "prompt": pane.prompt,
            "focus": pane.focus,
            "env": env,
            "name": pane.name,
            "profile": if pane.agent.is_some() { "agent" } else { "normal" },
        });
        if let Some(request) = worktree_request(pane) {
            pane_params["worktree"] = worktree_request_json(&request);
        }
        panes.push(pane_params);
    }

    let params = json!({
        "name": spec.name,
        "space": spec.space,
        "layout": spec.layout.as_ipc(),
        "panes": panes,
    });

    if dry_run {
        let mut preview = params;
        for (pane, plan) in preview["panes"]
            .as_array_mut()
            .into_iter()
            .flatten()
            .zip(&worktree_plans)
        {
            if let Some(plan) = plan {
                pane["worktree_plan"] = managed_json(plan);
            }
        }
        super::print_json(&preview)?;
        return Ok(EXIT_OK);
    }

    if panes_need_orchestration(&spec.panes) {
        ensure_orchestration_gate(client)?;
    }

    let result = client
        .call("workspace.up", params)
        .map_err(CliError::runtime)?;
    let result = super::reject_legacy_error(result)?;
    for notice in result["notices"].as_array().into_iter().flatten() {
        if let Some(notice) = notice.as_str() {
            eprintln!("{notice}");
        }
    }
    super::print_json(&result)?;
    Ok(EXIT_OK)
}

pub(super) fn worktree_request_json(request: &paneflow_git::worktree_plan::Request) -> Value {
    json!({
        "branch": request.branch,
        "from": request.base,
        "copy_env": request.copy_env,
        "setup": request.setup,
        "setup_timeout_secs": request.setup_timeout_secs,
        "teardown": request.teardown,
    })
}

fn check_space(config: &PaneFlowConfig, space: Option<&str>) -> Result<(), CliError> {
    match space {
        Some(value) if crate::app::spaces::find_space(config, value).is_none() => {
            Err(CliError::runtime(format!("unknown space: {value}")))
        }
        _ => Ok(()),
    }
}

fn panes_need_orchestration(panes: &[PaneSpec]) -> bool {
    panes.iter().any(pane_needs_orchestration)
}

fn pane_needs_orchestration(pane: &PaneSpec) -> bool {
    pane.agent.is_some()
        || (pane.worktree.is_some() && pane.setup.as_deref().is_some_and(|value| !value.is_empty()))
        || pane
            .command
            .as_deref()
            .is_some_and(|value| !value.is_empty())
        || pane
            .prompt
            .as_deref()
            .is_some_and(|value| !value.is_empty())
        || pane.env.as_ref().is_some_and(|env| !env.is_empty())
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

pub(super) use paneflow_git::worktree_plan::Plan as WorktreePlan;

pub(super) fn plan_worktree(idx: usize, pane: &PaneSpec) -> Result<Option<WorktreePlan>, CliError> {
    let Some(request) = worktree_request(pane) else {
        return Ok(None);
    };
    paneflow_git::worktree_plan::plan(idx, &request)
        .map(Some)
        .map_err(CliError::runtime)
}

pub(super) fn worktree_request(pane: &PaneSpec) -> Option<paneflow_git::worktree_plan::Request> {
    Some(paneflow_git::worktree_plan::Request {
        branch: pane.worktree.clone()?,
        cwd: pane.cwd.clone(),
        base: pane.from.clone(),
        copy_env: pane.copy_env,
        setup: pane.setup.clone(),
        setup_timeout_secs: pane.setup_timeout_secs,
        teardown: pane.worktree_teardown.clone(),
    })
}

pub(super) fn managed_json(plan: &WorktreePlan) -> Value {
    json!({
        "path": plan.path.to_string_lossy(),
        "repo_root": plan.repo_root.to_string_lossy(),
        "branch": plan.branch,
        "teardown": plan.teardown,
        "action": if plan.creates_worktree() { "create" } else { "reuse" },
    })
}

pub(super) fn check_worktree_conflicts(plans: &mut [Option<WorktreePlan>]) -> Result<(), CliError> {
    paneflow_git::worktree_plan::resolve_conflicts(plans).map_err(CliError::runtime)
}

pub(super) fn execute_worktree_plan(plan: &WorktreePlan) -> Result<(), CliError> {
    let notices = paneflow_git::worktree_plan::execute(plan).map_err(CliError::runtime)?;
    for notice in notices {
        eprintln!("{notice}");
    }
    Ok(())
}

pub(super) fn rollback_created_worktrees(plans: &[&WorktreePlan]) {
    for notice in paneflow_git::worktree_plan::rollback(plans) {
        eprintln!("{notice}");
    }
}

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

pub(super) fn allocate_port_offsets(
    refs: &[bool],
    port_base: u16,
    is_free: impl Fn(u16) -> bool,
) -> Result<Vec<Option<u16>>, CliError> {
    let mut next_stride: u16 = 0;
    let mut offsets = Vec::with_capacity(refs.len());
    for wants in refs {
        offsets.push(if !*wants {
            None
        } else {
            let candidate = loop {
                let offset = next_stride
                    .checked_mul(PORT_STRIDE)
                    .ok_or_else(port_exhausted)?;
                let candidate = port_base.checked_add(offset).ok_or_else(port_exhausted)?;
                if candidate > u16::MAX - (PORT_STRIDE - 1) {
                    return Err(port_exhausted());
                }
                next_stride = next_stride.checked_add(1).ok_or_else(port_exhausted)?;
                if port_stride_is_free(candidate, &is_free) {
                    break candidate;
                }
            };
            Some(candidate)
        });
    }
    Ok(offsets)
}

fn port_exhausted() -> CliError {
    CliError::runtime(format!(
        "no free {PORT_STRIDE}-port stride is available at or above the configured port_base"
    ))
}

fn port_stride_is_free(port_base: u16, is_free: &impl Fn(u16) -> bool) -> bool {
    (0..PORT_STRIDE).all(|offset| port_base.checked_add(offset).is_some_and(is_free))
}

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
    if !resolved.is_installed_now() {
        return Err(CliError::runtime(format!(
            "pane {idx}: agent '{agent}' ({}) not found on PATH",
            resolved.binary()
        )));
    }
    Ok(Some(resolved.launch_command(config)))
}

pub(super) fn sandbox_env(
    idx: usize,
    pane: &PaneSpec,
    config: &PaneFlowConfig,
    env: Option<HashMap<String, String>>,
) -> Result<Option<HashMap<String, String>>, CliError> {
    let Some(agent) = pane.agent.as_deref().and_then(resolve_agent) else {
        return Ok(env);
    };
    crate::agent_launcher::AgentLaunch::Builtin(agent)
        .with_sandbox_request(config, env)
        .map_err(|reason| CliError::runtime(format!("pane {idx}: {reason}")))
}

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
            from: None,
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
    fn navigation_only_panes_do_not_need_orchestration_gate() {
        let mut navigation = pane(None, None);
        navigation.cwd = Some("/tmp".to_string());
        navigation.focus = Some(true);
        assert!(!panes_need_orchestration(&[navigation]));
        assert!(panes_need_orchestration(&[pane(None, Some("true"))]));

        let mut with_prompt = pane(None, None);
        with_prompt.prompt = Some("prefill".to_string());
        assert!(panes_need_orchestration(&[with_prompt]));
    }

    #[test]
    fn allocate_port_offsets_skips_busy_strides() {
        let refs = vec![true, false, true];
        let offsets = allocate_port_offsets(&refs, 3000, |p| p != 3010).expect("offsets");
        assert_eq!(offsets, vec![Some(3000), None, Some(3020)]);
    }

    #[test]
    fn allocate_port_offsets_checks_the_whole_stride() {
        let refs = vec![true, true];
        let offsets = allocate_port_offsets(&refs, 3000, |p| p != 3005).expect("offsets");
        assert_eq!(offsets, vec![Some(3010), Some(3020)]);
    }

    #[test]
    fn allocate_port_offsets_errors_when_stride_cannot_fit() {
        let refs = vec![true];
        let err = allocate_port_offsets(&refs, u16::MAX, |_| true).unwrap_err();
        assert!(
            err.message.contains("no free") && err.message.contains("port_base"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn substitute_env_replaces_only_the_supported_token() {
        let mut env = HashMap::new();
        env.insert("PORT".to_string(), "${port_offset}".to_string());
        env.insert("PLAIN".to_string(), "untouched".to_string());
        let out = substitute_env(Some(&env), Some(3010)).expect("some");
        assert_eq!(out["PORT"], "3010");
        assert_eq!(out["PLAIN"], "untouched", "passthrough exact (AC4)");
        let out = substitute_env(Some(&env), None).expect("some");
        assert_eq!(out["PORT"], "${port_offset}");
    }

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

    #[test]
    fn resolve_command_rejects_unknown_agent() {
        let cfg = PaneFlowConfig::default();
        let err = resolve_command(2, &pane(Some("cloode"), None), &cfg).unwrap_err();
        assert_eq!(err.code, crate::cli::EXIT_RUNTIME);
        assert!(err.message.contains("pane 2"), "got: {}", err.message);
        assert!(err.message.contains("cloode"), "got: {}", err.message);
    }

    #[test]
    fn resolve_command_passes_raw_command_through() {
        let cfg = PaneFlowConfig::default();
        let resolved = resolve_command(0, &pane(None, Some("cargo watch")), &cfg).expect("ok");
        assert_eq!(resolved.as_deref(), Some("cargo watch"));
    }

    struct RecordingTransport(std::cell::RefCell<Vec<(String, Value)>>);
    impl IpcTransport for RecordingTransport {
        fn call(&self, method: &str, params: Value) -> Result<Value, String> {
            self.0.borrow_mut().push((method.to_string(), params));
            Ok(json!({ "index": 1, "surface_ids": [7] }))
        }
    }

    #[test]
    fn up_sends_the_spec_space_to_workspace_up() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _root = worktree::test_support::scoped_root(dir.path().join("worktrees"));
        let file = dir.path().join("paneflow.workspace.toml");
        let cwd = dir.path().to_string_lossy().replace('\\', "/");
        std::fs::write(
            &file,
            format!("space = \"default\"\n[[panes]]\ncwd = \"{cwd}\"\n"),
        )
        .expect("write spec");
        let fake = RecordingTransport(std::cell::RefCell::new(Vec::new()));

        assert_eq!(
            up(&fake, file.to_str().expect("utf-8 path"), false).expect("ok"),
            EXIT_OK
        );

        let calls = fake.0.borrow();
        let (method, params) = calls.last().expect("workspace.up was called");
        assert_eq!(method, "workspace.up");
        assert_eq!(params["space"], "default");
    }

    #[test]
    fn up_refuses_an_unknown_space_before_creating_any_worktree() {
        let mut config = PaneFlowConfig::default();
        config.spaces.push(paneflow_config::schema::SpaceConfig {
            id: paneflow_config::schema::SpaceId::parse("550e8400-e29b-41d4-a716-446655440001")
                .expect("valid id"),
            name: "Work".to_string(),
            color: None,
            avatar: None,
            env: Default::default(),
        });
        assert!(check_space(&config, Some("WORK")).is_ok());
        assert!(check_space(&config, None).is_ok());
        let err = check_space(&config, Some("Client")).expect_err("no space is named Client");
        assert_eq!(err.code, crate::cli::EXIT_RUNTIME);
        assert_eq!(err.message, "unknown space: Client");

        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("paneflow.workspace.toml");
        let cwd = dir.path().to_string_lossy().replace('\\', "/");
        std::fs::write(
            &file,
            format!("space = \"no-such-space-7f3a9c\"\n[[panes]]\ncwd = \"{cwd}\"\n"),
        )
        .expect("write spec");
        let fake = RecordingTransport(std::cell::RefCell::new(Vec::new()));
        let err = up(&fake, file.to_str().expect("utf-8 path"), false).expect_err("refused");
        assert_eq!(err.message, "unknown space: no-such-space-7f3a9c");
        assert!(fake.0.borrow().is_empty());
    }
}
