use crate::agent_launcher::TerminalAgent;
use std::collections::{HashMap, HashSet};

pub use paneflow_ipc_client::agent::{
    AgentLifecycleEvent, AgentState, AgentStateSource, FieldUpdate, SessionTransition,
    accepts_event, accepts_source, next_waiting_since, reduce_lifecycle_event,
};

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
    pub auto_naming: crate::auto_naming::SessionNaming,
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
            auto_naming: crate::auto_naming::SessionNaming::default(),
        }
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
                    agg.dominant = s.state;
                    agg.active_tool_name = s.active_tool_name.clone();
                }
            })
            .or_insert_with(|| ToolAggregate {
                tool: s.tool,
                dominant: s.state,
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

    #[test]
    fn the_shared_reducer_is_the_one_the_host_owns() {
        assert_eq!(
            reduce_lifecycle_event(AgentLifecycleEvent::PromptSubmit),
            paneflow_ipc_client::agent::reduce_lifecycle_event(AgentLifecycleEvent::PromptSubmit)
        );
        assert_eq!(AgentState::Thinking.wire_str(), "thinking");
    }

    fn s(tool: TerminalAgent, state: AgentState) -> AgentSession {
        AgentSession::new(tool, state)
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
