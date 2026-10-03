use std::path::Path;

use paneflow_agent_config::runtime_catalog::{RuntimeLifecycleAuthority, runtime_for_tool};
use paneflow_config::schema::{SessionGeneration, SessionId, WorkspaceId};
use paneflow_ipc_client::scrollback::{
    fit_matches_to_ipc_frame, neutralize_untrusted, paginate_scrollback, search_text,
    truncate_ipc_text, wrap_untrusted,
};
use paneflow_ipc_client::send_text::{
    SUBMIT_ECHO_EXTRA, SUBMIT_ECHO_POLL, SubmitTick, bracketed_paste_frame, resolve_paste_mode,
    resolve_send_text_body_mode, submit_echo_tick,
};
use serde_json::{Value, json};

use crate::host::{HostError, SessionHost, SessionSummary};
use crate::manifest::HookRecord;

pub const DEFAULT_READ_LINES: usize = 200;
pub const MAX_READ_LINES: usize = 4000;
pub const DEFAULT_SEARCH_MATCHES: usize = 50;
pub const MAX_SEARCH_MATCHES: usize = 1000;
pub const MAX_SEND_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_SEARCH_PATTERN_BYTES: usize = 512;

pub const CONTROL_METHODS: &[&str] = &[
    "system.capabilities",
    "surface.list",
    "surface.read",
    "surface.search",
    "surface.status",
    "surface.send_text",
    "pane.write",
    "fleet.list",
    "agent.capture",
    "agent.explain",
];

