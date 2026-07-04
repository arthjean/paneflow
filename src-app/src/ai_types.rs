//! AI tool type definitions shared across the app.
//!
//! The tool identity is [`crate::agent_launcher::TerminalAgent`] - the same
//! 16-agent taxonomy as the terminal launchers (single source of truth:
//! binaries are the wire ids, `display_name`/`accent`/`display_rank` come
//! for free). The historical 2-variant `AiTool` enum was folded into it
//! when hook support grew past Claude Code + Codex; on the wire, `tool` is
//! the agent's binary name (`claude`, `codex`, `gemini`, …) resolved via
//! [`TerminalAgent::from_binary`], and an UNKNOWN string is now rejected
//! instead of silently retyped as Claude.
//!
//! `AgentState` tracks the lifecycle state of a single agent session.
//! `AgentSession` bundles tool + state + the currently-active sub-tool name
//! (`Edit`, `Bash`, …) for one PID. A workspace can hold many sessions
//! concurrently - keyed by PID in `Workspace::agent_sessions`.
//!
//! State transitions are driven by IPC hooks from the `paneflow-ai-hook`
//! binary. Each lifecycle frame carries the emitting process's PID so the
//! server can route updates to the exact session rather than collapsing
//! everything per tool name (which broke when two Claude Codes ran in the
//! same workspace - the second `ai.session_start` used to overwrite the
//! first PID in a `HashMap<String, u32>`).

use crate::agent_launcher::TerminalAgent;
use std::collections::{HashMap, HashSet};

/// Lifecycle state for one agent session (one PID).
///
/// `Inactive` is implicit (a session that's not in the map is inactive),
/// so the enum carries only the "visible" states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentState {
    /// Agent is processing a prompt or using tools.
    Thinking,
    /// Agent needs user input or approval (permission prompt, elicitation).
    WaitingForInput,
    /// Agent finished its response. Auto-cleared after 5 s by the IPC
    /// `ai.stop` handler unless overridden by a new state transition.
    Finished,
    /// EP-004 US-010 (cli-cockpit): the agent BINARY exited non-zero -
    /// reported by the shim's `ai.exit` frame (the shell's `ChildExit`
    /// only carries the shell's exit, never the agent's). Sticky until a
    /// new lifecycle event replaces it or its pane closes; never produced
    /// by a human interrupt (see [`state_for_exit`]).
    Errored,
    /// EP-004 US-011 (cli-cockpit): a `Thinking` session with no hook
    /// activity past the configured silence threshold. Flipped by the
    /// periodic sweep; any subsequent hook event replaces it immediately
    /// (never sticky).
    Stalled,
}

impl AgentState {
    /// Stable wire string for IPC (`fleet.list` / `surface.status`,
    /// prd-agent-control-plane EP-001). These are machine ids a conductor
    /// matches on, distinct from `display_name` - never shown to a human, never
    /// localised.
    pub fn wire_str(&self) -> &'static str {
        match self {
            AgentState::Thinking => "thinking",
            AgentState::WaitingForInput => "waiting_for_input",
            AgentState::Finished => "finished",
            AgentState::Errored => "errored",
            AgentState::Stalled => "stalled",
        }
    }

    /// EP-004 US-013 (agent-control-plane): the watchdog rule. A session is
    /// considered stalled (a likely-lost `ai.stop`, shim killed while the shell
    /// lives) when it is still `Thinking` and its last hook activity is older
    /// than `threshold`. Only `Thinking` qualifies, so the flip is once-per-
    /// episode and non-sticky: any later hook routes through
    /// `upsert_session_state`, which overwrites the state AND resets the idle
    /// clock, so a `Stalled` (or `WaitingForInput`/`Finished`) session never
    /// re-flips here. Pure, so the rule is unit-tested without the GPUI sweep.
    pub fn stalls_after(&self, idle: std::time::Duration, threshold: std::time::Duration) -> bool {
        matches!(self, AgentState::Thinking) && idle >= threshold
    }
}

/// EP-004 US-010: classify the agent binary's raw exit code into the
/// session state it produces. Exit codes are reported by the shim with the
/// shell convention `128 + signum` for signal terminations (see
/// `paneflow-shim::exec::raw_exit_code_from_status`).
///
/// A termination *initiated from outside the agent* is not an agent
/// failure (FR-06: "une interruption humaine n'est PAS une erreur"):
/// - 130 (`128+SIGINT`) - Ctrl+C, the PRD-mandated case.
/// - 129 (`128+SIGHUP`) - pane/PTY closed under a running agent. Without
///   this exclusion every pane close with a live agent would flash a
///   false `Errored`.
/// - 143 (`128+SIGTERM`) / 137 (`128+SIGKILL`) - external kill.
/// - `STATUS_CONTROL_C_EXIT` (0xC000013A) - the Windows Ctrl+C exit code
///   (`code()` is always `Some` on Windows; there are no signals).
///
/// Genuine crash signals (SIGSEGV → 139, SIGABRT → 134, …) and every
/// other non-zero code classify as `Errored`.
pub fn state_for_exit(exit_code: i32) -> AgentState {
    /// `{Application Exit by CTRL+C}` - 0xC000013A as i32.
    const STATUS_CONTROL_C_EXIT: i32 = 0xC000_013Au32 as i32;
    match exit_code {
        0 => AgentState::Finished,
        129 | 130 | 137 | 143 => AgentState::Finished,
        STATUS_CONTROL_C_EXIT => AgentState::Finished,
        _ => AgentState::Errored,
    }
}

