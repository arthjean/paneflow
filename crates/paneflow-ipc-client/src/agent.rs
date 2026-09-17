use std::time::Duration;

use crate::ai_hook::EVENT_REORDER_TOLERANCE_MS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    Thinking,
    WaitingForInput,
    Finished,
    Errored,
}

impl AgentState {
    pub fn wire_str(&self) -> &'static str {
        match self {
            AgentState::Thinking => "thinking",
            AgentState::WaitingForInput => "waiting_for_input",
            AgentState::Finished => "finished",
            AgentState::Errored => "errored",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "thinking" => Some(AgentState::Thinking),
            "waiting_for_input" => Some(AgentState::WaitingForInput),
            "finished" => Some(AgentState::Finished),
            "errored" => Some(AgentState::Errored),
            _ => None,
        }
    }

    pub fn is_busy(&self) -> bool {
        matches!(self, AgentState::Thinking | AgentState::WaitingForInput)
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

impl AgentStateSource {
    pub fn wire_str(&self) -> &'static str {
        match self {
            AgentStateSource::Terminal => "terminal",
            AgentStateSource::SessionRegistry => "session_registry",
            AgentStateSource::Hook => "hook",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "terminal" => Some(AgentStateSource::Terminal),
            "session_registry" => Some(AgentStateSource::SessionRegistry),
            "hook" => Some(AgentStateSource::Hook),
            _ => None,
        }
    }
}

pub const SOURCE_TAKEOVER_SILENCE: Duration = Duration::from_secs(20);

pub fn accepts_source(
    existing: Option<(AgentStateSource, Duration)>,
    incoming: AgentStateSource,
) -> bool {
    match existing {
        None => true,
        Some((held, silence)) => incoming >= held || silence >= SOURCE_TAKEOVER_SILENCE,
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

pub fn next_waiting_since<T: Copy>(
    prev: Option<(&AgentState, Option<T>)>,
    new_state: &AgentState,
    now: T,
) -> Option<T> {
    match new_state {
        AgentState::WaitingForInput => match prev {
            Some((AgentState::WaitingForInput, since @ Some(_))) => since,
            _ => Some(now),
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_weaker_source_never_talks_over_a_live_stronger_one() {
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
                SOURCE_TAKEOVER_SILENCE - Duration::from_millis(1)
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
    fn waiting_since_stamps_on_entering_waiting_only() {
        use AgentState::*;
        let now = 1_000u64;
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
        let first = 1_000u64;
        let later = first + 90_000;
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
    fn wire_str_is_stable_for_every_state_and_source() {
        use AgentState::*;
        assert_eq!(Thinking.wire_str(), "thinking");
        assert_eq!(WaitingForInput.wire_str(), "waiting_for_input");
        assert_eq!(Finished.wire_str(), "finished");
        assert_eq!(Errored.wire_str(), "errored");
        for state in [Thinking, WaitingForInput, Finished, Errored] {
            assert_eq!(AgentState::parse(state.wire_str()), Some(state));
        }
        assert_eq!(AgentState::parse("idle"), None);
        for source in [
            AgentStateSource::Terminal,
            AgentStateSource::SessionRegistry,
            AgentStateSource::Hook,
        ] {
            assert_eq!(AgentStateSource::parse(source.wire_str()), Some(source));
        }
        assert_eq!(AgentStateSource::parse("shim"), None);
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
    fn only_busy_states_hold_a_live_indicator() {
        assert!(AgentState::Thinking.is_busy());
        assert!(AgentState::WaitingForInput.is_busy());
        assert!(!AgentState::Finished.is_busy());
        assert!(!AgentState::Errored.is_busy());
    }
}
