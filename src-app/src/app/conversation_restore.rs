use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use gpui::{Context, Entity, Window};
use paneflow_ipc_client::ai_hook::{METHOD_EXIT, METHOD_SESSION_END, METHOD_SESSION_START};
use serde_json::Value;

use crate::PaneFlowApp;
use crate::agent_resume::ConversationTemplate;
use crate::app::ipc_handler::find_terminal_by_surface_id;
use crate::app::workspace_ops::SurfaceLaunch;
use crate::layout::SplitDirection;
use crate::terminal::TerminalView;
use crate::terminal::view::conversation::AgentSessionSignal;

pub(crate) const RESUME_SPACING: Duration = Duration::from_millis(250);
pub(crate) const RESUME_FRONT_PATIENCE: Duration = Duration::from_secs(8);
pub(crate) const FORK_TOOLTIP: &str =
    "Opens a branch of this conversation in a split; both conversations share the same files";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestoreCandidate {
    pub(crate) surface: u64,
    pub(crate) runtime: String,
    pub(crate) id: String,
    pub(crate) cwd: Option<String>,
    pub(crate) location: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct RestorePlan {
    pub(crate) order: Vec<u64>,
    pub(crate) duplicates: Vec<(u64, String)>,
    pub(crate) shared_folder: Vec<u64>,
}

pub(crate) fn plan_restore_order(candidates: Vec<RestoreCandidate>) -> RestorePlan {
    let mut owners: HashMap<(String, String), String> = HashMap::new();
    let mut continuing: HashMap<(String, Option<String>), usize> = HashMap::new();
    for candidate in candidates
        .iter()
        .filter(|candidate| candidate.id.is_empty())
    {
        *continuing
            .entry((candidate.runtime.clone(), candidate.cwd.clone()))
            .or_default() += 1;
    }
    let mut plan = RestorePlan::default();
    for candidate in candidates {
        if candidate.id.is_empty() {
            if continuing
                .get(&(candidate.runtime, candidate.cwd))
                .is_some_and(|count| *count > 1)
            {
                plan.shared_folder.push(candidate.surface);
            } else {
                plan.order.push(candidate.surface);
            }
            continue;
        }
        let key = (candidate.runtime, candidate.id);
        match owners.get(&key) {
            Some(owner) => plan.duplicates.push((candidate.surface, owner.clone())),
            None => {
                owners.insert(key, candidate.location);
                plan.order.push(candidate.surface);
            }
        }
    }
    plan
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrontState {
    Gone,
    Waiting,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DrainStep {
    Idle,
    Drop,
    Write,
    WaitUntil(Instant),
}

#[derive(Debug, Default)]
pub(crate) struct ConversationRestoreQueue {
    order: VecDeque<u64>,
    front_since: Option<Instant>,
    last_write: Option<Instant>,
    wake_at: Option<Instant>,
}

impl ConversationRestoreQueue {
    pub(crate) fn front(&self) -> Option<u64> {
        self.order.front().copied()
    }

    pub(crate) fn step(&mut self, now: Instant, front: FrontState) -> DrainStep {
        if self.order.is_empty() {
            self.front_since = None;
            return DrainStep::Idle;
        }
        let front_since = *self.front_since.get_or_insert(now);
        match front {
            FrontState::Gone => DrainStep::Drop,
            FrontState::Ready => match self.last_write {
                Some(last) if now < last + RESUME_SPACING => {
                    DrainStep::WaitUntil(last + RESUME_SPACING)
                }
                _ => DrainStep::Write,
            },
            FrontState::Waiting if now.duration_since(front_since) >= RESUME_FRONT_PATIENCE => {
                DrainStep::Drop
            }
            FrontState::Waiting => DrainStep::WaitUntil(front_since + RESUME_FRONT_PATIENCE),
        }
    }

    pub(crate) fn pop(&mut self, wrote_at: Option<Instant>) {
        self.order.pop_front();
        self.front_since = None;
        if wrote_at.is_some() {
            self.last_write = wrote_at;
        }
    }

    pub(crate) fn enqueue(&mut self, surface: u64) {
        if !self.order.contains(&surface) {
            self.order.push_back(surface);
        }
    }

    fn claim_wake(&mut self, at: Instant) -> bool {
        if self.wake_at.is_some_and(|scheduled| scheduled <= at) {
            return false;
        }
        self.wake_at = Some(at);
        true
    }
}

fn hook_text<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params
        .get("hook_payload")
        .and_then(|hook| hook.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConversationEffect<'a> {
    Signal(AgentSessionSignal<'a>),
    Exit(i32),
    Transition,
    None,
}

pub(crate) fn conversation_effect<'a>(
    method: &str,
    params: &'a Value,
    fallback_cwd: Option<&'a str>,
) -> ConversationEffect<'a> {
    let tool = params
        .get("tool")
        .and_then(Value::as_str)
        .and_then(crate::agent_launcher::TerminalAgent::from_binary);
    match method {
        METHOD_SESSION_START => match (tool, hook_text(params, "session_id")) {
            (Some(tool), Some(id)) => ConversationEffect::Signal(AgentSessionSignal::Started {
                runtime: tool.runtime().id,
                id,
                cwd: hook_text(params, "cwd").or(fallback_cwd),
            }),
            _ => ConversationEffect::None,
        },
        METHOD_SESSION_END => ConversationEffect::Signal(AgentSessionSignal::Ended {
            reason: hook_text(params, "reason"),
        }),
        METHOD_EXIT => params
            .get("exit_code")
            .and_then(Value::as_i64)
            .and_then(|code| i32::try_from(code).ok())
            .map_or(ConversationEffect::None, ConversationEffect::Exit),
        _ => ConversationEffect::Transition,
    }
}

impl PaneFlowApp {
    pub(crate) fn plan_conversation_restore(&mut self, cx: &mut Context<Self>) {
        let mut candidates = Vec::new();
        let mut views = HashMap::new();
        for workspace in &self.workspaces {
            for (pane_index, pane) in workspace.collect_panes().iter().enumerate() {
                for terminal in pane.read(cx).terminals() {
                    let view = terminal.read(cx);
                    let Some(recorded) = view.agent_session() else {
                        continue;
                    };
                    if !view.conversation_restore_pending() {
                        continue;
                    }
                    let surface = terminal.entity_id().as_u64();
                    candidates.push(RestoreCandidate {
                        surface,
                        runtime: recorded.runtime.clone(),
                        id: recorded.id.clone(),
                        cwd: recorded.cwd.clone(),
                        location: format!("pane {} of {}", pane_index + 1, workspace.title),
                    });
                    views.insert(surface, terminal.clone());
                }
            }
        }
        let plan = plan_restore_order(candidates);
        for (surface, location) in plan.duplicates {
            if let Some(view) = views.get(&surface) {
                view.update(cx, |view, cx| {
                    view.mark_conversation_duplicate(location, cx)
                });
            }
        }
        for surface in plan.shared_folder {
            if let Some(view) = views.get(&surface) {
                view.update(cx, |view, _cx| view.mark_conversation_shared_folder());
            }
        }
        for surface in plan.order {
            self.conversation_restore.enqueue(surface);
        }
    }

    pub(crate) fn conversation_ready(&mut self, surface: u64, cx: &mut Context<Self>) {
        self.conversation_restore.enqueue(surface);
        self.drain_conversation_restore(cx);
    }

    pub(crate) fn drain_conversation_restore(&mut self, cx: &mut Context<Self>) {
        loop {
            let now = Instant::now();
            let terminal = self
                .conversation_restore
                .front()
                .and_then(|surface| find_terminal_by_surface_id(&self.workspaces, surface, cx));
            let front = match terminal.as_ref().map(|terminal| terminal.read(cx)) {
                Some(view) if view.conversation_restore_ready() => FrontState::Ready,
                Some(view) if view.conversation_restore_pending() => FrontState::Waiting,
                _ => FrontState::Gone,
            };
            match self.conversation_restore.step(now, front) {
                DrainStep::Idle => return,
                DrainStep::Drop => self.conversation_restore.pop(None),
                DrainStep::WaitUntil(at) => {
                    self.schedule_conversation_drain(at, cx);
                    return;
                }
                DrainStep::Write => {
                    let Some(terminal) = terminal else {
                        self.conversation_restore.pop(None);
                        continue;
                    };
                    match self.live_conversation_owner(&terminal, cx) {
                        Some(location) => {
                            self.conversation_restore.pop(None);
                            terminal.update(cx, |view, cx| {
                                view.mark_conversation_live_owner(location, cx);
                            });
                        }
                        None => {
                            self.conversation_restore.pop(Some(now));
                            terminal.update(cx, |view, cx| view.write_conversation_resume(cx));
                        }
                    }
                }
            }
        }
    }

    fn schedule_conversation_drain(&mut self, at: Instant, cx: &mut Context<Self>) {
        if !self.conversation_restore.claim_wake(at) {
            return;
        }
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                smol::Timer::at(at).await;
                let _ = this.update(cx, |app, cx| {
                    if app.conversation_restore.wake_at == Some(at) {
                        app.conversation_restore.wake_at = None;
                    }
                    app.drain_conversation_restore(cx);
                });
            },
        )
        .detach();
    }

    fn live_conversation_owner(
        &self,
        terminal: &Entity<TerminalView>,
        cx: &gpui::App,
    ) -> Option<String> {
        let view = terminal.read(cx);
        let agent = view.conversation_agent()?;
        let recorded = view
            .agent_session()
            .filter(|recorded| !recorded.continues_latest())?;
        let owner = self.host_agents.live_session_with_provider_id(
            agent,
            &recorded.id,
            &view.terminal.session_id,
        )?;
        let location = self
            .workspaces
            .iter()
            .find_map(|workspace| {
                workspace
                    .collect_panes()
                    .iter()
                    .position(|pane| {
                        pane.read(cx)
                            .terminals()
                            .any(|terminal| terminal.read(cx).terminal.session_id == owner)
                    })
                    .map(|index| format!("pane {} of {}", index + 1, workspace.title))
            })
            .unwrap_or_else(|| "another pane".to_string());
        Some(location)
    }

    pub(crate) fn apply_conversation_signal(
        &mut self,
        method: &str,
        params: &Value,
        surface_id: u64,
        cx: &mut Context<Self>,
    ) {
        let Some(terminal) = find_terminal_by_surface_id(&self.workspaces, surface_id, cx) else {
            return;
        };
        terminal.update(cx, |view, cx| {
            let fallback_cwd = view.terminal.current_cwd.clone();
            match conversation_effect(method, params, fallback_cwd.as_deref()) {
                ConversationEffect::Signal(signal) => view.apply_agent_session_signal(signal, cx),
                ConversationEffect::Exit(code) => view.note_conversation_exit(code, cx),
                ConversationEffect::Transition => view.note_conversation_transition(),
                ConversationEffect::None => {}
            }
        });
    }

    pub(crate) fn conversation_returned_to_shell(
        &mut self,
        terminal: &Entity<TerminalView>,
        agent_reaped: bool,
        cx: &mut Context<Self>,
    ) {
        terminal.update(cx, |view, cx| {
            if agent_reaped || view.conversation_is_live() {
                view.apply_agent_session_signal(AgentSessionSignal::ReturnedToShell, cx);
            }
        });
    }

    pub(crate) fn fork_conversation_in(
        &mut self,
        pane: Entity<crate::pane::Pane>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(terminal) = pane.read(cx).active_terminal_opt().cloned() else {
            return;
        };
        let (agent, id) = {
            let view = terminal.read(cx);
            let (Some(agent), Some(recorded)) = (view.conversation_agent(), view.agent_session())
            else {
                return;
            };
            (agent, recorded.id.clone())
        };
        let config = self.cached_config.clone();
        let Some(command) = crate::agent_resume::conversation_command(
            agent,
            ConversationTemplate::Fork,
            &id,
            &config,
        ) else {
            return;
        };
        match self.split_with_target(
            pane,
            SplitDirection::Vertical,
            paneflow_config::schema::TerminalSurfaceProfile::Normal,
            SurfaceLaunch {
                command: Some(command),
                env: None,
            },
            window,
            cx,
        ) {
            Ok(forked) => forked.update(cx, |view, cx| view.watch_conversation_start(agent, cx)),
            Err(message) => self.show_toast(message, cx),
        }
    }

    pub(crate) fn handle_fork_conversation(
        &mut self,
        _: &crate::ForkConversation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self
            .active_workspace()
            .and_then(|workspace| workspace.active_tab().root.as_ref())
            .and_then(|root| root.focused_pane(window, cx))
        else {
            return;
        };
        self.fork_conversation_in(pane, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(surface: u64, id: &str, location: &str) -> RestoreCandidate {
        RestoreCandidate {
            surface,
            runtime: "com.anthropic.claude-code".to_string(),
            id: id.to_string(),
            cwd: Some("/repo".to_string()),
            location: location.to_string(),
        }
    }

    fn fx_candidate(surface: u64, cwd: &str) -> RestoreCandidate {
        RestoreCandidate {
            surface,
            runtime: "sh.fx.cli".to_string(),
            id: String::new(),
            cwd: Some(cwd.to_string()),
            location: format!("pane {surface} of main"),
        }
    }

    #[test]
    fn a_lone_fx_resumes_and_fx_panes_sharing_a_folder_all_wait_for_the_user() {
        let plan = plan_restore_order(vec![
            candidate(1, "a", "pane 1 of main"),
            fx_candidate(2, "/repo"),
            fx_candidate(3, "/other"),
            fx_candidate(4, "/repo"),
        ]);
        assert_eq!(plan.order, vec![1, 3]);
        assert_eq!(plan.shared_folder, vec![2, 4]);
        assert!(plan.duplicates.is_empty());
    }

    const CLAUDE_SESSION_START: &str = r#"{"session_id":"3922faec-860a-47b1-8f2d-e6b9488c467c","transcript_path":"C:\\Users\\Arthur\\.claude\\projects\\C--dev-paneflow\\3922faec-860a-47b1-8f2d-e6b9488c467c.jsonl","cwd":"C:\\dev\\paneflow","scratchpad_dir":"C:\\Users\\Arthur\\AppData\\Local\\Temp\\claude\\C--dev-paneflow\\3922faec-860a-47b1-8f2d-e6b9488c467c\\scratchpad","hook_event_name":"SessionStart","source":"startup","model":"claude-opus-5-5"}"#;

    fn compacted_session_start(tool: &str, raw: &Value) -> Value {
        let mut hook = serde_json::Map::new();
        hook.insert("hook_event_name".into(), Value::from("HookSeen"));
        for key in ["session_id", "cwd", "transcript_path"] {
            if let Some(value) = raw.get(key) {
                hook.insert(key.into(), value.clone());
            }
        }
        let frame = serde_json::json!({
            "type": "event",
            "kind": METHOD_SESSION_START,
            "tool": tool,
            "pid": 37484,
            "hook_payload": Value::Object(hook),
        });
        crate::app::host_agents::legacy_ai_params(&frame, 1, 7).expect("legacy params")
    }

    #[test]
    fn a_captured_claude_session_start_records_its_conversation_and_cwd() {
        let raw: Value = serde_json::from_str(CLAUDE_SESSION_START).expect("captured payload");
        let params = compacted_session_start("claude", &raw);
        assert_eq!(
            conversation_effect(METHOD_SESSION_START, &params, Some("/elsewhere")),
            ConversationEffect::Signal(AgentSessionSignal::Started {
                runtime: "com.anthropic.claude-code",
                id: "3922faec-860a-47b1-8f2d-e6b9488c467c",
                cwd: Some("C:\\dev\\paneflow"),
            })
        );
    }

    #[test]
    fn a_codex_session_start_without_cwd_falls_back_to_the_pane_cwd() {
        let raw = serde_json::json!({ "session_id": "01a0f952-1fa0-7e91-a35c-899b9dbfe97e" });
        let params = compacted_session_start("codex", &raw);
        assert_eq!(
            conversation_effect(METHOD_SESSION_START, &params, Some("/repo")),
            ConversationEffect::Signal(AgentSessionSignal::Started {
                runtime: "com.openai.codex",
                id: "01a0f952-1fa0-7e91-a35c-899b9dbfe97e",
                cwd: Some("/repo"),
            })
        );
    }

    #[test]
    fn a_shim_session_start_without_an_id_is_not_a_conversation() {
        let params = compacted_session_start("claude", &serde_json::json!({}));
        assert_eq!(
            conversation_effect(METHOD_SESSION_START, &params, None),
            ConversationEffect::None
        );
    }

    #[test]
    fn session_end_exit_and_turn_frames_map_to_their_effects() {
        let end = serde_json::json!({ "tool": "claude", "hook_payload": { "reason": "prompt_input_exit" } });
        assert_eq!(
            conversation_effect(METHOD_SESSION_END, &end, None),
            ConversationEffect::Signal(AgentSessionSignal::Ended {
                reason: Some("prompt_input_exit")
            })
        );
        let exit = serde_json::json!({ "tool": "codex", "exit_code": 1, "hook_payload": {} });
        assert_eq!(
            conversation_effect(METHOD_EXIT, &exit, None),
            ConversationEffect::Exit(1)
        );
        let prompt = serde_json::json!({ "tool": "codex", "hook_payload": {} });
        assert_eq!(
            conversation_effect("ai.prompt_submit", &prompt, None),
            ConversationEffect::Transition
        );
    }

    #[test]
    fn only_the_first_surface_of_a_conversation_resumes() {
        let plan = plan_restore_order(vec![
            candidate(1, "a", "pane 1 of main"),
            candidate(2, "b", "pane 2 of main"),
            candidate(3, "a", "pane 3 of main"),
            candidate(4, "a", "pane 1 of docs"),
        ]);
        assert_eq!(plan.order, vec![1, 2]);
        assert_eq!(
            plan.duplicates,
            vec![
                (3, "pane 1 of main".to_string()),
                (4, "pane 1 of main".to_string())
            ]
        );
    }

    #[test]
    fn resumes_are_written_in_order_at_least_250_ms_apart() {
        let start = Instant::now();
        let mut queue = ConversationRestoreQueue::default();
        queue.enqueue(1);
        queue.enqueue(2);
        assert_eq!(queue.step(start, FrontState::Ready), DrainStep::Write);
        queue.pop(Some(start));
        assert_eq!(queue.front(), Some(2));
        let early = start + Duration::from_millis(100);
        assert_eq!(
            queue.step(early, FrontState::Ready),
            DrainStep::WaitUntil(start + RESUME_SPACING)
        );
        assert_eq!(
            queue.step(start + RESUME_SPACING, FrontState::Ready),
            DrainStep::Write
        );
    }

    #[test]
    fn a_pane_that_never_settles_stops_holding_the_queue() {
        let start = Instant::now();
        let mut queue = ConversationRestoreQueue::default();
        queue.enqueue(7);
        queue.enqueue(8);
        assert_eq!(
            queue.step(start, FrontState::Waiting),
            DrainStep::WaitUntil(start + RESUME_FRONT_PATIENCE)
        );
        assert_eq!(
            queue.step(start + RESUME_FRONT_PATIENCE, FrontState::Waiting),
            DrainStep::Drop
        );
        queue.pop(None);
        assert_eq!(queue.front(), Some(8));
        queue.enqueue(8);
        queue.enqueue(7);
        assert_eq!(queue.order, VecDeque::from([8, 7]));
    }

    #[test]
    fn a_closed_pane_is_dropped_without_a_write() {
        let mut queue = ConversationRestoreQueue::default();
        queue.enqueue(3);
        assert_eq!(
            queue.step(Instant::now(), FrontState::Gone),
            DrainStep::Drop
        );
        queue.pop(None);
        assert_eq!(
            queue.step(Instant::now(), FrontState::Ready),
            DrainStep::Idle
        );
        assert_eq!(queue.last_write, None);
    }
}
