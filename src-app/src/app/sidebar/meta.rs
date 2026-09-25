use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SidebarAgentState {
    NeedsInput,
    Errored,
    Finished,
    Thinking,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SidebarAgentSummary {
    pub(super) state: SidebarAgentState,
    pub(super) count: usize,
    pub(super) tint: Option<u32>,
}

impl SidebarAgentSummary {
    fn tooltip_state(self) -> String {
        match self.state {
            SidebarAgentState::NeedsInput => {
                agent_status_sentence(self.count, "needs input", "need input")
            }
            SidebarAgentState::Errored => agent_status_sentence(self.count, "errored", "errored"),
            SidebarAgentState::Thinking => {
                agent_status_sentence(self.count, "thinking", "thinking")
            }
            SidebarAgentState::Finished => {
                "Agent finished. Click the workspace or tab to dismiss.".to_string()
            }
        }
    }
}

fn agent_status_sentence(count: usize, singular_state: &str, plural_state: &str) -> String {
    if count == 1 {
        format!("1 agent {singular_state}")
    } else {
        format!("{count} agents {plural_state}")
    }
}

pub(super) fn sidebar_agent_summary<'a, I>(
    sessions: I,
    completion_unread: usize,
) -> Option<SidebarAgentSummary>
where
    I: IntoIterator<Item = &'a ai_types::AgentSession>,
{
    let mut counts = [0usize; 3];
    for session in sessions {
        let index = match session.state {
            ai_types::AgentState::WaitingForInput => 0,
            ai_types::AgentState::Errored => 1,
            ai_types::AgentState::Thinking => 2,
            ai_types::AgentState::Finished => continue,
        };
        counts[index] += 1;
    }

    let priority = [SidebarAgentState::NeedsInput, SidebarAgentState::Errored];
    for (state, count) in priority.into_iter().zip(counts[..2].iter().copied()) {
        if count > 0 {
            return Some(SidebarAgentSummary {
                state,
                count,
                tint: None,
            });
        }
    }

    if completion_unread > 0 {
        return Some(SidebarAgentSummary {
            state: SidebarAgentState::Finished,
            count: completion_unread,
            tint: None,
        });
    }

    (counts[2] > 0).then_some(SidebarAgentSummary {
        state: SidebarAgentState::Thinking,
        count: counts[2],
        tint: None,
    })
}

pub(super) fn folder_row_sessions<'a, I>(
    sessions: I,
    expanded: bool,
) -> impl Iterator<Item = &'a ai_types::AgentSession>
where
    I: IntoIterator<Item = &'a ai_types::AgentSession>,
    I::IntoIter: 'a,
{
    sessions
        .into_iter()
        .filter(move |session| !expanded || session.surface_id.is_none())
}

pub(super) fn tab_row_sessions<'a, I>(
    sessions: I,
    surfaces: &'a std::collections::HashSet<u64>,
) -> impl Iterator<Item = &'a ai_types::AgentSession>
where
    I: IntoIterator<Item = &'a ai_types::AgentSession>,
    I::IntoIter: 'a,
{
    sessions
        .into_iter()
        .filter(move |session| session.surface_id.is_some_and(|id| surfaces.contains(&id)))
}

