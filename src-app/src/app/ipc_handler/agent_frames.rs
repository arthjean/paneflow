use super::*;

fn read_notification_message(params: &serde_json::Value) -> Option<String> {
    let hook = params.get("hook_payload");
    hook.and_then(|h| h.get("message"))
        .and_then(|v| v.as_str())
        .or_else(|| params.get("message").and_then(|v| v.as_str()))
        .map(sanitize_notification_message)
        .filter(|message| !message.trim().is_empty())
}

fn is_interrupt_lifecycle_event(params: &serde_json::Value) -> bool {
    LifecycleEventSource::from_wire_params(params) == Some(LifecycleEventSource::Interrupt)
}

pub(crate) fn frame_is_hook_sourced(params: &serde_json::Value) -> bool {
    match params.get("activity_source").and_then(|v| v.as_str()) {
        None => true,
        Some(source) => source == "hooks",
    }
}

#[allow(clippy::too_many_arguments)]
fn session_event_value(
    method: &str,
    workspace_id: Option<u64>,
    pid: Option<u32>,
    tool: Option<&str>,
    state: Option<&str>,
    surface_id: Option<u64>,
    message: Option<&str>,
    active_tool: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "type": method,
        "workspace_id": workspace_id,
        "pid": pid,
        "tool": tool,
        "state": state,
        "surface_id": surface_id,
        "message": message,
        "active_tool_name": active_tool,
        "ts": crate::ipc_events::now_ms(),
    })
}

fn resolved_event_surface_id(
    session_surface_id: Option<u64>,
    explicit_surface_id: Option<u64>,
) -> Option<u64> {
    session_surface_id.or(explicit_surface_id)
}

fn read_session_pid(params: &serde_json::Value) -> Option<u32> {
    SessionPid::from_wire_params(params).map(SessionPid::get)
}

fn read_frame_surface_id(params: &serde_json::Value) -> Option<u64> {
    SurfaceId::from_wire_params(params).map(SurfaceId::get)
}

enum GeneratedTitleSource {
    ClaudeTranscript(std::path::PathBuf),
}

impl GeneratedTitleSource {
    fn read(self) -> Option<String> {
        match self {
            Self::ClaudeTranscript(path) => crate::claude_sessions::read_generated_title(&path),
        }
    }
}

fn generated_title_source(
    tool: crate::agent_launcher::TerminalAgent,
    params: &serde_json::Value,
) -> Option<GeneratedTitleSource> {
    match tool {
        crate::agent_launcher::TerminalAgent::ClaudeCode => {
            read_transcript_path(params).map(GeneratedTitleSource::ClaudeTranscript)
        }
        _ => None,
    }
}

fn read_hook_prompt(params: &serde_json::Value) -> Option<&str> {
    params
        .get("hook_payload")?
        .get("prompt")
        .and_then(serde_json::Value::as_str)
}

fn read_hook_prompt_title(params: &serde_json::Value) -> Option<String> {
    crate::sidebar_title::tab_title_from_prompt(read_hook_prompt(params)?)
}

fn read_tool(params: &serde_json::Value) -> Option<crate::agent_launcher::TerminalAgent> {
    let tool_name = AiToolName::from_wire_params(params).ok()?;
    crate::agent_launcher::TerminalAgent::from_binary(tool_name.as_str())
}

fn session_end_fallback_candidate(
    sessions: &std::collections::HashMap<u32, AgentSession>,
    tool: Option<crate::agent_launcher::TerminalAgent>,
    explicit_surface_id: Option<u64>,
) -> Option<u32> {
    let mut candidates: Vec<u32> = sessions
        .iter()
        .filter(|(_, s)| s.state != ai_types::AgentState::Errored)
        .filter(|(_, s)| tool.is_none_or(|t| s.tool == t))
        .filter(|(_, s)| explicit_surface_id.is_none_or(|sid| s.surface_id == Some(sid)))
        .map(|(k, _)| *k)
        .collect();
    candidates.sort_unstable();
    match candidates.as_slice() {
        [single] => Some(*single),
        _ => None,
    }
}

const SYNTHETIC_SESSION_PID_BASE: u32 = 0xFFFF_0000;

pub(crate) fn bind_session_surface(
    sessions: &mut std::collections::HashMap<u32, AgentSession>,
    key: u32,
    sid: u64,
) -> bool {
    let Some(tool) = sessions.get(&key).map(|session| session.tool) else {
        return false;
    };
    let twins: Vec<u32> = sessions
        .iter()
        .filter(|(k, s)| **k != key && s.tool == tool && s.surface_id == Some(sid))
        .map(|(k, _)| *k)
        .collect();
    let bound = sessions
        .get(&key)
        .is_some_and(|session| session.surface_id == Some(sid));
    if bound && twins.is_empty() {
        return false;
    }
    let mut inherited_result = None;
    for twin in twins {
        if let Some(twin) = sessions.remove(&twin)
            && inherited_result.is_none()
        {
            inherited_result = twin.last_result;
        }
    }
    let session = sessions
        .get_mut(&key)
        .expect("the bound session was just read");
    session.surface_id = Some(sid);
    if session.last_result.is_none() {
        session.last_result = inherited_result;
    }
    true
}

