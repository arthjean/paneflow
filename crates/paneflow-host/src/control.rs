use std::path::Path;

use paneflow_config::schema::SessionId;
use paneflow_ipc_client::scrollback::{
    paginate_scrollback, search_text, truncate_ipc_text, wrap_untrusted,
};
use paneflow_ipc_client::send_text::{
    bracketed_paste_frame, resolve_paste_mode, resolve_send_text_body_mode,
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
    "fleet.list",
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

    pub fn is_empty(&self) -> bool {
        self.aliases.is_empty()
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

fn agent_status_value(alias: u64, last_hook: Option<&HookRecord>, now_ms: u64) -> Value {
    match last_hook {
        Some(hook) => json!({
            "surface_id": alias,
            "state": "unknown",
            "hooked": true,
            "tool": hook.tool,
            "hook_event_name": hook.hook_event_name,
            "active_tool_name": hook.tool_name,
            "runtime_generation": hook.runtime_generation,
            "idle_ms": now_ms.saturating_sub(hook.received_at_ms),
            "output_generation": 0,
            "reduced_by": Value::Null,
        }),
        None => json!({
            "surface_id": alias,
            "state": "idle",
            "hooked": false,
            "output_generation": 0,
            "reduced_by": Value::Null,
        }),
    }
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
            let mut sessions = host.list(None);
            sessions.sort_by(|a, b| a.manifest.session.cmp(&b.manifest.session));
            aliases.refresh(&sessions);
            let surfaces: Vec<Value> = sessions
                .iter()
                .enumerate()
                .map(|(index, summary)| surface_value(index as u64 + 1, summary))
                .collect();
            Ok(json!({
                "pane_count": surfaces.len(),
                "workspace": Value::Null,
                "surfaces": surfaces,
            }))
        }
        "surface.read" => {
            let session = resolve_session(aliases, params)?;
            let lines = param_usize(params, "lines")
                .map(|lines| lines.clamp(1, MAX_READ_LINES))
                .unwrap_or(DEFAULT_READ_LINES);
            let offset = param_usize(params, "offset").unwrap_or(0);
            let full = host.text(&session)?;
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
            let session = resolve_session(aliases, params)?;
            let max_matches = param_usize(params, "max_matches")
                .map(|max| max.clamp(1, MAX_SEARCH_MATCHES))
                .unwrap_or(DEFAULT_SEARCH_MATCHES);
            let full = host.text(&session)?;
            let (matches, truncated) = search_text(&full, pattern, max_matches);
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
            Ok(agent_status_value(
                alias,
                summary.manifest.last_hook.as_ref(),
                now_ms,
            ))
        }
        "fleet.list" => {
            let mut sessions = host.list(None);
            sessions.sort_by(|a, b| a.manifest.session.cmp(&b.manifest.session));
            aliases.refresh(&sessions);
            let agents: Vec<Value> = sessions
                .iter()
                .enumerate()
                .filter_map(|(index, summary)| {
                    let hook = summary.manifest.last_hook.as_ref()?;
                    Some(json!({
                        "pid": hook.pid,
                        "tool": hook.tool,
                        "state": "unknown",
                        "hooked": true,
                        "reason": Value::Null,
                        "surface_id": index as u64 + 1,
                        "surface_name": surface_name(summary),
                        "session": summary.manifest.session,
                        "workspace": summary.manifest.workspace,
                        "hook_event_name": hook.hook_event_name,
                        "active_tool_name": hook.tool_name,
                        "runtime_generation": hook.runtime_generation,
                        "idle_ms": now_ms.saturating_sub(hook.received_at_ms),
                        "reduced_by": Value::Null,
                    }))
                })
                .collect();
            Ok(json!({"agents": agents}))
        }
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
            let summary = host.inspect(&session)?;
            let generation = Some(summary.manifest.generation);
            let agent_target = summary.manifest.last_hook.is_some();
            let terminal_bracketed_paste = host.bracketed_paste_enabled(&session)?;
            let paste =
                resolve_paste_mode(paste_param, submit, agent_target, terminal_bracketed_paste);
            let paste =
                resolve_send_text_body_mode(text, paste_param, paste, terminal_bracketed_paste)
                    .map_err(|message| ControlError::Params(message.to_string()))?;
            if !text.is_empty() {
                let body = if paste && terminal_bracketed_paste {
                    bracketed_paste_frame(text)
                } else {
                    text.to_string()
                };
                host.input(&session, generation, body.into_bytes())?;
            }
            let submit_mode = match (submit, paste && !text.is_empty()) {
                (false, _) => Value::Null,
                (true, true) => {
                    std::thread::sleep(host.submit_paste_delay());
                    host.input(&session, generation, b"\r".to_vec())?;
                    json!("deferred_paste_cr")
                }
                (true, false) => {
                    host.input(&session, generation, b"\r".to_vec())?;
                    json!("inline_cr")
                }
            };
            Ok(json!({
                "sent": true,
                "length": text.len(),
                "submitted": submit,
                "paste": paste,
                "submit_mode": submit_mode,
                "agent_target": agent_target,
                "agent_tool": summary.manifest.last_hook.as_ref().map(|hook| hook.tool.clone()),
                "terminal_bracketed_paste": terminal_bracketed_paste,
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
                generation_started_at_ms: None,
                screen_changed_at_ms: None,
                screen_activity: None,
                menu_prompt_active: false,
                runtime: None,
                host_protocol_version: crate::protocol::HOST_PROTOCOL_VERSION,
                host_build_id: crate::protocol::host_build_id(),
                created_at_ms: 1,
                updated_at_ms: 1,
            },
            live: true,
            owned: true,
        }
    }

    #[test]
    fn aliases_are_connection_local_and_never_guessed() {
        let first = SessionId::new();
        let second = SessionId::new();
        let mut aliases = ConnectionAliases::default();
        assert!(aliases.is_empty());
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

    #[test]
    fn the_status_row_reports_the_raw_hook_and_never_a_reduced_state() {
        let hook = HookRecord {
            hook_event_name: "UserPromptSubmit".to_string(),
            tool: "claude".to_string(),
            tool_name: None,
            pid: Some(42),
            runtime_generation: SessionGeneration::FIRST,
            provider_session_id: None,
            transcript_path: None,
            emitted_at_ms: Some(900),
            received_at_ms: 1_000,
        };
        let value = agent_status_value(3, Some(&hook), 5_000);
        assert_eq!(
            value["state"], "unknown",
            "the core never reduces; only a worker names a state"
        );
        assert_eq!(value["hooked"], true);
        assert_eq!(value["hook_event_name"], "UserPromptSubmit");
        assert_eq!(value["idle_ms"], 4_000);
        assert!(value["reduced_by"].is_null());

        let idle = agent_status_value(3, None, 5_000);
        assert_eq!(idle["state"], "idle");
        assert_eq!(idle["hooked"], false);
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
