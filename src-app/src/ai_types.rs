use crate::agent_launcher::TerminalAgent;
use paneflow_ipc_client::ai_hook::EVENT_REORDER_TOLERANCE_MS;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentState {
    Thinking,
    WaitingForInput,
    Finished,
    Errored,
    Stalled,
}

impl AgentState {
    pub fn wire_str(&self) -> &'static str {
        match self {
            AgentState::Thinking => "thinking",
            AgentState::WaitingForInput => "waiting_for_input",
            AgentState::Finished => "finished",
            AgentState::Errored => "errored",
            AgentState::Stalled => "stalled",
        }
    }

    pub fn stalls_after(&self, idle: std::time::Duration, threshold: std::time::Duration) -> bool {
        matches!(self, AgentState::Thinking) && idle >= threshold
    }
}

const STATUS_CONTROL_C_EXIT: i32 = 0xC000_013Au32 as i32;

pub fn is_human_interruption_exit(exit_code: i32) -> bool {
    matches!(exit_code, 129 | 130 | 137 | 143 | STATUS_CONTROL_C_EXIT)
}

pub fn state_for_exit(exit_code: i32) -> AgentState {
    match exit_code {
        0 => AgentState::Finished,
        code if is_human_interruption_exit(code) => AgentState::Finished,
        _ => AgentState::Errored,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AgentStateSource {
    Terminal,
    SessionRegistry,
    Hook,
}

pub const SOURCE_TAKEOVER_SILENCE: std::time::Duration = std::time::Duration::from_secs(20);

pub fn accepts_source(
    existing: Option<(AgentStateSource, std::time::Duration)>,
    incoming: AgentStateSource,
) -> bool {
    match existing {
        None => true,
        Some((held, silence)) => incoming >= held || silence >= SOURCE_TAKEOVER_SILENCE,
    }
}

#[derive(Debug, Clone)]
pub struct AgentSession {
    pub tool: TerminalAgent,
    pub state: AgentState,
    pub source: AgentStateSource,
    pub active_tool_name: Option<String>,
    pub message: Option<String>,
    pub surface_id: Option<u64>,
    pub waiting_since: Option<std::time::Instant>,
    pub last_activity: std::time::Instant,
    pub proc_start: Option<u64>,
    pub last_result: Option<String>,
    pub last_event_at_ms: Option<u64>,
    pub pending_tab_title: Option<String>,
}

impl AgentSession {
    pub fn new(tool: TerminalAgent, state: AgentState) -> Self {
        Self {
            tool,
            state,
            source: AgentStateSource::Hook,
            active_tool_name: None,
            message: None,
            surface_id: None,
            waiting_since: None,
            last_activity: std::time::Instant::now(),
            proc_start: None,
            last_result: None,
            last_event_at_ms: None,
            pending_tab_title: None,
        }
    }
}

pub fn accepts_event(last: Option<u64>, incoming: Option<u64>) -> bool {
    match (last, incoming) {
        (Some(last), Some(incoming)) if incoming < last => {
            last - incoming > EVENT_REORDER_TOLERANCE_MS
        }
        _ => true,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldUpdate<T> {
    Keep,
    Set(T),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentLifecycleEvent {
    PromptSubmit,
    ToolUse { tool_name: Option<String> },
    Notification { message: Option<String> },
    Stop { summary: Option<String> },
    Exit { exit_code: i32 },
    Working,
    Idle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTransition {
    pub state: AgentState,
    pub active_tool_name: Option<String>,
    pub message: FieldUpdate<Option<String>>,
    pub last_result: FieldUpdate<Option<String>>,
}

pub fn reduce_lifecycle_event(event: AgentLifecycleEvent) -> SessionTransition {
    match event {
        AgentLifecycleEvent::PromptSubmit => SessionTransition {
            state: AgentState::Thinking,
            active_tool_name: None,
            message: FieldUpdate::Set(None),
            last_result: FieldUpdate::Keep,
        },
        AgentLifecycleEvent::ToolUse { tool_name } => SessionTransition {
            state: AgentState::Thinking,
            active_tool_name: tool_name,
            message: FieldUpdate::Keep,
            last_result: FieldUpdate::Keep,
        },
        AgentLifecycleEvent::Notification { message } => SessionTransition {
            state: AgentState::WaitingForInput,
            active_tool_name: None,
            message: FieldUpdate::Set(message),
            last_result: FieldUpdate::Keep,
        },
        AgentLifecycleEvent::Stop { summary } => SessionTransition {
            state: AgentState::Finished,
            active_tool_name: None,
            message: FieldUpdate::Set(None),
            last_result: FieldUpdate::Set(summary),
        },
        AgentLifecycleEvent::Exit { exit_code } => SessionTransition {
            state: state_for_exit(exit_code),
            active_tool_name: None,
            message: FieldUpdate::Set(None),
            last_result: FieldUpdate::Keep,
        },
        AgentLifecycleEvent::Working => SessionTransition {
            state: AgentState::Thinking,
            active_tool_name: None,
            message: FieldUpdate::Set(None),
            last_result: FieldUpdate::Keep,
        },
        AgentLifecycleEvent::Idle => SessionTransition {
            state: AgentState::Finished,
            active_tool_name: None,
            message: FieldUpdate::Set(None),
            last_result: FieldUpdate::Keep,
        },
    }
}

pub fn next_waiting_since(
    prev: Option<(&AgentState, Option<std::time::Instant>)>,
    new_state: &AgentState,
    now: std::time::Instant,
) -> Option<std::time::Instant> {
    match new_state {
        AgentState::WaitingForInput => match prev {
            Some((AgentState::WaitingForInput, since @ Some(_))) => since,
            _ => Some(now),
        },
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct ToolAggregate {
    pub tool: TerminalAgent,
    pub dominant: AgentState,
    pub count: usize,
    pub active_tool_name: Option<String>,
}

impl ToolAggregate {
    pub fn extra_suffix(&self) -> String {
        if self.count > 1 {
            format!(" +{}", self.count - 1)
        } else {
            String::new()
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkspaceAgentStatus {
    pub hooked: Vec<ToolAggregate>,
    pub unhooked: Vec<TerminalAgent>,
    pub active_labels: Vec<String>,
}

pub fn workspace_agent_status<'a, I>(
    sessions: I,
    detected_agents: &HashSet<String>,
) -> WorkspaceAgentStatus
where
    I: IntoIterator<Item = &'a AgentSession>,
{
    let hooked = aggregate_by_tool(sessions);
    let hooked_tools: HashSet<TerminalAgent> = hooked.iter().map(|row| row.tool).collect();

    let mut detected_tools: Vec<TerminalAgent> = detected_agents
        .iter()
        .filter_map(|binary| TerminalAgent::from_binary(binary))
        .collect();
    detected_tools.sort_by_key(|tool| tool.display_rank());
    detected_tools.dedup();

    let mut active_labels: Vec<String> = hooked
        .iter()
        .map(|row| row.tool.display_name().to_string())
        .chain(detected_agents.iter().map(|binary| {
            TerminalAgent::from_binary(binary)
                .map(|tool| tool.display_name().to_string())
                .unwrap_or_else(|| binary.clone())
        }))
        .collect();
    active_labels.sort();
    active_labels.dedup();

    let unhooked = detected_tools
        .into_iter()
        .filter(|tool| !hooked_tools.contains(tool))
        .collect();

    WorkspaceAgentStatus {
        hooked,
        unhooked,
        active_labels,
    }
}

fn state_rank(s: &AgentState) -> u8 {
    match s {
        AgentState::Errored => 5,
        AgentState::WaitingForInput => 4,
        AgentState::Stalled => 3,
        AgentState::Thinking => 2,
        AgentState::Finished => 1,
    }
}

pub fn aggregate_by_tool<'a, I>(sessions: I) -> Vec<ToolAggregate>
where
    I: IntoIterator<Item = &'a AgentSession>,
{
    let mut by_tool: HashMap<TerminalAgent, ToolAggregate> = HashMap::new();

    for s in sessions {
        by_tool
            .entry(s.tool)
            .and_modify(|agg| {
                agg.count += 1;
                if state_rank(&s.state) > state_rank(&agg.dominant) {
                    agg.dominant = s.state.clone();
                    agg.active_tool_name = s.active_tool_name.clone();
                }
            })
            .or_insert_with(|| ToolAggregate {
                tool: s.tool,
                dominant: s.state.clone(),
                count: 1,
                active_tool_name: s.active_tool_name.clone(),
            });
    }

    let mut rows: Vec<ToolAggregate> = by_tool.into_values().collect();
    rows.sort_by_key(|a| a.tool.display_rank());
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(tool: TerminalAgent, state: AgentState) -> AgentSession {
        AgentSession::new(tool, state)
    }

    #[test]
    fn a_weaker_source_never_talks_over_a_live_stronger_one() {
        use std::time::Duration;
        let fresh = Duration::from_secs(1);

        assert!(accepts_source(None, AgentStateSource::Terminal));

        assert!(!accepts_source(
            Some((AgentStateSource::Hook, fresh)),
            AgentStateSource::Terminal
        ));
        assert!(!accepts_source(
            Some((AgentStateSource::Hook, fresh)),
            AgentStateSource::SessionRegistry
        ));
        assert!(!accepts_source(
            Some((AgentStateSource::SessionRegistry, fresh)),
            AgentStateSource::Terminal
        ));

        for held in [
            AgentStateSource::Terminal,
            AgentStateSource::SessionRegistry,
            AgentStateSource::Hook,
        ] {
            assert!(accepts_source(Some((held, fresh)), AgentStateSource::Hook));
            assert!(accepts_source(Some((held, fresh)), held));
        }
        assert!(accepts_source(
            Some((AgentStateSource::Terminal, fresh)),
            AgentStateSource::SessionRegistry
        ));
    }

    #[test]
    fn a_silent_source_hands_over_instead_of_freezing_the_session() {
        assert!(accepts_source(
            Some((AgentStateSource::Hook, SOURCE_TAKEOVER_SILENCE)),
            AgentStateSource::Terminal
        ));
        assert!(!accepts_source(
            Some((
                AgentStateSource::Hook,
                SOURCE_TAKEOVER_SILENCE - std::time::Duration::from_millis(1)
            )),
            AgentStateSource::Terminal
        ));
    }

    #[test]
    fn sourceless_observations_move_state_without_inventing_detail() {
        let working = reduce_lifecycle_event(AgentLifecycleEvent::Working);
        assert_eq!(working.state, AgentState::Thinking);
        assert_eq!(working.active_tool_name, None);
        assert_eq!(working.message, FieldUpdate::Set(None));
        assert_eq!(working.last_result, FieldUpdate::Keep);

        let idle = reduce_lifecycle_event(AgentLifecycleEvent::Idle);
        assert_eq!(idle.state, AgentState::Finished);
        assert_eq!(idle.message, FieldUpdate::Set(None));
        assert_eq!(idle.last_result, FieldUpdate::Keep);
    }

    #[test]
    fn out_of_order_frames_are_rejected_but_a_clock_jump_is_not() {
        assert!(accepts_event(None, Some(1_000)));
        assert!(accepts_event(Some(1_000), None));
        assert!(accepts_event(None, None));
        assert!(accepts_event(Some(1_000), Some(1_001)));
        assert!(accepts_event(Some(1_000), Some(1_000)));
        assert!(!accepts_event(Some(1_000), Some(999)));
        assert!(!accepts_event(
            Some(1_000_000),
            Some(1_000_000 - EVENT_REORDER_TOLERANCE_MS)
        ));
        assert!(accepts_event(
            Some(1_000_000),
            Some(1_000_000 - EVENT_REORDER_TOLERANCE_MS - 1)
        ));
    }

    #[test]
    fn lifecycle_events_reduce_to_their_session_state() {
        let prompt = reduce_lifecycle_event(AgentLifecycleEvent::PromptSubmit);
        assert_eq!(prompt.state, AgentState::Thinking);
        assert_eq!(prompt.message, FieldUpdate::Set(None));
        assert_eq!(prompt.last_result, FieldUpdate::Keep);

        let tool_use = reduce_lifecycle_event(AgentLifecycleEvent::ToolUse {
            tool_name: Some("Edit".into()),
        });
        assert_eq!(tool_use.state, AgentState::Thinking);
        assert_eq!(tool_use.active_tool_name.as_deref(), Some("Edit"));
        assert_eq!(tool_use.message, FieldUpdate::Keep);

        let notification = reduce_lifecycle_event(AgentLifecycleEvent::Notification {
            message: Some("Approve edit?".into()),
        });
        assert_eq!(notification.state, AgentState::WaitingForInput);
        assert_eq!(
            notification.message,
            FieldUpdate::Set(Some("Approve edit?".into()))
        );
        assert!(notification.active_tool_name.is_none());

        let stop = reduce_lifecycle_event(AgentLifecycleEvent::Stop {
            summary: Some("3 files changed".into()),
        });
        assert_eq!(stop.state, AgentState::Finished);
        assert_eq!(stop.message, FieldUpdate::Set(None));
        assert_eq!(
            stop.last_result,
            FieldUpdate::Set(Some("3 files changed".into()))
        );

        for code in [0, 130, 129, 143] {
            let exit = reduce_lifecycle_event(AgentLifecycleEvent::Exit { exit_code: code });
            assert_eq!(exit.state, AgentState::Finished, "exit code {code}");
            assert_eq!(exit.message, FieldUpdate::Set(None));
            assert_eq!(exit.last_result, FieldUpdate::Keep);
        }
        assert_eq!(
            reduce_lifecycle_event(AgentLifecycleEvent::Exit { exit_code: 139 }).state,
            AgentState::Errored
        );
    }

    #[test]
    fn stalls_after_only_thinking_past_threshold() {
        use std::time::Duration;
        let threshold = Duration::from_secs(60);
        assert!(AgentState::Thinking.stalls_after(Duration::from_secs(61), threshold));
        assert!(AgentState::Thinking.stalls_after(Duration::from_secs(60), threshold));
        assert!(!AgentState::Thinking.stalls_after(Duration::from_secs(59), threshold));
        assert!(!AgentState::Stalled.stalls_after(Duration::from_secs(600), threshold));
        assert!(!AgentState::WaitingForInput.stalls_after(Duration::from_secs(600), threshold));
        assert!(!AgentState::Finished.stalls_after(Duration::from_secs(600), threshold));
        assert!(!AgentState::Errored.stalls_after(Duration::from_secs(600), threshold));
    }

    #[test]
    fn waiting_since_stamps_on_entering_waiting_only() {
        use AgentState::*;
        let now = std::time::Instant::now();
        assert_eq!(next_waiting_since(None, &WaitingForInput, now), Some(now));
        assert_eq!(
            next_waiting_since(Some((&Thinking, None)), &WaitingForInput, now),
            Some(now)
        );
        assert_eq!(
            next_waiting_since(Some((&WaitingForInput, Some(now))), &Thinking, now),
            None
        );
        assert_eq!(
            next_waiting_since(Some((&WaitingForInput, Some(now))), &Finished, now),
            None
        );
    }

    #[test]
    fn waiting_since_survives_renotification() {
        use AgentState::*;
        let first = std::time::Instant::now();
        let later = first + std::time::Duration::from_secs(90);
        assert_eq!(
            next_waiting_since(
                Some((&WaitingForInput, Some(first))),
                &WaitingForInput,
                later
            ),
            Some(first)
        );
        assert_eq!(
            next_waiting_since(Some((&WaitingForInput, None)), &WaitingForInput, later),
            Some(later)
        );
    }

    #[test]
    fn wire_str_is_stable_for_every_state() {
        use AgentState::*;
        assert_eq!(Thinking.wire_str(), "thinking");
        assert_eq!(WaitingForInput.wire_str(), "waiting_for_input");
        assert_eq!(Finished.wire_str(), "finished");
        assert_eq!(Errored.wire_str(), "errored");
        assert_eq!(Stalled.wire_str(), "stalled");
    }

    #[test]
    fn aggregate_empty_yields_no_rows() {
        let rows = aggregate_by_tool(std::iter::empty());
        assert!(rows.is_empty());
    }

    #[test]
    fn single_session_no_suffix() {
        let sessions = [s(TerminalAgent::ClaudeCode, AgentState::Thinking)];
        let rows = aggregate_by_tool(sessions.iter());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].count, 1);
        assert_eq!(rows[0].extra_suffix(), "");
    }

    #[test]
    fn multi_same_tool_yields_plus_n_suffix() {
        let sessions = [
            s(TerminalAgent::ClaudeCode, AgentState::Thinking),
            s(TerminalAgent::ClaudeCode, AgentState::Thinking),
            s(TerminalAgent::ClaudeCode, AgentState::Thinking),
        ];
        let rows = aggregate_by_tool(sessions.iter());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].count, 3);
        assert_eq!(rows[0].extra_suffix(), " +2");
    }

    #[test]
    fn dominant_picks_waiting_over_thinking() {
        let sessions = [
            s(TerminalAgent::ClaudeCode, AgentState::Thinking),
            s(TerminalAgent::ClaudeCode, AgentState::WaitingForInput),
            s(TerminalAgent::ClaudeCode, AgentState::Finished),
        ];
        let rows = aggregate_by_tool(sessions.iter());
        assert_eq!(rows[0].dominant, AgentState::WaitingForInput);
    }

    #[test]
    fn dominant_picks_thinking_over_finished() {
        let sessions = [
            s(TerminalAgent::ClaudeCode, AgentState::Finished),
            s(TerminalAgent::ClaudeCode, AgentState::Thinking),
        ];
        let rows = aggregate_by_tool(sessions.iter());
        assert_eq!(rows[0].dominant, AgentState::Thinking);
    }

    #[test]
    fn dominant_picks_errored_over_everything() {
        let sessions = [
            s(TerminalAgent::ClaudeCode, AgentState::Thinking),
            s(TerminalAgent::ClaudeCode, AgentState::WaitingForInput),
            s(TerminalAgent::ClaudeCode, AgentState::Errored),
        ];
        let rows = aggregate_by_tool(sessions.iter());
        assert_eq!(rows[0].dominant, AgentState::Errored);
    }

    #[test]
    fn dominant_picks_waiting_over_stalled() {
        let sessions = [
            s(TerminalAgent::ClaudeCode, AgentState::Stalled),
            s(TerminalAgent::ClaudeCode, AgentState::WaitingForInput),
        ];
        let rows = aggregate_by_tool(sessions.iter());
        assert_eq!(rows[0].dominant, AgentState::WaitingForInput);
    }

    #[test]
    fn dominant_picks_stalled_over_thinking() {
        let sessions = [
            s(TerminalAgent::ClaudeCode, AgentState::Thinking),
            s(TerminalAgent::ClaudeCode, AgentState::Stalled),
        ];
        let rows = aggregate_by_tool(sessions.iter());
        assert_eq!(rows[0].dominant, AgentState::Stalled);
    }

    #[test]
    fn exit_zero_and_interrupts_finish_everything_else_errors() {
        use AgentState::*;
        assert_eq!(state_for_exit(0), Finished);
        assert_eq!(state_for_exit(130), Finished, "128+SIGINT (Ctrl+C)");
        assert_eq!(state_for_exit(129), Finished, "128+SIGHUP (pane closed)");
        assert_eq!(state_for_exit(143), Finished, "128+SIGTERM");
        assert_eq!(state_for_exit(137), Finished, "128+SIGKILL");
        assert_eq!(
            state_for_exit(0xC000_013Au32 as i32),
            Finished,
            "Windows STATUS_CONTROL_C_EXIT"
        );
        assert_eq!(state_for_exit(1), Errored);
        assert_eq!(state_for_exit(2), Errored);
        assert_eq!(state_for_exit(127), Errored, "command not found");
        assert_eq!(state_for_exit(139), Errored, "128+SIGSEGV is a crash");
        assert_eq!(state_for_exit(134), Errored, "128+SIGABRT is a crash");
        assert_eq!(state_for_exit(-1), Errored, "negative non-Ctrl+C code");
    }

    #[test]
    fn human_interruption_exit_excludes_clean_exit_and_crashes() {
        assert!(!is_human_interruption_exit(0));
        assert!(is_human_interruption_exit(130));
        assert!(is_human_interruption_exit(0xC000_013Au32 as i32));
        assert!(!is_human_interruption_exit(1));
        assert!(!is_human_interruption_exit(139));
    }

    #[test]
    fn claude_renders_before_codex() {
        let sessions = [
            s(TerminalAgent::Codex, AgentState::Thinking),
            s(TerminalAgent::ClaudeCode, AgentState::Thinking),
        ];
        let rows = aggregate_by_tool(sessions.iter());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].tool, TerminalAgent::ClaudeCode);
        assert_eq!(rows[1].tool, TerminalAgent::Codex);
    }

    #[test]
    fn workspace_agent_status_splits_hooked_from_unhooked() {
        let sessions = [s(TerminalAgent::ClaudeCode, AgentState::Thinking)];
        let mut detected = HashSet::new();
        detected.insert(TerminalAgent::ClaudeCode.binary().to_string());
        detected.insert(TerminalAgent::Copilot.binary().to_string());

        let status = workspace_agent_status(sessions.iter(), &detected);

        assert_eq!(status.hooked.len(), 1);
        assert_eq!(status.hooked[0].tool, TerminalAgent::ClaudeCode);
        assert_eq!(status.unhooked, vec![TerminalAgent::Copilot]);
        assert_eq!(
            status.active_labels,
            vec!["Claude Code".to_string(), "Copilot".to_string()]
        );
    }

    #[test]
    fn workspace_agent_status_keeps_hook_only_label_active() {
        let sessions = [s(TerminalAgent::ClaudeCode, AgentState::Thinking)];
        let detected = HashSet::new();

        let status = workspace_agent_status(sessions.iter(), &detected);

        assert_eq!(status.hooked.len(), 1);
        assert!(status.unhooked.is_empty());
        assert_eq!(status.active_labels, vec!["Claude Code".to_string()]);
    }

    #[test]
    fn workspace_agent_status_preserves_unknown_detection_labels() {
        let sessions: [AgentSession; 0] = [];
        let mut detected = HashSet::new();
        detected.insert("future-agent".to_string());

        let status = workspace_agent_status(sessions.iter(), &detected);

        assert!(status.hooked.is_empty());
        assert!(status.unhooked.is_empty());
        assert_eq!(status.active_labels, vec!["future-agent".to_string()]);
    }
}
