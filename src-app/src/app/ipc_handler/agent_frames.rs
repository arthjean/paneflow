use super::*;

fn is_interrupt_lifecycle_event(params: &serde_json::Value) -> bool {
    LifecycleEventSource::from_wire_params(params) == Some(LifecycleEventSource::Interrupt)
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

fn attention_surfaces<'a>(
    sessions: impl Iterator<Item = &'a AgentSession>,
) -> (HashSet<u64>, HashSet<u64>) {
    let mut waiting = HashSet::new();
    let mut errored = HashSet::new();
    for session in sessions {
        let Some(sid) = session.surface_id else {
            continue;
        };
        match session.state {
            ai_types::AgentState::WaitingForInput => {
                waiting.insert(sid);
            }
            ai_types::AgentState::Errored => {
                errored.insert(sid);
            }
            _ => {}
        }
    }
    (waiting, errored)
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
        self.apply_conversation_signal(method, params, surface_id, cx);
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
        let (waiting, errored) = attention_surfaces(
            self.workspaces
                .iter()
                .flat_map(|ws| ws.agent_sessions.values()),
        );
        for ws in &self.workspaces {
            for pane in ws.collect_panes() {
                let sid = pane
                    .read(cx)
                    .active_terminal_opt()
                    .map(|t| t.entity_id().as_u64());
                let attention = sid.is_some_and(|sid| waiting.contains(&sid));
                let is_errored = sid.is_some_and(|sid| errored.contains(&sid));
                pane.update(cx, |p, cx| {
                    p.set_attention(attention, cx);
                    p.set_errored(is_errored, cx);
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_waiting_agent_without_a_message_still_raises_attention() {
        let mut silent = AgentSession::new(
            crate::agent_launcher::TerminalAgent::ClaudeCode,
            ai_types::AgentState::WaitingForInput,
        );
        silent.surface_id = Some(7);
        silent.message = None;
        let mut asking = silent.clone();
        asking.surface_id = Some(8);
        asking.message = Some("Approve?".into());
        let mut errored = silent.clone();
        errored.surface_id = Some(9);
        errored.state = ai_types::AgentState::Errored;

        let (waiting, failed) = attention_surfaces([&silent, &asking, &errored].into_iter());
        assert_eq!(waiting, HashSet::from([7, 8]));
        assert_eq!(failed, HashSet::from([9]));
    }

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