pub(crate) fn upsert_session_state(
    sessions: &mut std::collections::HashMap<u32, AgentSession>,
    pid: Option<u32>,
    tool: crate::agent_launcher::TerminalAgent,
    transition: ai_types::SessionTransition,
    emitted_at_ms: Option<u64>,
    source: ai_types::AgentStateSource,
) -> Option<u32> {
    let key = match pid {
        Some(p) => p,
        None => {
            if let Some((existing_pid, _)) = sessions.iter().find(|(_, s)| s.tool == tool) {
                *existing_pid
            } else {
                let mut k: u32 = u32::MAX;
                while k > SYNTHETIC_SESSION_PID_BASE && sessions.contains_key(&k) {
                    k -= 1;
                }
                k
            }
        }
    };

    if let Some(existing) = sessions.get(&key)
        && !ai_types::accepts_event(existing.last_event_at_ms, emitted_at_ms)
    {
        return None;
    }

    if let Some(existing) = sessions.get(&key)
        && !ai_types::accepts_source(
            Some((existing.source, existing.last_activity.elapsed())),
            source,
        )
    {
        return None;
    }

    let now = std::time::Instant::now();
    let probe_start = |k: u32| {
        if k <= i32::MAX as u32 {
            paneflow_host::process::process_start_time(k)
        } else {
            None
        }
    };
    match sessions.get_mut(&key) {
        Some(s) => {
            s.waiting_since = ai_types::next_waiting_since(
                Some((&s.state, s.waiting_since)),
                &transition.state,
                now,
            );
            s.tool = tool;
            s.state = transition.state;
            s.active_tool_name = transition.active_tool_name;
            s.source = source;
            apply_field_update(&mut s.message, transition.message);
            apply_field_update(&mut s.last_result, transition.last_result);
            s.last_activity = now;
            s.last_event_at_ms = emitted_at_ms.or(s.last_event_at_ms);
            if s.proc_start.is_none() {
                s.proc_start = probe_start(key);
            }
        }
        None => {
            let mut session = ai_types::AgentSession::new(tool, transition.state);
            session.waiting_since = ai_types::next_waiting_since(None, &session.state, now);
            session.source = source;
            session.active_tool_name = transition.active_tool_name;
            apply_field_update(&mut session.message, transition.message);
            apply_field_update(&mut session.last_result, transition.last_result);
            session.last_activity = now;
            session.last_event_at_ms = emitted_at_ms;
            session.proc_start = probe_start(key);
            sessions.insert(key, session);
        }
    }
    Some(key)
}

fn read_emitted_at(params: &serde_json::Value) -> Option<u64> {
    paneflow_ipc_client::ai_hook::emitted_at_ms_from_wire_params(params)
}

fn apply_field_update<T>(slot: &mut T, update: ai_types::FieldUpdate<T>) {
    if let ai_types::FieldUpdate::Set(value) = update {
        *slot = value;
    }
}

fn stale_frame_response() -> serde_json::Value {
    serde_json::json!({"status": "stale"})
}

impl PaneFlowApp {
    pub(crate) fn broadcast_ai_frame(&self, method: &str, params: &serde_json::Value) {
        if !self.event_bus.has_subscribers() {
            return;
        }
        let workspace_id = params.get("workspace_id").and_then(|v| v.as_u64());
        let pid = read_session_pid(params);
        let explicit_surface_id = read_frame_surface_id(params);
        let tool = read_tool(params);
        let workspace = workspace_id.and_then(|wid| self.workspaces.iter().find(|w| w.id == wid));
        let session = workspace
            .and_then(|w| {
                pid.and_then(|p| w.agent_sessions.get(&p)).or_else(|| {
                    explicit_surface_id.and_then(|sid| {
                        w.agent_sessions
                            .values()
                            .find(|s| s.surface_id == Some(sid))
                    })
                })
            })
            .or_else(|| {
                explicit_surface_id.and_then(|sid| {
                    self.workspaces
                        .iter()
                        .flat_map(|w| w.agent_sessions.values())
                        .find(|s| s.surface_id == Some(sid))
                })
            });
        let (state, session_surface_id, message, active_tool) = match session {
            Some(s) => (
                Some(s.state.wire_str()),
                s.surface_id,
                s.message.clone(),
                s.active_tool_name.clone(),
            ),
            None => (None, None, None, None),
        };
        let surface_id = resolved_event_surface_id(session_surface_id, explicit_surface_id);
        let event = session_event_value(
            method,
            workspace_id,
            pid,
            tool.map(|t| t.binary()),
            state,
            surface_id,
            message.as_deref(),
            active_tool.as_deref(),
        );
        self.event_bus.broadcast(method, surface_id, &event);
    }

    fn validated_frame_surface_id(&self, params: &serde_json::Value, cx: &App) -> Option<u64> {
        let sid = read_frame_surface_id(params)?;
        find_terminal_by_surface_id(&self.workspaces, sid, cx)
            .is_some()
            .then_some(sid)
    }

    fn bind_or_resolve_session_surface(
        &mut self,
        ws_id: u64,
        session_key: u32,
        explicit_surface_id: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        if let Some(sid) = explicit_surface_id {
            self.set_session_surface(ws_id, session_key, sid, cx);
        } else {
            self.schedule_surface_resolution(ws_id, session_key, cx);
        }
    }