pub(super) fn sidebar_agent_status_tooltip(
    summary: SidebarAgentSummary,
    status: &ai_types::WorkspaceAgentStatus,
) -> SharedString {
    let state = summary.tooltip_state();
    if summary.state == SidebarAgentState::Finished {
        return state.into();
    }

    let mut details: Vec<String> = status
        .hooked
        .iter()
        .map(|aggregate| {
            format!(
                "{}{}",
                aggregate.tool.display_name(),
                aggregate.extra_suffix()
            )
        })
        .chain(
            status
                .unhooked
                .iter()
                .map(|tool| format!("{} running", tool.display_name())),
        )
        .collect();
    for label in &status.active_labels {
        if !details.iter().any(|detail| detail.starts_with(label)) {
            details.push(label.clone());
        }
    }

    if details.is_empty() {
        state.into()
    } else {
        format!("{state} · {}", details.join(", ")).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_launcher::TerminalAgent;
    use crate::ai_types::{AgentSession, AgentState};

    use std::collections::HashSet;

    fn session(state: AgentState) -> AgentSession {
        AgentSession::new(TerminalAgent::ClaudeCode, state)
    }

    #[test]
    fn sidebar_agent_summary_hides_idle_without_signal() {
        assert_eq!(sidebar_agent_summary(std::iter::empty(), 0), None);
    }

    #[test]
    fn sidebar_agent_summary_counts_winning_needs_input_sessions() {
        let sessions = [
            session(AgentState::WaitingForInput),
            session(AgentState::Errored),
            session(AgentState::WaitingForInput),
        ];
        assert_eq!(
            sidebar_agent_summary(sessions.iter(), 0),
            Some(SidebarAgentSummary {
                state: SidebarAgentState::NeedsInput,
                count: 2,
                tint: None,
            })
        );
    }

    #[test]
    fn sidebar_agent_summary_applies_sidebar_priority() {
        let cases = [
            (
                vec![AgentState::Finished, AgentState::Thinking],
                SidebarAgentState::Thinking,
            ),
            (
                vec![AgentState::Thinking, AgentState::Errored],
                SidebarAgentState::Errored,
            ),
            (
                vec![AgentState::Errored, AgentState::WaitingForInput],
                SidebarAgentState::NeedsInput,
            ),
        ];
        for (states, expected) in cases {
            let sessions: Vec<_> = states.into_iter().map(session).collect();
            assert_eq!(
                sidebar_agent_summary(sessions.iter(), 0).map(|summary| summary.state),
                Some(expected)
            );
        }
    }

    #[test]
    fn sidebar_agent_summary_surfaces_unread_completion_without_live_session() {
        assert_eq!(
            sidebar_agent_summary(std::iter::empty(), 1),
            Some(SidebarAgentSummary {
                state: SidebarAgentState::Finished,
                count: 1,
                tint: None,
            })
        );
    }

    #[test]
    fn sidebar_agent_summary_hides_acknowledged_finished_session() {
        let sessions = [session(AgentState::Finished)];
        assert_eq!(sidebar_agent_summary(sessions.iter(), 0), None);
    }

    #[test]
    fn the_finished_count_is_the_number_of_unread_surfaces() {
        assert_eq!(
            sidebar_agent_summary(std::iter::empty(), 3),
            Some(SidebarAgentSummary {
                state: SidebarAgentState::Finished,
                count: 3,
                tint: None,
            })
        );
    }

    fn attributed_sessions() -> [AgentSession; 3] {
        let mut mine = session(AgentState::WaitingForInput);
        mine.surface_id = Some(11);
        let mut other_tab = session(AgentState::Errored);
        other_tab.surface_id = Some(22);
        let unattributed = session(AgentState::Thinking);
        [mine, other_tab, unattributed]
    }

    #[test]
    fn tab_row_speaks_only_for_the_sessions_of_its_own_surfaces() {
        let sessions = attributed_sessions();
        let surfaces = HashSet::from([11u64]);
        assert_eq!(
            sidebar_agent_summary(tab_row_sessions(sessions.iter(), &surfaces), 0),
            Some(SidebarAgentSummary {
                state: SidebarAgentState::NeedsInput,
                count: 1,
                tint: None,
            }),
            "a tab must not inherit a sibling tab's session, nor an unattributed one"
        );

        assert_eq!(
            sidebar_agent_summary(tab_row_sessions(sessions.iter(), &HashSet::new()), 0),
            None
        );
    }

    #[test]
    fn expanded_folder_keeps_only_the_unattributed_sessions() {
        let sessions = attributed_sessions();
        assert_eq!(
            sidebar_agent_summary(folder_row_sessions(sessions.iter(), true), 0),
            Some(SidebarAgentSummary {
                state: SidebarAgentState::Thinking,
                count: 1,
                tint: None,
            })
        );

        let resolved = [attributed_sessions()[0].clone()];
        assert_eq!(
            sidebar_agent_summary(folder_row_sessions(resolved.iter(), true), 0),
            None
        );
    }

    #[test]
    fn collapsed_folder_re_aggregates_every_tab() {
        let sessions = attributed_sessions();
        assert_eq!(
            sidebar_agent_summary(folder_row_sessions(sessions.iter(), false), 0),
            Some(SidebarAgentSummary {
                state: SidebarAgentState::NeedsInput,
                count: 1,
                tint: None,
            })
        );
    }

    #[test]
    fn a_late_resolution_never_double_counts() {
        let mut sessions = attributed_sessions().to_vec();
        let surfaces = HashSet::from([11u64, 33u64]);
        let folder_before = folder_row_sessions(sessions.iter(), true).count();
        let tab_before = tab_row_sessions(sessions.iter(), &surfaces).count();
        assert_eq!((folder_before, tab_before), (1, 1));

        sessions[2].surface_id = Some(33);
        let folder_after = folder_row_sessions(sessions.iter(), true).count();
        let tab_after = tab_row_sessions(sessions.iter(), &surfaces).count();
        assert_eq!((folder_after, tab_after), (0, 2));
        assert_eq!(folder_before + tab_before, folder_after + tab_after);
    }
}