/// One row in the per-workspace `agent_sessions` map.
#[derive(Debug, Clone)]
pub struct AgentSession {
    pub tool: TerminalAgent,
    pub state: AgentState,
    /// Name of the active sub-tool (Edit, Bash, Read, …) reported by
    /// `ai.tool_use` hooks. Cleared on every non-Thinking transition.
    pub active_tool_name: Option<String>,
    /// The agent's question, from the `ai.notification` hook payload (≤512
    /// chars, UNTRUSTED terminal-adjacent text - display only, never
    /// interpreted). Set on `WaitingForInput`, cleared on `prompt_submit` /
    /// `stop` so a stale question never haunts the next turn (US-016).
    pub message: Option<String>,
    /// The surface (terminal entity id) this session runs in, resolved from
    /// the hook PID by walking the process ancestor chain to a known pane
    /// `child_pid` (US-017). `None` when unresolved - the session then only
    /// exists at workspace level (no per-pane glow), never a wrong pane.
    pub surface_id: Option<u64>,
    /// EP-002 US-004 (cli-cockpit): when this session ENTERED
    /// `WaitingForInput` - drives the Attention Queue's wait column and its
    /// longest-waiting-first order. Stamped by `upsert_session_state` via
    /// [`next_waiting_since`]; cleared on any non-waiting transition.
    /// `Instant` (monotonic) so a wall-clock jump never shows a negative or
    /// absurd wait.
    pub waiting_since: Option<std::time::Instant>,
    /// EP-004 US-011 (cli-cockpit): when the last `ai.*` lifecycle event
    /// for this session arrived. Stamped by `upsert_session_state` on every
    /// hook frame (prompt_submit / tool_use / notification / stop / exit);
    /// the periodic sweep flips a `Thinking` session to `Stalled` once this
    /// exceeds the configured silence threshold. Monotonic for the same
    /// reason as `waiting_since`.
    pub last_activity: std::time::Instant,
    /// OS start time of the session's process, pinned at session creation
    /// (Linux `/proc/{pid}/stat` field 22, macOS `pbi_start_tvsec`, Windows
    /// `GetProcessTimes` creation FILETIME - opaque, only compared for
    /// equality). Guards the sweep's `pid_is_alive` probe against PID reuse:
    /// a live PID whose start time changed belongs to a DIFFERENT process,
    /// so the session is dead. `None` (synthetic PID, probe failure) keeps
    /// the conservative liveness-only check.
    pub proc_start: Option<u64>,
    /// EP-004 US-015 (agent-control-plane): an optional summary of the agent's
    /// last completed turn, surfaced by `fleet.list` / `surface.status` so a
    /// conductor reads structured context instead of scraping the scrollback.
    /// Best-effort: populated on `ai.stop` from the stop hook payload when it
    /// carries a summary; `None` (the common case today) when the hook provides
    /// none. UNTRUSTED, display-only (same provenance as `message`).
    pub last_result: Option<String>,
}

impl AgentSession {
    pub fn new(tool: TerminalAgent, state: AgentState) -> Self {
        Self {
            tool,
            state,
            active_tool_name: None,
            message: None,
            surface_id: None,
            waiting_since: None,
            last_activity: std::time::Instant::now(),
            proc_start: None,
            last_result: None,
        }
    }
}

/// EP-002 US-004: next value of `waiting_since` for a state transition.
/// Stamped on ENTERING `WaitingForInput`; a re-notification while already
/// waiting keeps the original stamp so the queue shows the true wait;
/// any other state clears it. Pure - unit-tested.
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

/// Aggregate of a workspace's sessions for a single tool, used by the
/// sidebar render. Computed on-the-fly from `agent_sessions` - never
/// stored. The "dominant" state is the most user-salient one across all
/// sessions of the same tool: `WaitingForInput > Thinking > Finished`.
/// `count` is the total number of sessions for this tool in any visible
/// state (i.e., everything in the map for that tool); `extra` is
/// `count - 1`, the "+N" suffix shown after the lead label.
#[derive(Debug, Clone)]
pub struct ToolAggregate {
    pub tool: TerminalAgent,
    pub dominant: AgentState,
    pub count: usize,
    pub active_tool_name: Option<String>,
}