    pub(crate) fn schedule_surface_resolution(
        &mut self,
        ws_id: u64,
        session_key: u32,
        cx: &mut Context<Self>,
    ) {
        if session_key >= SYNTHETIC_SESSION_PID_BASE {
            return;
        }
        let already = self
            .workspaces
            .iter()
            .find(|ws| ws.id == ws_id)
            .and_then(|ws| ws.agent_sessions.get(&session_key))
            .is_none_or(|s| s.surface_id.is_some());
        if already {
            return;
        }
        let mut candidates: HashMap<u32, u64> = HashMap::new();
        for ws in &self.workspaces {
            for pane in ws.collect_panes() {
                for terminal in pane.read(cx).terminals() {
                    let pid = terminal.read(cx).terminal.child_pid;
                    if pid > 0 {
                        candidates.insert(pid, terminal.entity_id().as_u64());
                    }
                }
            }
        }
        if let Some(&sid) = candidates.get(&session_key) {
            self.set_session_surface(ws_id, session_key, sid, cx);
            return;
        }
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let resolved = smol::unblock(move || {
                    crate::workspace::pid_resolve::resolve_surface_for_pid(session_key, &candidates)
                })
                .await;
                if let Some(sid) = resolved {
                    let _ = cx.update(|cx| {
                        this.update(cx, |app, cx| {
                            app.set_session_surface(ws_id, session_key, sid, cx);
                        })
                    });
                }
            },
        )
        .detach();
    }

    pub(crate) fn set_session_surface(
        &mut self,
        ws_id: u64,
        key: u32,
        sid: u64,
        cx: &mut Context<Self>,
    ) {
        if let Some(ws) = self.workspaces.iter_mut().find(|ws| ws.id == ws_id)
            && bind_session_surface(&mut ws.agent_sessions, key, sid)
        {
            self.sync_attention(cx);
            self.agent_sessions_changed(cx);
            cx.notify();
            self.apply_pending_tab_title(ws_id, key, cx);
        }
    }

    pub(crate) fn apply_pending_tab_title(
        &mut self,
        ws_id: u64,
        session_key: u32,
        cx: &mut Context<Self>,
    ) {
        let Some(ws_idx) = self.workspaces.iter().position(|ws| ws.id == ws_id) else {
            return;
        };
        let Some((title, surface_id)) = self.workspaces[ws_idx]
            .agent_sessions
            .get(&session_key)
            .and_then(|session| Some((session.pending_tab_title.clone()?, session.surface_id?)))
        else {
            return;
        };
        let Some((tab_idx, surfaces)) = tab_for_surface(&self.workspaces[ws_idx], surface_id, cx)
        else {
            return;
        };
        if let Some(session) = self.workspaces[ws_idx].agent_sessions.get_mut(&session_key) {
            session.pending_tab_title = None;
        }
        let named = surfaces == 1
            && self.workspaces[ws_idx]
                .tab_mut(tab_idx)
                .is_some_and(|tab| tab.set_title(&title, TabTitleSource::Prompt));
        if named {
            self.save_session(cx);
            cx.notify();
        }
    }

    pub(crate) fn apply_projected_agent_metadata(
        &mut self,
        method: &str,
        params: &serde_json::Value,
        workspace_id: u64,
        surface_id: u64,
        cx: &mut Context<Self>,
    ) {
        let session_key = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .and_then(|workspace| {
                workspace
                    .agent_sessions
                    .iter()
                    .find(|(_, session)| session.surface_id == Some(surface_id))
                    .map(|(key, _)| *key)
            });
        let Some(tool) = read_tool(params) else {
            return;
        };
        match method {
            METHOD_SESSION_START => {
                if let Some(terminal) =
                    find_terminal_by_surface_id(&self.workspaces, surface_id, cx)
                {
                    terminal.update(cx, |view, cx| {
                        view.declare_agent(tool);
                        cx.notify();
                    });
                }
            }
            METHOD_PROMPT_SUBMIT => {
                let Some(session_key) = session_key else {
                    return;
                };
                if let Some(session) = self
                    .workspaces
                    .iter_mut()
                    .find(|workspace| workspace.id == workspace_id)
                    .and_then(|workspace| workspace.agent_sessions.get_mut(&session_key))
                {
                    if let Some(prompt) = read_hook_prompt(params) {
                        session
                            .auto_naming
                            .record(crate::auto_naming::Role::User, prompt);
                    }
                    if let Some(title) = read_hook_prompt_title(params) {
                        session.pending_tab_title = Some(title);
                    }
                }
                self.apply_pending_tab_title(workspace_id, session_key, cx);
            }
            METHOD_STOP if !is_interrupt_lifecycle_event(params) => {
                let Some(session_key) = session_key else {
                    return;
                };
                let (summary, transcript) = read_stop_summary(params);
                self.schedule_generated_title_scan(workspace_id, session_key, tool, params, cx);
                if let Some(summary) = summary.as_deref() {
                    self.record_auto_naming_message(
                        workspace_id,
                        session_key,
                        crate::auto_naming::Role::Assistant,
                        summary,
                    );
                }
                if let Some(path) = transcript {
                    Self::schedule_transcript_turn_end(
                        Some((workspace_id, session_key)),
                        path,
                        None,
                        cx,
                    );
                } else {
                    self.schedule_auto_naming(workspace_id, session_key, cx);
                }
            }
            _ => {}
        }
    }

    fn schedule_generated_title_scan(
        &mut self,
        ws_id: u64,
        session_key: u32,
        tool: crate::agent_launcher::TerminalAgent,
        params: &serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        if self.tab_title_is_settled(ws_id, session_key, cx) {
            return;
        }
        let Some(source) = generated_title_source(tool, params) else {
            return;
        };
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let Some(title) = smol::unblock(move || source.read()).await else {
                    return;
                };
                cx.update(|cx| {
                    let _ = this.update(cx, |app, cx| {
                        app.apply_generated_tab_title(ws_id, session_key, &title, cx);
                    });
                });
            },
        )
        .detach();
    }

    fn apply_generated_tab_title(
        &mut self,
        ws_id: u64,
        session_key: u32,
        title: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(ws_idx) = self.workspaces.iter().position(|ws| ws.id == ws_id) else {
            return;
        };
        let Some(surface_id) = self.workspaces[ws_idx]
            .agent_sessions
            .get(&session_key)
            .and_then(|session| session.surface_id)
        else {
            return;
        };
        let Some((tab_idx, surfaces)) = tab_for_surface(&self.workspaces[ws_idx], surface_id, cx)
        else {
            return;
        };
        if surfaces == 1
            && self.workspaces[ws_idx]
                .tab_mut(tab_idx)
                .is_some_and(|tab| tab.set_title(title, TabTitleSource::Generated))
        {
            self.save_session(cx);
            cx.notify();
        }
    }

    fn tab_title_is_settled(&self, ws_id: u64, session_key: u32, cx: &App) -> bool {
        let Some(ws) = self.workspaces.iter().find(|ws| ws.id == ws_id) else {
            return false;
        };
        let Some(surface_id) = ws
            .agent_sessions
            .get(&session_key)
            .and_then(|session| session.surface_id)
        else {
            return false;
        };
        let Some((tab_idx, surfaces)) = tab_for_surface(ws, surface_id, cx) else {
            return false;
        };
        surfaces > 1 || ws.tabs().get(tab_idx).is_some_and(Tab::title_is_settled)
    }

    pub(crate) fn sync_attention(&self, cx: &mut Context<Self>) {
        let mut waiting: HashMap<u64, Option<String>> = HashMap::new();
        let mut errored: std::collections::HashSet<u64> = std::collections::HashSet::new();
        for ws in &self.workspaces {
            for session in ws.agent_sessions.values() {
                let Some(sid) = session.surface_id else {
                    continue;
                };
                match session.state {
                    ai_types::AgentState::WaitingForInput => {
                        waiting.insert(sid, session.message.clone());
                    }
                    ai_types::AgentState::Errored => {
                        errored.insert(sid);
                    }
                    _ => {}
                }
            }
        }
        for ws in &self.workspaces {
            for pane in ws.collect_panes() {
                let sid = pane
                    .read(cx)
                    .active_terminal_opt()
                    .map(|t| t.entity_id().as_u64());
                let attention = sid.and_then(|sid| waiting.get(&sid).cloned()).flatten();
                let is_errored = sid.is_some_and(|sid| errored.contains(&sid));
                pane.update(cx, |p, cx| {
                    p.set_attention(attention, cx);
                    p.set_errored(is_errored, cx);
                });
            }
        }
    }

    pub(super) fn handle_agent_frame(
        &mut self,
        method: &str,
        params: &serde_json::Value,
        cx: &mut Context<Self>,
    ) -> serde_json::Value {
        match method {
            METHOD_SESSION_START => {
                let Some(workspace_id) = params.get("workspace_id").and_then(|v| v.as_u64()) else {
                    return serde_json::json!({"error": "Missing workspace_id"});
                };
                let Some(pid) = read_session_pid(params) else {
                    return serde_json::json!({"error": "Missing or invalid pid"});
                };
                let Some(tool) = read_tool(params) else {
                    return serde_json::json!({"error": "Unknown tool"});
                };
                let explicit_surface_id = self.validated_frame_surface_id(params, cx);

                if self.workspaces.iter().any(|ws| ws.id == workspace_id) {
                    let _ = pid;
                    if let Some(sid) = explicit_surface_id
                        && let Some(terminal) =
                            find_terminal_by_surface_id(&self.workspaces, sid, cx)
                    {
                        terminal.update(cx, |view, cx| {
                            view.declare_agent(tool);
                            cx.notify();
                        });
                        cx.notify();
                    }
                    serde_json::json!({"registered": true})
                } else {
                    serde_json::json!({"error": format!("Unknown workspace_id: {workspace_id}")})
                }
            }
            METHOD_PROMPT_SUBMIT => {
                let Some(workspace_id) = params.get("workspace_id").and_then(|v| v.as_u64()) else {
                    return serde_json::json!({"error": "Missing workspace_id"});
                };
                let pid = read_session_pid(params);
                let Some(tool) = read_tool(params) else {
                    return serde_json::json!({"error": "Unknown tool"});
                };
                let explicit_surface_id = self.validated_frame_surface_id(params, cx);

                if let Some(ws) = self.workspaces.iter_mut().find(|ws| ws.id == workspace_id) {
                    let Some(key) = upsert_session_state(
                        &mut ws.agent_sessions,
                        pid,
                        tool,
                        ai_types::reduce_lifecycle_event(
                            ai_types::AgentLifecycleEvent::PromptSubmit,
                        ),
                        read_emitted_at(params),
                        ai_types::AgentStateSource::Hook,
                    ) else {
                        return stale_frame_response();
                    };
                    if let Some(session) = ws.agent_sessions.get_mut(&key) {
                        if let Some(prompt) = read_hook_prompt(params) {
                            session
                                .auto_naming
                                .record(crate::auto_naming::Role::User, prompt);
                        }
                        if let Some(title) = read_hook_prompt_title(params) {
                            session.pending_tab_title = Some(title);
                        }
                    }
                    cx.notify();
                    self.bind_or_resolve_session_surface(
                        workspace_id,
                        key,
                        explicit_surface_id,
                        cx,
                    );
                    self.apply_pending_tab_title(workspace_id, key, cx);
                    self.sync_attention(cx);
                    self.agent_sessions_changed(cx);
                    serde_json::json!({"status": "running"})
                } else {
                    serde_json::json!({"error": format!("Unknown workspace_id: {workspace_id}")})
                }
            }
            METHOD_TOOL_USE => {
                let Some(workspace_id) = params.get("workspace_id").and_then(|v| v.as_u64()) else {
                    return serde_json::json!({"error": "Missing workspace_id"});
                };
                let hook = params.get("hook_payload");
                let active_tool_name = hook
                    .and_then(|h| h.get("tool_name"))
                    .and_then(|v| v.as_str())
                    .or_else(|| params.get("tool_name").and_then(|v| v.as_str()))
                    .map(|s| s.chars().take(128).collect::<String>());
                let pid = read_session_pid(params);
                let Some(tool) = read_tool(params) else {
                    return serde_json::json!({"error": "Unknown tool"});
                };
                let explicit_surface_id = self.validated_frame_surface_id(params, cx);

                if let Some(ws) = self.workspaces.iter_mut().find(|ws| ws.id == workspace_id) {
                    let Some(key) = upsert_session_state(
                        &mut ws.agent_sessions,
                        pid,
                        tool,
                        ai_types::reduce_lifecycle_event(ai_types::AgentLifecycleEvent::ToolUse {
                            tool_name: active_tool_name,
                        }),
                        read_emitted_at(params),
                        ai_types::AgentStateSource::Hook,
                    ) else {
                        return stale_frame_response();
                    };
                    cx.notify();
                    self.bind_or_resolve_session_surface(
                        workspace_id,
                        key,
                        explicit_surface_id,
                        cx,
                    );
                    self.sync_attention(cx);
                    self.agent_sessions_changed(cx);
                    serde_json::json!({"status": "running"})
                } else {
                    serde_json::json!({"error": format!("Unknown workspace_id: {workspace_id}")})
                }
            }
            METHOD_NOTIFICATION => {
                let Some(workspace_id) = params.get("workspace_id").and_then(|v| v.as_u64()) else {
                    return serde_json::json!({"error": "Missing workspace_id"});
                };
                let pid = read_session_pid(params);
                let Some(tool) = read_tool(params) else {
                    return serde_json::json!({"error": "Unknown tool"});
                };
                let explicit_surface_id = self.validated_frame_surface_id(params, cx);
                let message = read_notification_message(params);
                let notify_config = self.cached_config.clone();
                if let Some(ws) = self.workspaces.iter_mut().find(|ws| ws.id == workspace_id) {
                    let Some(key) = upsert_session_state(
                        &mut ws.agent_sessions,
                        pid,
                        tool,
                        ai_types::reduce_lifecycle_event(
                            ai_types::AgentLifecycleEvent::Notification {
                                message: message.clone(),
                            },
                        ),
                        read_emitted_at(params),
                        ai_types::AgentStateSource::Hook,
                    ) else {
                        return stale_frame_response();
                    };
                    let ws_title = ws.title.clone();
                    cx.notify();
                    self.bind_or_resolve_session_surface(
                        workspace_id,
                        key,
                        explicit_surface_id,
                        cx,
                    );
                    fire_attention_notification(
                        tool,
                        &ws_title,
                        message.as_deref(),
                        &notify_config,
                        self.session_is_seen(workspace_id, key, cx)
                            || self.workspace_is_muted(workspace_id),
                        cx.background_executor().clone(),
                    );
                    self.sync_attention(cx);
                    self.agent_sessions_changed(cx);
                    serde_json::json!({"status": "waiting"})
                } else {
                    serde_json::json!({"error": format!("Unknown workspace_id: {workspace_id}")})
                }
            }
            METHOD_STOP => {
                let Some(workspace_id) = params.get("workspace_id").and_then(|v| v.as_u64()) else {
                    return serde_json::json!({"error": "Missing workspace_id"});
                };
                let pid = read_session_pid(params);
                let Some(tool) = read_tool(params) else {
                    return serde_json::json!({"error": "Unknown tool"});
                };
                let explicit_surface_id = self.validated_frame_surface_id(params, cx);
                let notify_config = self.cached_config.clone();
                let hook_sourced = frame_is_hook_sourced(params);
                let visible_surfaces = self.surfaces_under_user_eye(workspace_id, cx);
                if let Some(ws) = self.workspaces.iter_mut().find(|ws| ws.id == workspace_id) {
                    let interrupt_stop = is_interrupt_lifecycle_event(params);
                    let (session_summary, transcript_to_read) = if interrupt_stop {
                        (None, None)
                    } else {
                        read_stop_summary(params)
                    };
                    let Some(session_key) = upsert_session_state(
                        &mut ws.agent_sessions,
                        pid,
                        tool,
                        ai_types::reduce_lifecycle_event(ai_types::AgentLifecycleEvent::Stop {
                            summary: session_summary.clone(),
                        }),
                        read_emitted_at(params),
                        ai_types::AgentStateSource::Hook,
                    ) else {
                        return stale_frame_response();
                    };
                    let finished_surface = ws
                        .agent_sessions
                        .get(&session_key)
                        .and_then(|session| session.surface_id);
                    let seen = crate::app::agent_status::completion_was_seen(
                        visible_surfaces.as_ref(),
                        finished_surface,
                    ) || ws.muted;
                    if !interrupt_stop && hook_sourced {
                        ws.agent_completion_notification
                            .record_finished(seen, finished_surface);
                    }
                    let ws_title = ws.title.clone();
                    cx.notify();
                    if !interrupt_stop {
                        self.schedule_generated_title_scan(
                            workspace_id,
                            session_key,
                            tool,
                            params,
                            cx,
                        );
                        if let Some(summary) = session_summary.as_deref() {
                            self.record_auto_naming_message(
                                workspace_id,
                                session_key,
                                crate::auto_naming::Role::Assistant,
                                summary,
                            );
                        }
                        if transcript_to_read.is_none() {
                            self.schedule_auto_naming(workspace_id, session_key, cx);
                        }
                    }
                    if !interrupt_stop {
                        if let Some(path) = transcript_to_read {
                            Self::schedule_transcript_turn_end(
                                Some((workspace_id, session_key)),
                                path,
                                Some(TranscriptTurnEndNotification {
                                    agent: tool,
                                    title: ws_title.clone(),
                                    config: notify_config.clone(),
                                    seen,
                                    executor: cx.background_executor().clone(),
                                }),
                                cx,
                            );
                        } else {
                            fire_turn_end_notification(
                                tool,
                                &ws_title,
                                session_summary.as_deref(),
                                &notify_config,
                                seen,
                                cx.background_executor().clone(),
                            );
                        }
                    }
                    self.bind_or_resolve_session_surface(
                        workspace_id,
                        session_key,
                        explicit_surface_id,
                        cx,
                    );
                    self.sync_attention(cx);
                    self.agent_sessions_changed(cx);

                    let ws_id = workspace_id;
                    cx.spawn(
                        async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                            smol::Timer::after(std::time::Duration::from_secs(5)).await;
                            cx.update(|cx| {
                                let _ = this.update(cx, |app, cx| {
                                    if let Some(ws) =
                                        app.workspaces.iter_mut().find(|ws| ws.id == ws_id)
                                        && matches!(
                                            ws.agent_sessions.get(&session_key).map(|s| &s.state),
                                            Some(ai_types::AgentState::Finished)
                                        )
                                    {
                                        ws.agent_sessions.remove(&session_key);
                                        app.sync_attention(cx);
                                        app.agent_sessions_changed(cx);
                                        cx.notify();
                                    }
                                });
                            });
                        },
                    )
                    .detach();

                    serde_json::json!({"status": "idle"})
                } else {
                    serde_json::json!({"error": format!("Unknown workspace_id: {workspace_id}")})
                }
            }
            METHOD_EXIT => {
                let Some(workspace_id) = params.get("workspace_id").and_then(|v| v.as_u64()) else {
                    return serde_json::json!({"error": "Missing workspace_id"});
                };
                let Some(exit_code) = params
                    .get("exit_code")
                    .and_then(|v| v.as_i64())
                    .and_then(|n| i32::try_from(n).ok())
                else {
                    return serde_json::json!({"error": "Missing or invalid exit_code"});
                };
                let pid = read_session_pid(params);
                let Some(tool) = read_tool(params) else {
                    return serde_json::json!({"error": "Unknown tool"});
                };
                let explicit_surface_id = self.validated_frame_surface_id(params, cx);
                let notify_config = self.cached_config.clone();
                if let Some(ws) = self.workspaces.iter_mut().find(|ws| ws.id == workspace_id) {
                    let transition =
                        ai_types::reduce_lifecycle_event(ai_types::AgentLifecycleEvent::Exit {
                            exit_code,
                        });
                    let errored = transition.state == ai_types::AgentState::Errored;
                    let Some(key) = upsert_session_state(
                        &mut ws.agent_sessions,
                        pid,
                        tool,
                        transition,
                        read_emitted_at(params),
                        ai_types::AgentStateSource::Hook,
                    ) else {
                        return stale_frame_response();
                    };
                    let ws_title = ws.title.clone();
                    cx.notify();
                    self.bind_or_resolve_session_surface(
                        workspace_id,
                        key,
                        explicit_surface_id,
                        cx,
                    );
                    if errored {
                        fire_agent_exit_notification(
                            tool,
                            &ws_title,
                            exit_code,
                            &notify_config,
                            self.session_is_seen(workspace_id, key, cx)
                                || self.workspace_is_muted(workspace_id),
                            cx.background_executor().clone(),
                        );
                    }
                    self.sync_attention(cx);
                    self.agent_sessions_changed(cx);
                    serde_json::json!({"status": if errored { "errored" } else { "finished" }})
                } else {
                    serde_json::json!({"error": format!("Unknown workspace_id: {workspace_id}")})
                }
            }
            METHOD_SESSION_END => {
                let Some(workspace_id) = params.get("workspace_id").and_then(|v| v.as_u64()) else {
                    return serde_json::json!({"error": "Missing workspace_id"});
                };
                let tool_name = match AiToolName::from_wire_params(params) {
                    Ok(tool_name) => tool_name,
                    Err(_) => return serde_json::json!({"error": "Invalid tool name"}),
                };
                let pid = read_session_pid(params);
                let tool = crate::agent_launcher::TerminalAgent::from_binary(tool_name.as_str());
                let explicit_surface_id = self.validated_frame_surface_id(params, cx);

                if let Some(ws) = self.workspaces.iter_mut().find(|ws| ws.id == workspace_id) {
                    let is_errored =
                        |s: &ai_types::AgentSession| s.state == ai_types::AgentState::Errored;
                    let removed = if let Some(p) = pid
                        && ws
                            .agent_sessions
                            .get(&p)
                            .is_some_and(|session| !is_errored(session))
                    {
                        ws.agent_sessions.remove(&p).is_some()
                    } else if pid.is_some_and(|p| ws.agent_sessions.contains_key(&p)) {
                        false
                    } else {
                        let pid_to_remove = session_end_fallback_candidate(
                            &ws.agent_sessions,
                            tool,
                            explicit_surface_id,
                        );
                        if let Some(k) = pid_to_remove {
                            ws.agent_sessions.remove(&k);
                            true
                        } else {
                            false
                        }
                    };
                    if removed {
                        self.sync_attention(cx);
                        self.agent_sessions_changed(cx);
                        cx.notify();
                    }
                    serde_json::json!({"cleared": removed})
                } else {
                    serde_json::json!({"error": format!("Unknown workspace_id: {workspace_id}")})
                }
            }
            _ => JsonRpcError::method_not_found(format!("Method not found: {method}")).into_value(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_session_pid_rejects_server_reserved_high_band() {
        let pid = |v: serde_json::Value| read_session_pid(&serde_json::json!({ "pid": v }));
        assert_eq!(pid(serde_json::json!(1234)), Some(1234));
        assert_eq!(pid(serde_json::json!(i32::MAX as u32)), Some(2147483647));
        assert_eq!(pid(serde_json::json!(i32::MAX as u32 + 1)), None);
        assert_eq!(
            pid(serde_json::json!(0xFFFF_0000u32)),
            None,
            "synthetic band floor"
        );
        assert_eq!(pid(serde_json::json!(u32::MAX)), None);
        assert_eq!(pid(serde_json::json!(0)), None);
        assert_eq!(read_session_pid(&serde_json::json!({})), None);
    }

    #[test]
    fn read_frame_surface_id_accepts_top_level_or_hook_payload() {
        assert_eq!(
            read_frame_surface_id(&serde_json::json!({ "surface_id": 42 })),
            Some(42)
        );
        assert_eq!(
            read_frame_surface_id(&serde_json::json!({
                "hook_payload": { "surface_id": 7 }
            })),
            Some(7)
        );
        assert_eq!(
            read_frame_surface_id(&serde_json::json!({ "surface_id": 0 })),
            None
        );
        assert_eq!(read_frame_surface_id(&serde_json::json!({})), None);
    }

    #[test]
    fn event_surface_id_falls_back_to_explicit_frame_surface() {
        assert_eq!(super::resolved_event_surface_id(Some(7), Some(9)), Some(7));
        assert_eq!(super::resolved_event_surface_id(None, Some(9)), Some(9));
        assert_eq!(super::resolved_event_surface_id(None, None), None);
    }

    #[test]
    fn upsert_session_state_transitions_keys_and_stamps() {
        use crate::agent_launcher::TerminalAgent;
        use crate::ai_types::{
            AgentLifecycleEvent, AgentSession, AgentState, reduce_lifecycle_event,
        };
        let mut sessions: std::collections::HashMap<u32, AgentSession> =
            std::collections::HashMap::new();

        let key = super::upsert_session_state(
            &mut sessions,
            Some(4242),
            TerminalAgent::ClaudeCode,
            reduce_lifecycle_event(AgentLifecycleEvent::ToolUse {
                tool_name: Some("Edit".into()),
            }),
            Some(1_000),
            crate::ai_types::AgentStateSource::Hook,
        )
        .expect("a first frame is never stale");
        assert_eq!(key, 4242);
        assert_eq!(sessions[&4242].state, AgentState::Thinking);
        assert_eq!(sessions[&4242].active_tool_name.as_deref(), Some("Edit"));

        let key = super::upsert_session_state(
            &mut sessions,
            Some(4242),
            TerminalAgent::ClaudeCode,
            reduce_lifecycle_event(AgentLifecycleEvent::Notification {
                message: Some("Approve edit?".into()),
            }),
            Some(1_100),
            crate::ai_types::AgentStateSource::Hook,
        )
        .expect("a forward frame applies");
        assert_eq!(key, 4242, "same PID updates in place");
        assert_eq!(sessions.len(), 1, "no duplicate session for the same PID");
        assert_eq!(sessions[&4242].state, AgentState::WaitingForInput);
        assert!(sessions[&4242].active_tool_name.is_none());
        assert!(
            sessions[&4242].waiting_since.is_some(),
            "wait stamp set on entering WaitingForInput"
        );
        assert_eq!(sessions[&4242].message.as_deref(), Some("Approve edit?"));

        assert_eq!(
            super::upsert_session_state(
                &mut sessions,
                Some(4242),
                TerminalAgent::ClaudeCode,
                reduce_lifecycle_event(AgentLifecycleEvent::Stop { summary: None }),
                Some(1_050),
                crate::ai_types::AgentStateSource::Hook,
            ),
            None
        );
        assert_eq!(sessions[&4242].state, AgentState::WaitingForInput);
        assert_eq!(sessions[&4242].message.as_deref(), Some("Approve edit?"));

        let key = super::upsert_session_state(
            &mut sessions,
            None,
            TerminalAgent::ClaudeCode,
            reduce_lifecycle_event(AgentLifecycleEvent::Stop {
                summary: Some("done".into()),
            }),
            None,
            crate::ai_types::AgentStateSource::Hook,
        )
        .expect("an unstamped frame is accepted");
        assert_eq!(
            key, 4242,
            "a no-pid frame matches the existing tool session"
        );
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[&4242].state, AgentState::Finished);
        assert_eq!(sessions[&4242].last_result.as_deref(), Some("done"));
        assert!(sessions[&4242].message.is_none());
        assert_eq!(sessions[&4242].last_event_at_ms, Some(1_100));

        let mut fresh: std::collections::HashMap<u32, AgentSession> =
            std::collections::HashMap::new();
        let key = super::upsert_session_state(
            &mut fresh,
            None,
            TerminalAgent::Codex,
            reduce_lifecycle_event(AgentLifecycleEvent::PromptSubmit),
            None,
            crate::ai_types::AgentStateSource::Hook,
        )
        .expect("a first frame is never stale");
        assert!(
            key >= super::SYNTHETIC_SESSION_PID_BASE,
            "synthetic key lands in the reserved band"
        );
    }

    #[test]
    fn a_weaker_source_is_refused_at_the_write_choke_point() {
        use crate::agent_launcher::TerminalAgent;
        use crate::ai_types::{
            AgentLifecycleEvent, AgentSession, AgentState, AgentStateSource, reduce_lifecycle_event,
        };
        let mut sessions: std::collections::HashMap<u32, AgentSession> =
            std::collections::HashMap::new();

        super::upsert_session_state(
            &mut sessions,
            Some(4242),
            TerminalAgent::ClaudeCode,
            reduce_lifecycle_event(AgentLifecycleEvent::Notification {
                message: Some("Approve edit?".into()),
            }),
            None,
            AgentStateSource::Hook,
        )
        .expect("a first frame is never stale");

        assert_eq!(
            super::upsert_session_state(
                &mut sessions,
                Some(4242),
                TerminalAgent::ClaudeCode,
                reduce_lifecycle_event(AgentLifecycleEvent::PromptSubmit),
                None,
                AgentStateSource::Terminal,
            ),
            None,
            "the terminal channel cannot talk over a live hook"
        );
        assert_eq!(sessions[&4242].state, AgentState::WaitingForInput);
        assert_eq!(sessions[&4242].message.as_deref(), Some("Approve edit?"));

        super::upsert_session_state(
            &mut sessions,
            Some(4242),
            TerminalAgent::ClaudeCode,
            reduce_lifecycle_event(AgentLifecycleEvent::Stop { summary: None }),
            None,
            AgentStateSource::Hook,
        )
        .expect("the stronger source applies");
        assert_eq!(sessions[&4242].state, AgentState::Finished);
        assert_eq!(sessions[&4242].source, AgentStateSource::Hook);
    }

    #[test]
    fn a_hook_free_session_is_driven_by_the_sources_that_remain() {
        use crate::agent_launcher::TerminalAgent;
        use crate::ai_types::{
            AgentLifecycleEvent, AgentSession, AgentState, AgentStateSource, reduce_lifecycle_event,
        };
        let mut sessions: std::collections::HashMap<u32, AgentSession> =
            std::collections::HashMap::new();

        super::upsert_session_state(
            &mut sessions,
            Some(4242),
            TerminalAgent::ClaudeCode,
            reduce_lifecycle_event(AgentLifecycleEvent::PromptSubmit),
            None,
            AgentStateSource::SessionRegistry,
        )
        .expect("nothing holds the session yet");
        assert_eq!(sessions[&4242].state, AgentState::Thinking);

        super::upsert_session_state(
            &mut sessions,
            Some(4242),
            TerminalAgent::ClaudeCode,
            reduce_lifecycle_event(AgentLifecycleEvent::Notification {
                message: Some("input needed".into()),
            }),
            None,
            AgentStateSource::SessionRegistry,
        )
        .expect("the same source always applies");
        assert_eq!(sessions[&4242].state, AgentState::WaitingForInput);
        assert!(sessions[&4242].waiting_since.is_some());

        super::upsert_session_state(
            &mut sessions,
            Some(4242),
            TerminalAgent::ClaudeCode,
            reduce_lifecycle_event(AgentLifecycleEvent::PromptSubmit),
            None,
            AgentStateSource::SessionRegistry,
        )
        .expect("the same source always applies");
        assert_eq!(sessions[&4242].state, AgentState::Thinking);
        assert!(sessions[&4242].message.is_none());
        assert!(sessions[&4242].waiting_since.is_none());
    }

    #[test]
    fn session_end_fallback_requires_unique_candidate() {
        use crate::agent_launcher::TerminalAgent;
        use crate::ai_types::{AgentSession, AgentState};

        let mut sessions: std::collections::HashMap<u32, AgentSession> =
            std::collections::HashMap::new();
        let mut first = AgentSession::new(TerminalAgent::ClaudeCode, AgentState::Thinking);
        first.surface_id = Some(10);
        let mut second = AgentSession::new(TerminalAgent::ClaudeCode, AgentState::Thinking);
        second.surface_id = Some(11);
        sessions.insert(100, first);
        sessions.insert(200, second);

        assert_eq!(
            super::session_end_fallback_candidate(&sessions, Some(TerminalAgent::ClaudeCode), None),
            None,
            "tool-only fallback must not pick an arbitrary sibling"
        );
        assert_eq!(
            super::session_end_fallback_candidate(
                &sessions,
                Some(TerminalAgent::ClaudeCode),
                Some(11)
            ),
            Some(200),
            "surface_id disambiguates legacy no-pid session_end"
        );

        sessions.get_mut(&200).expect("session exists").state = AgentState::Errored;
        assert_eq!(
            super::session_end_fallback_candidate(&sessions, Some(TerminalAgent::ClaudeCode), None),
            Some(100),
            "errored rows are not fallback-removal candidates"
        );
    }

    #[test]
    fn binding_a_surface_leaves_one_session_per_tool_on_it() {
        let mut sessions = std::collections::HashMap::new();
        let mut by_shell = AgentSession::new(
            TerminalAgent::ClaudeCode,
            crate::ai_types::AgentState::Thinking,
        );
        by_shell.surface_id = Some(11);
        by_shell.last_result = Some("earlier turn".into());
        sessions.insert(4000, by_shell);
        let by_registry = AgentSession::new(
            TerminalAgent::ClaudeCode,
            crate::ai_types::AgentState::Thinking,
        );
        sessions.insert(4242, by_registry);
        let mut codex =
            AgentSession::new(TerminalAgent::Codex, crate::ai_types::AgentState::Thinking);
        codex.surface_id = Some(11);
        sessions.insert(5000, codex);

        assert!(super::bind_session_surface(&mut sessions, 4242, 11));
        assert!(
            !sessions.contains_key(&4000),
            "the shell-keyed twin of the same agent on the same pane is folded away"
        );
        assert_eq!(
            sessions[&4242].last_result.as_deref(),
            Some("earlier turn"),
            "what the twin knew is carried over"
        );
        assert!(
            sessions.contains_key(&5000),
            "another agent on the same pane is not a twin"
        );
        assert!(
            !super::bind_session_surface(&mut sessions, 4242, 11),
            "rebinding to the same pane with no twin left changes nothing"
        );
        assert!(!super::bind_session_surface(&mut sessions, 9, 11));
    }

    #[test]
    fn read_notification_message_is_optional_and_sanitized() {
        let p = serde_json::json!({"hook_payload": {"message": "Approve?"}});
        assert_eq!(
            super::read_notification_message(&p).as_deref(),
            Some("Approve?")
        );

        let p = serde_json::json!({"message": " \u{202E} "});
        assert!(super::read_notification_message(&p).is_none());
        assert!(super::read_notification_message(&serde_json::json!({})).is_none());
    }

    #[test]
    fn interrupt_lifecycle_event_can_be_top_level_or_hook_payload() {
        let p = serde_json::json!({"event_source": "interrupt"});
        assert!(super::is_interrupt_lifecycle_event(&p));

        let p = serde_json::json!({"hook_payload": {"event_source": "interrupt"}});
        assert!(super::is_interrupt_lifecycle_event(&p));

        let p = serde_json::json!({"event_source": "natural"});
        assert!(!super::is_interrupt_lifecycle_event(&p));
    }

    #[test]
    fn session_event_value_carries_method_and_session_fields() {
        let v = session_event_value(
            "ai.stop",
            Some(7),
            Some(4321),
            Some("claude"),
            Some("finished"),
            Some(42),
            None,
            None,
        );
        assert_eq!(v["type"], "ai.stop");
        assert_eq!(v["workspace_id"], 7);
        assert_eq!(v["pid"], 4321);
        assert_eq!(v["tool"], "claude");
        assert_eq!(v["state"], "finished");
        assert_eq!(v["surface_id"], 42);
        assert!(v.get("ts").is_some());
    }

    #[test]
    fn session_event_value_nulls_missing_fields() {
        let v = session_event_value("ai.session_end", None, None, None, None, None, None, None);
        assert_eq!(v["type"], "ai.session_end");
        assert_eq!(v["pid"], serde_json::Value::Null);
        assert_eq!(v["surface_id"], serde_json::Value::Null);
    }

    #[test]
    fn a_prompt_frame_yields_the_title_its_payload_carries() {
        let frame = serde_json::json!({
            "hook_payload": { "prompt": "fix the flaky worktree test now please" },
        });
        assert_eq!(
            read_hook_prompt_title(&frame).as_deref(),
            Some("fix the flaky worktree test now")
        );
    }

    #[test]
    fn a_prompt_frame_without_a_usable_prompt_yields_no_title() {
        for frame in [
            serde_json::json!({}),
            serde_json::json!({ "hook_payload": {} }),
            serde_json::json!({ "hook_payload": { "user_input": "hello" } }),
            serde_json::json!({ "hook_payload": { "prompt": "   " } }),
            serde_json::json!({ "hook_payload": { "prompt": 42 } }),
        ] {
            assert_eq!(read_hook_prompt_title(&frame), None, "{frame}");
        }
    }

    #[test]
    fn a_generated_title_is_only_looked_for_where_one_exists() {
        use crate::agent_launcher::TerminalAgent;

        #[cfg(windows)]
        let transcript = r"C:\abs\session.jsonl";
        #[cfg(not(windows))]
        let transcript = "/abs/session.jsonl";
        let frame = serde_json::json!({
            "hook_payload": { "transcript_path": transcript },
        });

        assert!(
            generated_title_source(TerminalAgent::ClaudeCode, &frame).is_some(),
            "Claude Code writes an ai-title record into the transcript"
        );
        for tool in [
            TerminalAgent::Codex,
            TerminalAgent::Pi,
            TerminalAgent::OpenCode,
            TerminalAgent::Gemini,
        ] {
            assert!(
                generated_title_source(tool, &frame).is_none(),
                "{tool:?} has no generated title reachable from a hook frame"
            );
        }
    }

    #[test]
    fn a_generated_title_needs_the_frame_to_say_where_to_look() {
        use crate::agent_launcher::TerminalAgent;

        for frame in [
            serde_json::json!({}),
            serde_json::json!({ "hook_payload": {} }),
            serde_json::json!({ "hook_payload": { "transcript_path": "" } }),
            serde_json::json!({ "hook_payload": { "transcript_path": "relative.jsonl" } }),
        ] {
            assert!(
                generated_title_source(TerminalAgent::ClaudeCode, &frame).is_none(),
                "{frame}"
            );
        }
    }
}