pub const CONTROLLER_ONLY_METHODS: &[&str] = &[
    "surface.focus",
    "surface.split",
    "surface.rename",
    "surface.send_keystroke",
    "workspace.list",
    "workspace.current",
    "workspace.create",
    "workspace.select",
    "workspace.close",
    "workspace.up",
    "workspace.restore_layout",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlError {
    Params(String),
    Disabled(String),
    NoController(&'static str),
    Host(HostError),
}

impl From<HostError> for ControlError {
    fn from(error: HostError) -> Self {
        Self::Host(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlPermissions {
    pub scripting: bool,
    pub orchestration: bool,
    pub fenced_reads: bool,
}

impl ControlPermissions {
    pub fn from_environment(unrestricted: bool, fenced_reads: bool) -> Self {
        let scripting = std::env::var("PANEFLOW_IPC_SCRIPTING").as_deref() == Ok("1");
        let orchestration =
            scripting || std::env::var("PANEFLOW_IPC_ORCHESTRATION").as_deref() == Ok("1");
        Self {
            scripting: scripting || unrestricted,
            orchestration: orchestration || unrestricted,
            fenced_reads,
        }
    }
}

#[derive(Debug, Default)]
pub struct ConnectionAliases {
    aliases: Vec<(u64, SessionId)>,
}

impl ConnectionAliases {
    pub fn refresh(&mut self, sessions: &[SessionSummary]) {
        self.aliases = sessions
            .iter()
            .enumerate()
            .map(|(index, summary)| (index as u64 + 1, summary.manifest.session.clone()))
            .collect();
    }

    pub fn resolve(&self, alias: u64) -> Option<&SessionId> {
        self.aliases
            .iter()
            .find(|(held, _)| *held == alias)
            .map(|(_, session)| session)
    }

    pub fn alias_of(&self, session: &SessionId) -> Option<u64> {
        self.aliases
            .iter()
            .find(|(_, held)| held == session)
            .map(|(alias, _)| *alias)
    }
}

pub fn surface_name(summary: &SessionSummary) -> String {
    if let Some(title) = summary
        .manifest
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
    {
        return title.to_string();
    }
    let cwd = summary
        .manifest
        .current_cwd
        .as_deref()
        .unwrap_or(summary.manifest.cwd.as_str());
    Path::new(cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| summary.manifest.session.to_string())
}

fn surface_value(alias: u64, summary: &SessionSummary) -> Value {
    json!({
        "surface_id": alias,
        "session": summary.manifest.session,
        "generation": summary.manifest.generation,
        "name": surface_name(summary),
        "title": summary.manifest.title,
        "cwd": summary.manifest.current_cwd.clone().or_else(|| Some(summary.manifest.cwd.clone())),
        "cmd": summary.manifest.launch.shell,
        "workspace_id": Value::Null,
        "workspace": summary.manifest.workspace,
        "scope": "host",
        "live": summary.live,
        "lifecycle": summary.manifest.lifecycle.label(),
        "tab_id": Value::Null,
        "tab_title": Value::Null,
    })
}

fn current_hook(
    generation: SessionGeneration,
    last_hook: Option<&HookRecord>,
) -> Option<&HookRecord> {
    last_hook.filter(|hook| hook.runtime_generation == generation)
}

fn hook_is_authoritative(hook: &HookRecord) -> bool {
    runtime_for_tool(&hook.tool)
        .is_some_and(|runtime| runtime.lifecycle.authority == RuntimeLifecycleAuthority::Complete)
}

fn agent_hook_fields(
    generation: SessionGeneration,
    last_hook: Option<&HookRecord>,
    now_ms: u64,
) -> serde_json::Map<String, Value> {
    let hook = current_hook(generation, last_hook);
    let mut fields = serde_json::Map::new();
    fields.insert(
        "hooked".to_string(),
        Value::Bool(hook.is_some_and(hook_is_authoritative)),
    );
    if let Some(hook) = hook {
        fields.insert("tool".to_string(), json!(hook.tool));
        fields.insert("pid".to_string(), json!(hook.pid));
        fields.insert("hook_event_name".to_string(), json!(hook.hook_event_name));
        fields.insert("active_tool_name".to_string(), json!(hook.tool_name));
        fields.insert(
            "runtime_generation".to_string(),
            json!(hook.runtime_generation),
        );
        fields.insert(
            "idle_ms".to_string(),
            json!(now_ms.saturating_sub(hook.received_at_ms)),
        );
    }
    fields
}

pub(crate) fn agent_status_value(
    alias: u64,
    session: &SessionId,
    generation: SessionGeneration,
    last_hook: Option<&HookRecord>,
    output_generation: Option<u64>,
    now_ms: u64,
) -> Value {
    let mut value = agent_hook_fields(generation, last_hook, now_ms);
    value.insert("surface_id".to_string(), json!(alias));
    value.insert("session".to_string(), json!(session));
    value.insert("generation".to_string(), json!(generation));
    if let Some(output_generation) = output_generation {
        value.insert("output_generation".to_string(), json!(output_generation));
    }
    Value::Object(value)
}

pub(crate) fn foreground_runtime(summary: &SessionSummary) -> Value {
    json!(
        summary
            .manifest
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.current_observation.as_ref())
            .map(|observation| observation.id.as_str())
    )
}

pub(crate) fn output_generation(host: &SessionHost, session: &SessionId) -> Option<u64> {
    match host.output_stream(session, None) {
        Ok(Ok(stream)) => Some(stream.end_offset()),
        Ok(Err(ended)) => Some(ended.end_offset),
        Err(_) => None,
    }
}

pub const SCOPE_ALL: &str = "all";

const SCOPE_WORKSPACE: &str = "workspace";

fn string_is_nonempty(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|text| !text.is_empty())
}

pub fn create_requires_orchestration(params: &Value) -> bool {
    string_is_nonempty(params.get("command"))
        || string_is_nonempty(params.get("prompt"))
        || string_is_nonempty(params.get("shell"))
        || params
            .get("args")
            .and_then(Value::as_array)
            .is_some_and(|args| !args.is_empty())
        || params
            .get("env")
            .and_then(Value::as_object)
            .is_some_and(|env| env.values().any(Value::is_string))
}

fn create_gate(params: &Value, permissions: ControlPermissions) -> Result<(), ControlError> {
    if create_requires_orchestration(params) {
        if permissions.orchestration {
            return Ok(());
        }
        return Err(ControlError::Disabled(
            "session.create orchestration disabled; set PANEFLOW_IPC_ORCHESTRATION=1 or \
             PANEFLOW_IPC_SCRIPTING=1 to launch a command, prompt, or env"
                .to_string(),
        ));
    }
    if permissions.scripting {
        return Ok(());
    }
    Err(ControlError::Disabled(
        "session.create disabled; set PANEFLOW_IPC_SCRIPTING=1 to enable".to_string(),
    ))
}

fn input_gate(permissions: ControlPermissions) -> Result<(), ControlError> {
    if permissions.scripting {
        return Ok(());
    }
    Err(ControlError::Disabled(
        "session.input disabled; set PANEFLOW_IPC_SCRIPTING=1 to enable".to_string(),
    ))
}

pub fn authorize_session_write(
    host: &SessionHost,
    attaches: bool,
    permissions: ControlPermissions,
    method: &str,
    params: &Value,
) -> Result<(), ControlError> {
    match method {
        "session.input" => {
            if !attaches {
                input_gate(permissions)?;
            }
            match params
                .get("session")
                .and_then(Value::as_str)
                .map(SessionId::parse)
            {
                Some(Ok(target)) => authorize_write_scope(host, params, permissions, &target),
                _ => Ok(()),
            }
        }
        "session.create" if !attaches => create_gate(params, permissions),
        _ => Ok(()),
    }
}

pub fn control_input_audit(client: &str, session: &SessionId, bytes: u64) -> String {
    format!("control client {client} wrote {bytes} bytes to session {session}")
}

fn caller_scope(host: &SessionHost, params: &Value) -> Result<Option<ReadScope>, ControlError> {
    read_scope(host, params).map_err(|_| {
        let caller = params
            .get("scope_session")
            .map(|raw| raw.as_str().map_or_else(|| raw.to_string(), str::to_string))
            .unwrap_or_default();
        ControlError::Params(format!(
            "unknown caller session {caller}: this instance does not host it, so the write is refused"
        ))
    })
}

pub fn authorize_write_scope(
    host: &SessionHost,
    params: &Value,
    permissions: ControlPermissions,
    target: &SessionId,
) -> Result<(), ControlError> {
    match params.get("scope").map(Value::as_str) {
        None | Some(Some(SCOPE_WORKSPACE)) => {}
        Some(Some(SCOPE_ALL)) => {
            if permissions.orchestration {
                return Ok(());
            }
            return Err(ControlError::Disabled(
                "scope all needs orchestration; set PANEFLOW_IPC_ORCHESTRATION=1 or \
                 PANEFLOW_IPC_SCRIPTING=1 to write across workspaces"
                    .to_string(),
            ));
        }
        Some(_) => {
            return Err(ControlError::Params(
                "'scope' must be \"workspace\" or \"all\"".to_string(),
            ));
        }
    }
    let Some(scope) = caller_scope(host, params)? else {
        return Ok(());
    };
    let workspace = host.inspect(target)?.manifest.workspace;
    if scope.admits(target, &workspace) {
        return Ok(());
    }
    let place = workspace.map_or_else(
        || "no workspace".to_string(),
        |workspace| format!("workspace {workspace}"),
    );
    Err(ControlError::Params(format!(
        "session {target} belongs to {place}, outside the workspace of the calling pane; \
         rerun with --scope all (needs PANEFLOW_IPC_ORCHESTRATION=1) to write across workspaces"
    )))
}

fn submit_after_echo(
    host: &SessionHost,
    session: &SessionId,
    generation: Option<SessionGeneration>,
    before: u64,
) -> Result<(), ControlError> {
    let floor = host.submit_paste_delay();
    let cap = floor + SUBMIT_ECHO_EXTRA;
    std::thread::sleep(floor);
    let mut waited = floor;
    while submit_echo_tick(before, output_generation(host, session), waited, cap)
        == SubmitTick::Wait
    {
        std::thread::sleep(SUBMIT_ECHO_POLL);
        waited += SUBMIT_ECHO_POLL;
    }
    host.input(session, generation, b"\r".to_vec())?;
    Ok(())
}

pub(crate) struct Delivered {
    pub paste: bool,
    pub submit_mode: Value,
    pub terminal_bracketed_paste: bool,
}

pub(crate) fn deliver_text(
    host: &SessionHost,
    session: &SessionId,
    generation: Option<SessionGeneration>,
    text: &str,
    paste_param: Option<bool>,
    submit: bool,
    agent_target: bool,
) -> Result<Delivered, ControlError> {
    let before = output_generation(host, session).unwrap_or_default();
    let terminal_bracketed_paste = host.bracketed_paste_enabled(session)?;
    let paste = resolve_paste_mode(paste_param, submit, agent_target, terminal_bracketed_paste);
    let paste = resolve_send_text_body_mode(text, paste_param, paste, terminal_bracketed_paste)
        .map_err(|message| ControlError::Params(message.to_string()))?;
    if !text.is_empty() {
        let body = if paste && terminal_bracketed_paste {
            bracketed_paste_frame(text)
        } else {
            text.to_string()
        };
        host.input(session, generation, body.into_bytes())?;
    }
    let submit_mode = match (submit, paste && !text.is_empty()) {
        (false, _) => Value::Null,
        (true, true) => {
            submit_after_echo(host, session, generation, before)?;
            json!("deferred_paste_cr")
        }
        (true, false) => {
            host.input(session, generation, b"\r".to_vec())?;
            json!("inline_cr")
        }
    };
    Ok(Delivered {
        paste,
        submit_mode,
        terminal_bracketed_paste,
    })
}

pub fn send_text_gate(permissions: ControlPermissions) -> Result<(), ControlError> {
    if permissions.scripting {
        return Ok(());
    }
    Err(ControlError::Params(
        "surface.send_text disabled; set PANEFLOW_IPC_SCRIPTING=1 to enable".to_string(),
    ))
}

fn param_usize(params: &Value, key: &str) -> Option<usize> {
    params
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

pub fn resolve_session(
    aliases: &ConnectionAliases,
    params: &Value,
) -> Result<SessionId, ControlError> {
    if let Some(raw) = params.get("session").and_then(Value::as_str) {
        return SessionId::parse(raw).map_err(|error| ControlError::Params(error.to_string()));
    }
    let Some(alias) = params.get("surface_id") else {
        return Err(ControlError::Params(
            "name a session or a surface_id from surface.list".to_string(),
        ));
    };
    let alias = alias.as_u64().ok_or_else(|| {
        ControlError::Params("surface_id must be an unsigned integer alias".to_string())
    })?;
    aliases.resolve(alias).cloned().ok_or_else(|| {
        ControlError::Params(format!(
            "surface_id {alias} is a connection-local alias; call surface.list on this connection first"
        ))
    })
}

struct ReadScope {
    session: SessionId,
    workspace: Option<WorkspaceId>,
}

impl ReadScope {
    fn admits(&self, session: &SessionId, workspace: &Option<WorkspaceId>) -> bool {
        match &self.workspace {
            Some(scope) => workspace.as_ref() == Some(scope),
            None => *session == self.session,
        }
    }
}

fn read_scope(host: &SessionHost, params: &Value) -> Result<Option<ReadScope>, ControlError> {
    let Some(raw) = params.get("scope_session") else {
        return Ok(None);
    };
    let raw = raw
        .as_str()
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
        .ok_or_else(|| ControlError::Params("'scope_session' must be a session id".to_string()))?;
    let session = SessionId::parse(raw).map_err(|error| ControlError::Params(error.to_string()))?;
    host.list(None)
        .into_iter()
        .find(|summary| summary.manifest.session == session)
        .map(|summary| {
            Some(ReadScope {
                session: summary.manifest.session,
                workspace: summary.manifest.workspace,
            })
        })
        .ok_or_else(|| {
            ControlError::Params(format!("scope session {raw} is not hosted by this host"))
        })
}

fn authorize_scoped_session(
    host: &SessionHost,
    scope: Option<&ReadScope>,
    session: &SessionId,
) -> Result<(), ControlError> {
    let Some(scope) = scope else {
        return Ok(());
    };
    if scope.admits(session, &host.inspect(session)?.manifest.workspace) {
        Ok(())
    } else {
        Err(ControlError::Params(format!(
            "session {session} is outside the workspace of the scope session"
        )))
    }
}

pub fn dispatch(
    host: &SessionHost,
    aliases: &mut ConnectionAliases,
    permissions: ControlPermissions,
    method: &str,
    params: &Value,
    now_ms: u64,
) -> Option<Result<Value, ControlError>> {
    if CONTROLLER_ONLY_METHODS.contains(&method) {
        return Some(Err(ControlError::NoController(
            "no Paneflow window is attached to this state home; this action needs an open controller",
        )));
    }
    if !CONTROL_METHODS.contains(&method) {
        return None;
    }
    Some(answer(host, aliases, permissions, method, params, now_ms))
}

fn answer(
    host: &SessionHost,
    aliases: &mut ConnectionAliases,
    permissions: ControlPermissions,
    method: &str,
    params: &Value,
    now_ms: u64,
) -> Result<Value, ControlError> {
    match method {
        "system.capabilities" => Ok(json!({
            "scripting": permissions.scripting,
            "orchestration": permissions.orchestration,
            "controller": false,
            "host": host.identity(),
        })),
        "surface.list" => {
            let scope = read_scope(host, params)?;
            let mut sessions: Vec<SessionSummary> = host
                .list(None)
                .into_iter()
                .filter(|summary| {
                    scope.as_ref().is_none_or(|scope| {
                        scope.admits(&summary.manifest.session, &summary.manifest.workspace)
                    })
                })
                .collect();
            sessions.sort_by(|a, b| a.manifest.session.cmp(&b.manifest.session));
            aliases.refresh(&sessions);
            let surfaces: Vec<Value> = sessions
                .iter()
                .enumerate()
                .map(|(index, summary)| surface_value(index as u64 + 1, summary))
                .collect();
            let mut result = json!({
                "pane_count": surfaces.len(),
                "workspace": Value::Null,
                "surfaces": surfaces,
            });
            if let Some(scope) = scope {
                result["scope_workspace"] = json!(scope.workspace);
            }
            Ok(result)
        }
        "surface.read" => {
            let scope = read_scope(host, params)?;
            let session = resolve_session(aliases, params)?;
            authorize_scoped_session(host, scope.as_ref(), &session)?;
            let lines = param_usize(params, "lines")
                .map(|lines| lines.clamp(1, MAX_READ_LINES))
                .unwrap_or(DEFAULT_READ_LINES);
            let offset = param_usize(params, "offset").unwrap_or(0);
            let full = host.text(&session)?.text;
            let (text, returned, total, eof) = paginate_scrollback(&full, lines, offset);
            if offset > total {
                return Err(ControlError::Params(format!(
                    "offset {offset} out of range (total_lines={total})"
                )));
            }
            let fenced = params
                .get("fenced")
                .and_then(Value::as_bool)
                .unwrap_or(permissions.fenced_reads);
            let text = if fenced {
                neutralize_untrusted(&text)
            } else {
                text
            };
            let (text, truncated) = truncate_ipc_text(text);
            let alias = aliases.alias_of(&session).unwrap_or(0);
            let text = if fenced {
                wrap_untrusted(
                    &format!("source=\"session:{session}\" total_lines=\"{total}\" eof=\"{eof}\""),
                    &text,
                )
            } else {
                text
            };
            Ok(json!({
                "surface_id": alias,
                "session": session,
                "text": text,
                "lines": returned,
                "total_lines": total,
                "eof": eof,
                "output_generation": 0,
                "truncated": truncated,
            }))
        }
        "surface.search" => {
            let pattern = params
                .get("pattern")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if pattern.is_empty() {
                return Err(ControlError::Params(
                    "missing or empty 'pattern' parameter".to_string(),
                ));
            }
            if pattern.len() > MAX_SEARCH_PATTERN_BYTES {
                return Err(ControlError::Params(format!(
                    "pattern exceeds {MAX_SEARCH_PATTERN_BYTES} bytes"
                )));
            }
            let scope = read_scope(host, params)?;
            let session = resolve_session(aliases, params)?;
            authorize_scoped_session(host, scope.as_ref(), &session)?;
            let max_matches = param_usize(params, "max_matches")
                .map(|max| max.clamp(1, MAX_SEARCH_MATCHES))
                .unwrap_or(DEFAULT_SEARCH_MATCHES);
            let full = host.text(&session)?.text;
            let (matches, capped) = search_text(&full, pattern, max_matches);
            let (matches, clipped) = fit_matches_to_ipc_frame(matches);
            let truncated = capped || clipped;
            let matches: Vec<Value> = matches
                .into_iter()
                .map(|(line, text)| json!({"line": line, "text": text}))
                .collect();
            Ok(json!({"matches": matches, "truncated": truncated}))
        }
        "surface.status" => {
            let session = resolve_session(aliases, params)?;
            let summary = host.inspect(&session)?;
            let alias = aliases.alias_of(&session).unwrap_or(0);
            let mut status = agent_status_value(
                alias,
                &session,
                summary.manifest.generation,
                summary.manifest.last_hook.as_ref(),
                output_generation(host, &session),
                now_ms,
            );
            status["foreground_runtime"] = foreground_runtime(&summary);
            Ok(status)
        }
        "agent.capture" => {
            let scope = read_scope(host, params)?;
            let session = resolve_session(aliases, params)?;
            authorize_scoped_session(host, scope.as_ref(), &session)?;
            let capture = crate::viewport_scan::capture(host, &session)?;
            Ok(json!({
                "session": session,
                "screen": capture.scan.screen,
                "cols": capture.scan.cols,
                "rows": capture.scan.rows,
                "title": capture.scan.title,
                "progress": capture.scan.progress,
                "runtime_id": capture.runtime.map(|runtime| runtime.id),
            }))
        }
        "agent.explain" => {
            let scope = read_scope(host, params)?;
            let session = resolve_session(aliases, params)?;
            authorize_scoped_session(host, scope.as_ref(), &session)?;
            Ok(crate::viewport_scan::explain(host, &session, now_ms)?)
        }
        "fleet.list" => {
            let mut sessions = host.list(None);
            sessions.sort_by(|a, b| a.manifest.session.cmp(&b.manifest.session));
            aliases.refresh(&sessions);
            let agents: Vec<Value> = sessions
                .iter()
                .enumerate()
                .filter_map(|(index, summary)| {
                    current_hook(
                        summary.manifest.generation,
                        summary.manifest.last_hook.as_ref(),
                    )?;
                    let mut agent = agent_hook_fields(
                        summary.manifest.generation,
                        summary.manifest.last_hook.as_ref(),
                        now_ms,
                    );
                    agent.insert("surface_id".to_string(), json!(index as u64 + 1));
                    agent.insert("surface_name".to_string(), json!(surface_name(summary)));
                    agent.insert("session".to_string(), json!(summary.manifest.session));
                    agent.insert("generation".to_string(), json!(summary.manifest.generation));
                    agent.insert("workspace".to_string(), json!(summary.manifest.workspace));
                    Some(Value::Object(agent))
                })
                .collect();
            Ok(json!({"agents": agents}))
        }
        "pane.write" => crate::agent_write::pane_write(host, permissions, params, now_ms),
        "surface.send_text" => {
            send_text_gate(permissions)?;
            let text = params.get("text").and_then(Value::as_str).unwrap_or("");
            let submit = params
                .get("submit")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let paste_param = params.get("paste").and_then(Value::as_bool);
            if text.is_empty() && !submit {
                return Err(ControlError::Params("Missing 'text' parameter".to_string()));
            }
            if text.len() > MAX_SEND_TEXT_BYTES {
                return Err(ControlError::Params(
                    "Text exceeds 64 KiB limit".to_string(),
                ));
            }
            let session = resolve_session(aliases, params)?;
            authorize_write_scope(host, params, permissions, &session)?;
            let summary = host.inspect(&session)?;
            let generation = Some(summary.manifest.generation);
            let agent_target = summary.manifest.last_hook.is_some();
            let forced = params
                .get("force")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let delivered = deliver_text(
                host,
                &session,
                generation,
                text,
                paste_param,
                submit,
                agent_target,
            )?;
            log::info!(
                "paneflow-host: surface.send_text wrote {} bytes to session {session}{}",
                text.len(),
                if forced {
                    " (forced past the agent delivery checks)"
                } else {
                    ""
                }
            );
            Ok(json!({
                "sent": true,
                "length": text.len(),
                "submitted": submit,
                "paste": delivered.paste,
                "submit_mode": delivered.submit_mode,
                "agent_target": agent_target,
                "agent_tool": summary.manifest.last_hook.as_ref().map(|hook| hook.tool.clone()),
                "terminal_bracketed_paste": delivered.terminal_bracketed_paste,
            }))
        }
        _ => Err(ControlError::Params(format!(
            "method not handled by the local host: {method}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{SessionLaunch, SessionLifecycle, SessionManifest};
    use paneflow_config::schema::{HostInstanceToken, SessionGeneration, WorkspaceId};

    fn summary(session: SessionId, title: Option<&str>, cwd: &str) -> SessionSummary {
        SessionSummary {
            manifest: SessionManifest {
                schema: crate::manifest::MANIFEST_SCHEMA_VERSION,
                session,
                workspace: Some(WorkspaceId::new()),
                generation: SessionGeneration::FIRST,
                host_instance: HostInstanceToken::new(),
                cwd: cwd.to_string(),
                launch: SessionLaunch {
                    shell: "/bin/sh".to_string(),
                    args: Vec::new(),
                    env: Default::default(),
                    cols: 80,
                    rows: 24,
                },
                lifecycle: SessionLifecycle::Running,
                process: None,
                title: title.map(str::to_string),
                current_cwd: None,
                last_hook: None,
                hook_revision: 0,
                generation_started_at_ms: None,
                screen_changed_at_ms: None,
                screen_activity: None,
                menu_prompt_active: false,
                runtime: None,
                final_output: None,
                host_protocol_version: crate::protocol::HOST_PROTOCOL_VERSION,
                host_build_id: crate::protocol::host_build_id(),
                created_at_ms: 1,
                updated_at_ms: 1,
            },
            live: true,
            owned: true,
            pending_launch: false,
            launch_operation: None,
            descendants_unresolved: 0,
            durability_error: None,
        }
    }

    #[test]
    fn aliases_are_connection_local_and_never_guessed() {
        let first = SessionId::new();
        let second = SessionId::new();
        let mut aliases = ConnectionAliases::default();
        assert_eq!(
            resolve_session(&aliases, &json!({"surface_id": 1}))
                .unwrap_err(),
            ControlError::Params(
                "surface_id 1 is a connection-local alias; call surface.list on this connection first"
                    .to_string()
            )
        );

        let mut sessions = vec![
            summary(first.clone(), None, "/a"),
            summary(second.clone(), None, "/b"),
        ];
        sessions.sort_by(|a, b| a.manifest.session.cmp(&b.manifest.session));
        aliases.refresh(&sessions);
        let expected = sessions[0].manifest.session.clone();
        assert_eq!(
            resolve_session(&aliases, &json!({"surface_id": 1})).unwrap(),
            expected
        );
        assert_eq!(aliases.alias_of(&expected), Some(1));
        assert!(resolve_session(&aliases, &json!({"surface_id": 9})).is_err());

        assert_eq!(
            resolve_session(&aliases, &json!({"session": first.to_string()})).unwrap(),
            first,
            "a durable id never depends on the connection table"
        );
        assert!(resolve_session(&aliases, &json!({"session": "not-a-uuid"})).is_err());
        assert!(resolve_session(&aliases, &json!({})).is_err());
    }

    #[test]
    fn a_surface_name_prefers_the_title_then_the_directory() {
        let session = SessionId::new();
        assert_eq!(
            surface_name(&summary(session.clone(), Some("api"), "/a")),
            "api"
        );
        assert_eq!(
            surface_name(&summary(session.clone(), Some("   "), "/repo/web")),
            "web"
        );
        assert_eq!(
            surface_name(&summary(session.clone(), None, "/repo/web")),
            "web"
        );
    }

    #[test]
    fn controller_only_methods_answer_explicitly_instead_of_guessing() {
        let permissions = ControlPermissions {
            scripting: true,
            orchestration: true,
            fenced_reads: true,
        };
        for method in CONTROLLER_ONLY_METHODS {
            assert!(
                CONTROL_METHODS.iter().all(|held| held != method),
                "{method} cannot be both host-served and controller-only"
            );
            let _ = permissions;
        }
        assert!(CONTROLLER_ONLY_METHODS.contains(&"surface.focus"));
        assert!(CONTROLLER_ONLY_METHODS.contains(&"surface.send_keystroke"));
        assert!(CONTROL_METHODS.contains(&"surface.send_text"));
    }

    fn status_hook(tool: &str, generation: SessionGeneration) -> HookRecord {
        HookRecord {
            event: None,
            activity_event: None,
            hook_event_name: "UserPromptSubmit".to_string(),
            tool: tool.to_string(),
            tool_name: None,
            pid: Some(42),
            runtime_generation: generation,
            provider_session_id: None,
            transcript_path: None,
            emitted_at_ms: Some(900),
            received_at_ms: 1_000,
        }
    }

    #[test]
    fn the_status_row_reports_the_raw_hook_and_never_names_a_state() {
        let session = SessionId::new();
        let hook = status_hook("claude", SessionGeneration::FIRST);
        let value = agent_status_value(
            3,
            &session,
            SessionGeneration::FIRST,
            Some(&hook),
            Some(812),
            5_000,
        );
        assert!(
            value.get("state").is_none(),
            "the core never reduces; only a worker names a state"
        );
        assert_eq!(value["hooked"], true);
        assert_eq!(value["hook_event_name"], "UserPromptSubmit");
        assert_eq!(value["idle_ms"], 4_000);
        assert_eq!(value["output_generation"], 812);
        assert_eq!(value["session"], json!(session));

        let bare = agent_status_value(3, &session, SessionGeneration::FIRST, None, None, 5_000);
        assert_eq!(bare["hooked"], false);
        assert!(bare.get("state").is_none());
        assert!(
            bare.get("output_generation").is_none(),
            "an unknown counter is absent, never a constant"
        );
        assert!(!value.to_string().contains("unknown"));
        assert!(!bare.to_string().contains("unknown"));
    }

    #[test]
    fn the_status_names_the_runtime_the_host_sees_in_the_foreground_now() {
        let mut held = summary(SessionId::new(), None, "/a");
        assert_eq!(foreground_runtime(&held), Value::Null);
        held.manifest.runtime = Some(crate::manifest::HostedSessionRuntime {
            current_observation: Some(crate::runtime_observer::RuntimeObservation {
                id: "com.anthropic.claude-code".to_string(),
                pid: 42,
                pid_started_at: Some(7),
                process_group: 42,
                process_name: "claude".to_string(),
                argv: None,
            }),
            launch_binding: Some("com.anthropic.claude-code".to_string()),
        });
        assert_eq!(
            foreground_runtime(&held),
            json!("com.anthropic.claude-code")
        );
        held.manifest.runtime = Some(crate::manifest::HostedSessionRuntime {
            current_observation: None,
            launch_binding: Some("com.anthropic.claude-code".to_string()),
        });
        assert_eq!(
            foreground_runtime(&held),
            Value::Null,
            "a launch binding is not a foreground observation"
        );
    }

    #[test]
    fn a_relaunched_agent_is_not_hooked_by_the_previous_generation() {
        let session = SessionId::new();
        let previous = status_hook("claude", SessionGeneration::FIRST);
        let relaunched = agent_status_value(
            3,
            &session,
            SessionGeneration::FIRST.next(),
            Some(&previous),
            Some(10),
            5_000,
        );
        assert_eq!(relaunched["hooked"], false);
        assert!(relaunched.get("hook_event_name").is_none());
    }

    #[test]
    fn only_a_runtime_with_complete_authority_is_hooked() {
        let session = SessionId::new();
        for (tool, hooked) in [
            ("claude", true),
            ("codex", true),
            ("pi", false),
            ("gemini", false),
        ] {
            let hook = status_hook(tool, SessionGeneration::FIRST);
            let value = agent_status_value(
                1,
                &session,
                SessionGeneration::FIRST,
                Some(&hook),
                None,
                5_000,
            );
            assert_eq!(value["hooked"], hooked, "{tool}");
        }
    }

    #[test]
    fn permissions_default_closed_until_an_operator_opens_them() {
        let closed = ControlPermissions {
            scripting: false,
            orchestration: false,
            fenced_reads: true,
        };
        let error = send_text_gate(closed).unwrap_err();
        assert!(
            matches!(error, ControlError::Params(ref message) if message.contains("PANEFLOW_IPC_SCRIPTING"))
        );
        assert_eq!(
            send_text_gate(ControlPermissions {
                scripting: true,
                ..closed
            }),
            Ok(())
        );
        assert!(
            ControlPermissions::from_environment(true, true).scripting,
            "free access opens the gate without the env switch"
        );
    }
}
