use super::*;

use super::agents::AgentRank;
use crate::agent_launcher::TerminalAgent;

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

pub(super) fn lane_state(rank: AgentRank) -> Option<SidebarAgentState> {
    match rank {
        AgentRank::Blocked => Some(SidebarAgentState::NeedsInput),
        AgentRank::Errored => Some(SidebarAgentState::Errored),
        AgentRank::Done => Some(SidebarAgentState::Finished),
        AgentRank::Working => Some(SidebarAgentState::Thinking),
        AgentRank::Idle | AgentRank::NoStatus => None,
    }
}

const SUMMARY_RANKS: [AgentRank; 4] = [
    AgentRank::Blocked,
    AgentRank::Errored,
    AgentRank::Done,
    AgentRank::Working,
];

pub(super) fn sidebar_agent_summary<'a, I, U>(
    sessions: I,
    unread_results: U,
) -> Option<SidebarAgentSummary>
where
    I: IntoIterator<Item = &'a ai_types::AgentSession>,
    U: IntoIterator<Item = Option<u64>>,
{
    let unread: Vec<Option<u64>> = unread_results.into_iter().collect();
    let mut claimed = vec![false; unread.len()];
    let mut counts = [0usize; SUMMARY_RANKS.len()];
    let mut working_tool = None;
    let mut mixed_working_tools = false;
    let slot_of = |rank| SUMMARY_RANKS.iter().position(|&shown| shown == rank);
    for session in sessions {
        let unread_at = session
            .surface_id
            .and_then(|surface_id| unread.iter().position(|key| *key == Some(surface_id)));
        if let Some(slot) = unread_at.and_then(|at| claimed.get_mut(at)) {
            *slot = true;
        }
        let rank = AgentRank::of(session, unread_at.is_some());
        if rank == AgentRank::Working {
            mixed_working_tools |= working_tool.is_some_and(|tool| tool != session.tool);
            working_tool = Some(session.tool);
        }
        if let Some(at) = slot_of(rank) {
            counts[at] += 1;
        }
    }
    if let Some(at) = slot_of(AgentRank::Done) {
        counts[at] += claimed.iter().filter(|&&claimed| !claimed).count();
    }

    let (rank, count) = SUMMARY_RANKS
        .into_iter()
        .zip(counts)
        .find(|&(_, count)| count > 0)?;
    let tint = (rank == AgentRank::Working && !mixed_working_tools)
        .then_some(working_tool)
        .flatten()
        .and_then(TerminalAgent::accent);
    Some(SidebarAgentSummary {
        state: lane_state(rank)?,
        count,
        tint,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TreeStatus {
    Shown,
    LeftToAgentsSection,
}

impl TreeStatus {
    pub(super) fn resolve(
        show: paneflow_config::schema::SidebarShow,
        agents_section_collapsed: bool,
    ) -> Self {
        let agents_section_open = show.agents_enabled() && !agents_section_collapsed;
        if show.status_enabled() || !agents_section_open {
            Self::Shown
        } else {
            Self::LeftToAgentsSection
        }
    }
}

impl PaneFlowApp {
    pub(super) fn tree_status(&self) -> TreeStatus {
        TreeStatus::resolve(
            self.cached_config.sidebar_show,
            self.agents_section.borrow().collapsed(),
        )
    }
}

pub(super) fn tab_row_agent_summary(
    tree_status: TreeStatus,
    workspace: &crate::workspace::Workspace,
    surfaces: &std::collections::HashSet<u64>,
) -> Option<SidebarAgentSummary> {
    if tree_status == TreeStatus::LeftToAgentsSection {
        return None;
    }
    sidebar_agent_summary(
        tab_row_sessions(workspace.agent_sessions.values(), surfaces),
        workspace
            .agent_completion_notification
            .unread_results()
            .filter(|key| key.is_some_and(|surface_id| surfaces.contains(&surface_id))),
    )
}

pub(super) fn folder_row_agent_summary(
    tree_status: TreeStatus,
    workspace: &crate::workspace::Workspace,
    expanded: bool,
) -> Option<SidebarAgentSummary> {
    if tree_status == TreeStatus::LeftToAgentsSection {
        return None;
    }
    sidebar_agent_summary(
        folder_row_sessions(workspace.agent_sessions.values(), expanded),
        workspace
            .agent_completion_notification
            .unread_results()
            .filter(|key| !expanded || key.is_none()),
    )
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

    fn on_surface(state: AgentState, surface_id: u64) -> AgentSession {
        let mut session = session(state);
        session.surface_id = Some(surface_id);
        session.program_status_capable = true;
        session
    }

    fn codex() -> TerminalAgent {
        TerminalAgent::from_binary("codex").expect("codex runtime")
    }

    fn amp() -> TerminalAgent {
        TerminalAgent::from_binary("amp").expect("amp runtime")
    }

    fn claude_tint() -> Option<u32> {
        TerminalAgent::ClaudeCode.accent()
    }

    #[test]
    fn sidebar_agent_summary_hides_idle_without_signal() {
        assert_eq!(sidebar_agent_summary(std::iter::empty(), []), None);
    }

    #[test]
    fn sidebar_agent_summary_counts_winning_needs_input_sessions() {
        let sessions = [
            session(AgentState::WaitingForInput),
            session(AgentState::Errored),
            session(AgentState::WaitingForInput),
        ];
        assert_eq!(
            sidebar_agent_summary(sessions.iter(), []),
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
                sidebar_agent_summary(sessions.iter(), []).map(|summary| summary.state),
                Some(expected)
            );
        }
    }

    #[test]
    fn each_session_is_ranked_with_the_unread_result_of_its_own_surface() {
        let sessions = [
            on_surface(AgentState::Finished, 1),
            on_surface(AgentState::Finished, 2),
            on_surface(AgentState::Thinking, 3),
        ];
        assert_eq!(
            sidebar_agent_summary(sessions.iter(), [Some(1), Some(2)]),
            Some(SidebarAgentSummary {
                state: SidebarAgentState::Finished,
                count: 2,
                tint: None,
            }),
            "done outranks working and counts the sessions of its rank"
        );
        assert_eq!(
            sidebar_agent_summary(sessions.iter(), [Some(2)]).map(|summary| summary.count),
            Some(1)
        );
    }

    #[test]
    fn an_agent_back_at_work_over_an_unseen_result_reads_as_working() {
        let sessions = [on_surface(AgentState::Thinking, 7)];
        assert_eq!(
            sidebar_agent_summary(sessions.iter(), [Some(7)]),
            Some(SidebarAgentSummary {
                state: SidebarAgentState::Thinking,
                count: 1,
                tint: claude_tint(),
            })
        );
    }

    #[test]
    fn idle_and_no_status_sessions_produce_no_status() {
        let idle = on_surface(AgentState::Finished, 1);
        let mut no_status = on_surface(AgentState::Finished, 2);
        no_status.program_status_capable = false;
        assert_eq!(sidebar_agent_summary([&idle, &no_status], []), None);
    }

    #[test]
    fn sidebar_agent_summary_surfaces_unread_completion_without_live_session() {
        assert_eq!(
            sidebar_agent_summary(std::iter::empty(), [None]),
            Some(SidebarAgentSummary {
                state: SidebarAgentState::Finished,
                count: 1,
                tint: None,
            })
        );
        let elsewhere = [on_surface(AgentState::Thinking, 8)];
        assert_eq!(
            sidebar_agent_summary(elsewhere.iter(), [Some(9)]).map(|summary| summary.state),
            Some(SidebarAgentState::Finished),
            "a result whose session is gone still counts as done"
        );
    }

    #[test]
    fn sidebar_agent_summary_hides_acknowledged_finished_session() {
        let sessions = [session(AgentState::Finished)];
        assert_eq!(sidebar_agent_summary(sessions.iter(), []), None);
    }

    #[test]
    fn the_finished_count_is_the_number_of_unread_surfaces() {
        assert_eq!(
            sidebar_agent_summary(std::iter::empty(), [Some(1), Some(2), None]),
            Some(SidebarAgentSummary {
                state: SidebarAgentState::Finished,
                count: 3,
                tint: None,
            })
        );
    }

    #[test]
    fn the_matrix_wears_the_accent_of_a_single_working_runtime() {
        let mut first = on_surface(AgentState::Thinking, 1);
        let mut second = on_surface(AgentState::Thinking, 2);
        assert!(claude_tint().is_some());
        assert_eq!(
            sidebar_agent_summary([&first, &second], []).and_then(|summary| summary.tint),
            claude_tint()
        );

        first.tool = codex();
        second.tool = codex();
        assert_eq!(codex().accent(), None);
        assert_eq!(
            sidebar_agent_summary([&first, &second], []).map(|summary| summary.tint),
            Some(None),
            "a runtime without an accent keeps the muted matrix"
        );
    }

    #[test]
    fn mixed_working_runtimes_keep_the_muted_matrix() {
        let claude = on_surface(AgentState::Thinking, 1);
        let mut other = on_surface(AgentState::Thinking, 2);
        other.tool = amp();
        assert!(amp().accent().is_some() && amp().accent() != claude_tint());
        assert_eq!(
            sidebar_agent_summary([&claude, &other], []),
            Some(SidebarAgentSummary {
                state: SidebarAgentState::Thinking,
                count: 2,
                tint: None,
            })
        );

        let mut idle_other = on_surface(AgentState::Finished, 3);
        idle_other.tool = amp();
        assert_eq!(
            sidebar_agent_summary([&claude, &idle_other], []).and_then(|summary| summary.tint),
            claude_tint(),
            "only the working sessions choose the accent"
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
            sidebar_agent_summary(tab_row_sessions(sessions.iter(), &surfaces), []),
            Some(SidebarAgentSummary {
                state: SidebarAgentState::NeedsInput,
                count: 1,
                tint: None,
            }),
            "a tab must not inherit a sibling tab's session, nor an unattributed one"
        );

        assert_eq!(
            sidebar_agent_summary(tab_row_sessions(sessions.iter(), &HashSet::new()), []),
            None
        );
    }

    #[test]
    fn expanded_folder_keeps_only_the_unattributed_sessions() {
        let sessions = attributed_sessions();
        assert_eq!(
            sidebar_agent_summary(folder_row_sessions(sessions.iter(), true), []),
            Some(SidebarAgentSummary {
                state: SidebarAgentState::Thinking,
                count: 1,
                tint: claude_tint(),
            })
        );

        let resolved = [attributed_sessions()[0].clone()];
        assert_eq!(
            sidebar_agent_summary(folder_row_sessions(resolved.iter(), true), []),
            None
        );
    }

    #[test]
    fn collapsed_folder_re_aggregates_every_tab() {
        let sessions = attributed_sessions();
        assert_eq!(
            sidebar_agent_summary(folder_row_sessions(sessions.iter(), false), []),
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

    fn workspace_with(
        sessions: impl IntoIterator<Item = AgentSession>,
    ) -> crate::workspace::Workspace {
        let mut workspace = crate::workspace::Workspace::empty_with_cwd_and_id(
            1,
            "paneflow",
            std::path::PathBuf::new(),
        );
        for (id, session) in sessions.into_iter().enumerate() {
            workspace.agent_sessions.insert(id as u32, session);
        }
        workspace
    }

    fn status_hidden() -> TreeStatus {
        TreeStatus::LeftToAgentsSection
    }

    #[test]
    fn a_hidden_tree_status_comes_back_while_the_agents_section_is_folded_or_off() {
        use paneflow_config::schema::SidebarShow;

        let with = |status, agents| SidebarShow {
            status: Some(status),
            agents: Some(agents),
            ..Default::default()
        };
        for agents in [true, false] {
            for collapsed in [false, true] {
                assert_eq!(
                    TreeStatus::resolve(with(true, agents), collapsed),
                    TreeStatus::Shown,
                    "status on, agents={agents} collapsed={collapsed}"
                );
            }
        }
        assert_eq!(
            TreeStatus::resolve(with(false, true), false),
            TreeStatus::LeftToAgentsSection
        );
        assert_eq!(
            TreeStatus::resolve(with(false, true), true),
            TreeStatus::Shown,
            "a folded Agents section hands the status back to the tree"
        );
        for collapsed in [false, true] {
            assert_eq!(
                TreeStatus::resolve(with(false, false), collapsed),
                TreeStatus::Shown,
                "a hidden Agents section hands the status back to the tree, collapsed={collapsed}"
            );
        }
        assert_eq!(
            TreeStatus::resolve(SidebarShow::default(), false),
            TreeStatus::Shown
        );

        let workspace = workspace_with([on_surface(AgentState::WaitingForInput, 7)]);
        let surfaces = HashSet::from([7u64]);
        let folded = TreeStatus::resolve(with(false, true), true);
        assert_eq!(
            tab_row_agent_summary(folded, &workspace, &surfaces).map(|summary| summary.state),
            Some(SidebarAgentState::NeedsInput)
        );
        assert_eq!(
            folder_row_agent_summary(folded, &workspace, false).map(|summary| summary.state),
            Some(SidebarAgentState::NeedsInput)
        );
    }

    #[test]
    fn a_hidden_tree_status_leaves_tab_and_workspace_rows_without_a_summary() {
        let shown = TreeStatus::Shown;
        let surfaces = HashSet::from([7u64]);
        let mut with_a_status = 0;
        for state in [
            AgentState::Thinking,
            AgentState::WaitingForInput,
            AgentState::Finished,
            AgentState::Errored,
        ] {
            for unread in [false, true] {
                let mut workspace = workspace_with([on_surface(state, 7)]);
                if unread {
                    workspace
                        .agent_completion_notification
                        .record_finished(false, Some(7));
                }
                let case = format!("{state:?} unread={unread}");
                let expected = sidebar_agent_summary(
                    workspace.agent_sessions.values(),
                    unread.then_some(Some(7)),
                );
                assert_eq!(
                    tab_row_agent_summary(shown, &workspace, &surfaces),
                    expected,
                    "shown tab row, {case}"
                );
                assert_eq!(
                    folder_row_agent_summary(shown, &workspace, false),
                    expected,
                    "shown collapsed workspace row, {case}"
                );
                with_a_status += usize::from(expected.is_some());

                assert_eq!(
                    tab_row_agent_summary(status_hidden(), &workspace, &surfaces),
                    None,
                    "hidden tab row, {case}"
                );
                for expanded in [false, true] {
                    assert_eq!(
                        folder_row_agent_summary(status_hidden(), &workspace, expanded),
                        None,
                        "hidden workspace row expanded={expanded}, {case}"
                    );
                }
            }
        }
        assert_eq!(with_a_status, 7, "every case but finished and seen shows");
    }

    #[test]
    fn a_hidden_tree_status_hides_an_unread_result_without_a_live_session() {
        let mut workspace = workspace_with([]);
        workspace
            .agent_completion_notification
            .record_finished(false, None);
        let shown = TreeStatus::Shown;
        for expanded in [false, true] {
            assert_eq!(
                folder_row_agent_summary(shown, &workspace, expanded).map(|summary| summary.state),
                Some(SidebarAgentState::Finished)
            );
            assert_eq!(
                folder_row_agent_summary(status_hidden(), &workspace, expanded),
                None
            );
        }
    }

    #[test]
    fn a_hidden_tree_status_hands_the_lane_to_the_pull_request() {
        use crate::app::pull_request::{PrState, PullRequest};

        let open = PullRequest {
            number: 46,
            state: PrState::Open,
        };
        let workspace = workspace_with([on_surface(AgentState::WaitingForInput, 7)]);
        let surfaces = HashSet::from([7u64]);
        let shown = TreeStatus::Shown;

        assert!(matches!(
            infer_lane(
                tab_row_agent_summary(shown, &workspace, &surfaces),
                Some(open)
            ),
            Some(Lane::Agent(SidebarAgentSummary {
                state: SidebarAgentState::NeedsInput,
                ..
            }))
        ));
        assert_eq!(
            infer_lane(
                tab_row_agent_summary(status_hidden(), &workspace, &surfaces),
                Some(open)
            ),
            Some(Lane::PullRequest(open))
        );
        assert_eq!(
            infer_lane(
                folder_row_agent_summary(status_hidden(), &workspace, false),
                Some(open)
            ),
            Some(Lane::PullRequest(open))
        );
        assert_eq!(
            infer_lane(
                tab_row_agent_summary(status_hidden(), &workspace, &surfaces),
                None
            ),
            None,
            "no lane means no badge, no matrix, no shimmer and no status tooltip"
        );
    }

    fn working_summary() -> SidebarAgentSummary {
        SidebarAgentSummary {
            state: SidebarAgentState::Thinking,
            count: 1,
            tint: claude_tint(),
        }
    }

    #[test]
    fn the_tooltip_lists_each_tool_once_with_its_extra_sessions() {
        let sessions = [session(AgentState::Thinking), session(AgentState::Errored)];
        let status = ai_types::workspace_agent_status(sessions.iter(), &HashSet::new());
        assert_eq!(
            sidebar_agent_status_tooltip(working_summary(), &status).as_ref(),
            "1 agent thinking · Claude Code +1"
        );
    }

    #[test]
    fn the_tooltip_names_a_detected_agent_without_a_hooked_session_as_running() {
        let detected = HashSet::from([codex().binary().to_string()]);
        let status = ai_types::workspace_agent_status(std::iter::empty(), &detected);
        assert_eq!(
            sidebar_agent_status_tooltip(working_summary(), &status).as_ref(),
            format!("1 agent thinking · {} running", codex().display_name())
        );
    }
}