impl ToolAggregate {
    /// Render the `+N` suffix when more than one session of the same tool
    /// is active. Returns an empty string for a single session so the
    /// sidebar reads `Claude thinking…` (not `Claude thinking… +0`).
    pub fn extra_suffix(&self) -> String {
        if self.count > 1 {
            format!(" +{}", self.count - 1)
        } else {
            String::new()
        }
    }
}

/// Shared per-workspace agent-status projection for the CLI sidebar and
/// `fleet.list`.
///
/// `agent_sessions` is the hook-derived truth. `detected_agents` is the
/// process-scan fallback that tells us an agent is running even when no hook
/// lifecycle frames are available. Keeping the merge here prevents the UI and
/// IPC surfaces from drifting on what "hooked" vs "running without hook" means.
#[derive(Debug, Clone)]
pub struct WorkspaceAgentStatus {
    /// One row per hook-backed tool, collapsed from per-PID sessions.
    pub hooked: Vec<ToolAggregate>,
    /// Known agent tools detected in the process tree but absent from hooks.
    pub unhooked: Vec<TerminalAgent>,
    /// Human labels for the title-dot tooltip. Unknown strings are preserved so
    /// a future detector never collapses to a vague "AI" label.
    pub active_labels: Vec<String>,
}

/// Build the shared workspace agent-status projection.
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

/// Salience ranking used to pick the dominant state when a tool has
/// multiple sessions in different states. `Errored` outranks everything
/// (a crash must never hide behind a sibling's spinner); `Stalled` sits
/// between `WaitingForInput` (actionable now) and `Thinking` (nominal).
fn state_rank(s: &AgentState) -> u8 {
    match s {
        AgentState::Errored => 5,
        AgentState::WaitingForInput => 4,
        AgentState::Stalled => 3,
        AgentState::Thinking => 2,
        AgentState::Finished => 1,
    }
}

/// Aggregate the per-PID sessions of a workspace into one row per
/// `TerminalAgent`, sorted by `TerminalAgent::display_rank`.
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
    fn stalls_after_only_thinking_past_threshold() {
        // EP-004 US-013 / US-014: the watchdog rule.
        use std::time::Duration;
        let threshold = Duration::from_secs(60);
        // AC1: a Thinking session idle past the threshold stalls (boundary is
        // inclusive: elapsed >= threshold).
        assert!(AgentState::Thinking.stalls_after(Duration::from_secs(61), threshold));
        assert!(AgentState::Thinking.stalls_after(Duration::from_secs(60), threshold));
        // AC2: fresh hook activity (idle below threshold) does not stall.
        assert!(!AgentState::Thinking.stalls_after(Duration::from_secs(59), threshold));
        // AC4 + structural dedup: a non-Thinking session never stalls, however
        // idle - so an already-Stalled row cannot re-trigger, and a waiting or
        // finished agent is never mislabelled.
        assert!(!AgentState::Stalled.stalls_after(Duration::from_secs(600), threshold));
        assert!(!AgentState::WaitingForInput.stalls_after(Duration::from_secs(600), threshold));
        assert!(!AgentState::Finished.stalls_after(Duration::from_secs(600), threshold));
        assert!(!AgentState::Errored.stalls_after(Duration::from_secs(600), threshold));
    }

    #[test]
    fn waiting_since_stamps_on_entering_waiting_only() {
        use AgentState::*;
        let now = std::time::Instant::now();
        // Fresh session entering WaitingForInput → stamped.
        assert_eq!(next_waiting_since(None, &WaitingForInput, now), Some(now));
        // Thinking → WaitingForInput → stamped.
        assert_eq!(
            next_waiting_since(Some((&Thinking, None)), &WaitingForInput, now),
            Some(now)
        );
        // Any non-waiting target clears.
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
        // A second ai.notification while already waiting keeps the ORIGINAL
        // stamp - the queue must show the true wait, not reset on every
        // notification frame.
        assert_eq!(
            next_waiting_since(
                Some((&WaitingForInput, Some(first))),
                &WaitingForInput,
                later
            ),
            Some(first)
        );
        // Waiting state but a missing stamp (legacy row) self-heals.
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
        // A waiting agent is actionable NOW; a stalled one is a suspicion.
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
        // FR-06: clean exit and human/external terminations are not errors.
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
        // Genuine failures.
        assert_eq!(state_for_exit(1), Errored);
        assert_eq!(state_for_exit(2), Errored);
        assert_eq!(state_for_exit(127), Errored, "command not found");
        assert_eq!(state_for_exit(139), Errored, "128+SIGSEGV is a crash");
        assert_eq!(state_for_exit(134), Errored, "128+SIGABRT is a crash");
        assert_eq!(state_for_exit(-1), Errored, "negative non-Ctrl+C code");
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
