use std::cmp::Ordering;

use paneflow_config::schema::SpaceId;

use crate::ai_types::{AgentSession, AgentState};
use crate::workspace::Workspace;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum AgentRank {
    Blocked,
    Errored,
    Done,
    Working,
    Idle,
    NoStatus,
}

impl AgentRank {
    pub(crate) fn of(session: &AgentSession, unread: bool) -> Self {
        match session.state {
            AgentState::WaitingForInput => Self::Blocked,
            AgentState::Errored => Self::Errored,
            AgentState::Finished if unread => Self::Done,
            AgentState::Thinking => Self::Working,
            AgentState::Finished if session.program_status_capable => Self::Idle,
            AgentState::Finished => Self::NoStatus,
        }
    }

    pub(crate) fn awaits_user(self) -> bool {
        self <= Self::Done
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentCandidate {
    pub(crate) surface_id: u64,
    pub(crate) rank: AgentRank,
    pub(crate) workspace_idx: usize,
    pub(crate) tab_idx: usize,
    pub(crate) session_key: u32,
    pub(crate) in_active_space: bool,
    pub(crate) workspace_order: usize,
    pub(crate) surface_order: usize,
    pub(crate) last_event_at_ms: Option<u64>,
    pub(crate) state_seq: u64,
}

impl AgentCandidate {
    fn recency(&self) -> (bool, Option<u64>, u64) {
        (
            self.last_event_at_ms.is_some(),
            self.last_event_at_ms,
            self.state_seq,
        )
    }

    fn urgency(&self, other: &Self) -> Ordering {
        other
            .in_active_space
            .cmp(&self.in_active_space)
            .then(self.rank.cmp(&other.rank))
            .then_with(|| other.recency().cmp(&self.recency()))
            .then(self.workspace_order.cmp(&other.workspace_order))
            .then(self.surface_order.cmp(&other.surface_order))
    }
}

pub(crate) fn order_agents(
    mut candidates: Vec<AgentCandidate>,
    active_space_only: bool,
) -> Vec<AgentCandidate> {
    if active_space_only {
        candidates.retain(|candidate| candidate.in_active_space);
    }
    candidates.sort_by(AgentCandidate::urgency);
    candidates
}

pub(crate) fn agent_candidates(
    workspaces: &[Workspace],
    active_space: Option<&SpaceId>,
    cx: &gpui::App,
) -> Vec<AgentCandidate> {
    let display = crate::PaneFlowApp::compute_display_order(workspaces, active_space);
    let mut candidates = Vec::new();
    for (workspace_idx, workspace) in workspaces.iter().enumerate() {
        if workspace.agent_sessions.is_empty() {
            continue;
        }
        let in_active_space = crate::app::spaces::in_space(workspace, active_space);
        let workspace_order = display
            .iter()
            .position(|&shown| shown == workspace_idx)
            .unwrap_or(workspaces.len() + workspace_idx);
        let surfaces: Vec<(u64, usize)> = workspace
            .tabs()
            .iter()
            .enumerate()
            .flat_map(|(tab_idx, tab)| {
                tab.collect_panes()
                    .into_iter()
                    .map(move |pane| (tab_idx, pane))
            })
            .flat_map(|(tab_idx, pane)| {
                pane.read(cx)
                    .terminals()
                    .map(|terminal| (terminal.entity_id().as_u64(), tab_idx))
                    .collect::<Vec<_>>()
            })
            .collect();
        for (&session_key, session) in &workspace.agent_sessions {
            let Some(surface_id) = session.surface_id else {
                continue;
            };
            let Some(surface_order) = surfaces.iter().position(|&(id, _)| id == surface_id) else {
                continue;
            };
            let tab_idx = surfaces[surface_order].1;
            let unread = workspace
                .agent_completion_notification
                .is_unread_surface(surface_id);
            candidates.push(AgentCandidate {
                surface_id,
                rank: AgentRank::of(session, unread),
                workspace_idx,
                tab_idx,
                session_key,
                in_active_space,
                workspace_order,
                surface_order,
                last_event_at_ms: session.last_event_at_ms,
                state_seq: session.state_seq,
            });
        }
    }
    candidates
}

pub(crate) fn awaiting_agent_surfaces(
    workspaces: &[Workspace],
    active_space: Option<&SpaceId>,
    cx: &gpui::App,
) -> Vec<u64> {
    order_agents(agent_candidates(workspaces, active_space, cx), false)
        .into_iter()
        .filter(|candidate| candidate.rank.awaits_user())
        .map(|candidate| candidate.surface_id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_launcher::TerminalAgent;

    fn session(state: AgentState, capable: bool) -> AgentSession {
        let mut session = AgentSession::new(TerminalAgent::ClaudeCode, state);
        session.program_status_capable = capable;
        session
    }

    fn candidate(surface_id: u64, rank: AgentRank) -> AgentCandidate {
        AgentCandidate {
            surface_id,
            rank,
            workspace_idx: 0,
            tab_idx: 0,
            session_key: 0,
            in_active_space: true,
            workspace_order: 0,
            surface_order: surface_id as usize,
            last_event_at_ms: None,
            state_seq: 0,
        }
    }

    fn surfaces(candidates: &[AgentCandidate]) -> Vec<u64> {
        candidates
            .iter()
            .map(|candidate| candidate.surface_id)
            .collect()
    }

    #[test]
    fn each_session_gets_its_urgency_rank_from_its_declared_state_and_unread_result() {
        let cases = [
            (AgentState::WaitingForInput, false, true, AgentRank::Blocked),
            (AgentState::Errored, false, true, AgentRank::Errored),
            (AgentState::Finished, true, true, AgentRank::Done),
            (AgentState::Thinking, false, true, AgentRank::Working),
            (AgentState::Finished, false, true, AgentRank::Idle),
            (AgentState::Finished, false, false, AgentRank::NoStatus),
        ];
        for (state, unread, capable, rank) in cases {
            assert_eq!(
                AgentRank::of(&session(state, capable), unread),
                rank,
                "{state:?} unread={unread} capable={capable}"
            );
        }
        assert_eq!(
            [
                AgentRank::Blocked,
                AgentRank::Errored,
                AgentRank::Done,
                AgentRank::Working,
                AgentRank::Idle,
                AgentRank::NoStatus,
            ]
            .map(|rank| rank as u8),
            [0, 1, 2, 3, 4, 5]
        );
    }

    #[test]
    fn only_blocked_errored_and_unread_done_await_the_user() {
        let awaiting: Vec<AgentRank> = [
            AgentRank::Blocked,
            AgentRank::Errored,
            AgentRank::Done,
            AgentRank::Working,
            AgentRank::Idle,
            AgentRank::NoStatus,
        ]
        .into_iter()
        .filter(|rank| rank.awaits_user())
        .collect();
        assert_eq!(
            awaiting,
            [AgentRank::Blocked, AgentRank::Errored, AgentRank::Done]
        );
    }

    #[test]
    fn ranks_sort_before_recency_and_the_most_recent_of_a_rank_comes_first() {
        let mut idle = candidate(1, AgentRank::Idle);
        idle.last_event_at_ms = Some(900);
        let mut old_blocked = candidate(2, AgentRank::Blocked);
        old_blocked.last_event_at_ms = Some(100);
        let mut new_blocked = candidate(3, AgentRank::Blocked);
        new_blocked.last_event_at_ms = Some(500);
        let mut same_time_later_seq = candidate(4, AgentRank::Blocked);
        same_time_later_seq.last_event_at_ms = Some(100);
        same_time_later_seq.state_seq = 9;
        let no_status = candidate(5, AgentRank::NoStatus);
        let errored = candidate(6, AgentRank::Errored);

        let ordered = order_agents(
            vec![
                idle,
                old_blocked,
                no_status,
                new_blocked,
                errored,
                same_time_later_seq,
            ],
            false,
        );

        assert_eq!(surfaces(&ordered), [3, 4, 2, 6, 1, 5]);
    }

    #[test]
    fn an_undated_session_follows_the_dated_ones_of_its_rank_without_panicking() {
        let undated = candidate(1, AgentRank::Working);
        let mut dated = candidate(2, AgentRank::Working);
        dated.last_event_at_ms = Some(0);
        let mut undated_later = candidate(3, AgentRank::Working);
        undated_later.state_seq = 4;

        let ordered = order_agents(vec![undated, dated, undated_later], false);

        assert_eq!(surfaces(&ordered), [2, 3, 1]);
    }

    #[test]
    fn workspace_then_tab_order_breaks_the_remaining_ties() {
        let mut later_workspace = candidate(1, AgentRank::Working);
        later_workspace.workspace_order = 2;
        later_workspace.surface_order = 0;
        let mut second_tab = candidate(2, AgentRank::Working);
        second_tab.workspace_order = 1;
        second_tab.surface_order = 5;
        let mut first_tab = candidate(3, AgentRank::Working);
        first_tab.workspace_order = 1;
        first_tab.surface_order = 1;

        let ordered = order_agents(vec![later_workspace, second_tab, first_tab], false);

        assert_eq!(surfaces(&ordered), [3, 2, 1]);
    }

    #[test]
    fn the_active_space_comes_first_and_alone_when_filtered() {
        let mut hidden_blocked = candidate(1, AgentRank::Blocked);
        hidden_blocked.in_active_space = false;
        let shown_done = candidate(2, AgentRank::Done);

        let everywhere = order_agents(vec![hidden_blocked.clone(), shown_done.clone()], false);
        assert_eq!(surfaces(&everywhere), [2, 1]);

        let section = order_agents(vec![hidden_blocked, shown_done], true);
        assert_eq!(surfaces(&section), [2]);
    }

    #[gpui::test]
    fn candidates_come_from_the_workspace_sessions_with_unread_results_and_spaces(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::pane::Pane;
        use crate::terminal::TerminalView;
        use gpui::AppContext as _;

        let cx = cx.add_empty_window();
        let hidden = SpaceId::parse("550e8400-e29b-41d4-a716-446655440001").expect("space id");
        let mut surfaces_by_workspace = Vec::new();
        let mut workspaces: Vec<Workspace> = [(1, None), (2, None), (3, Some(hidden.clone()))]
            .into_iter()
            .map(|(id, space)| {
                let terminal = cx.new(|cx| TerminalView::display_only_for_test(id, cx));
                surfaces_by_workspace.push(terminal.entity_id().as_u64());
                let pane = cx.new(|cx| Pane::new(terminal, id, cx));
                let mut workspace = Workspace::with_id(id, format!("ws{id}"), pane);
                workspace.space = space;
                workspace
            })
            .collect();
        let states = [
            AgentState::Finished,
            AgentState::Thinking,
            AgentState::WaitingForInput,
        ];
        for ((workspace, surface_id), state) in workspaces
            .iter_mut()
            .zip(surfaces_by_workspace.iter().copied())
            .zip(states)
        {
            let mut agent = session(state, true);
            agent.surface_id = Some(surface_id);
            workspace.agent_sessions.insert(1, agent);
        }
        let mut orphan = session(AgentState::WaitingForInput, true);
        orphan.surface_id = Some(9_999);
        workspaces[1].agent_sessions.insert(2, orphan);
        workspaces[0]
            .agent_completion_notification
            .record_finished(false, Some(surfaces_by_workspace[0]));

        let section =
            cx.update(|_, cx| order_agents(agent_candidates(&workspaces, None, cx), true));
        assert_eq!(
            surfaces(&section),
            [surfaces_by_workspace[0], surfaces_by_workspace[1]],
            "the hidden space and a session without a live surface are absent"
        );
        assert_eq!(
            section.iter().map(|row| row.rank).collect::<Vec<_>>(),
            [AgentRank::Done, AgentRank::Working]
        );

        let jump = cx.update(|_, cx| awaiting_agent_surfaces(&workspaces, None, cx));
        assert_eq!(
            jump,
            [surfaces_by_workspace[0], surfaces_by_workspace[2]],
            "the active space's done result, then the hidden space's blocked agent"
        );

        let seen: std::collections::HashSet<u64> = [surfaces_by_workspace[0]].into();
        workspaces[0]
            .agent_completion_notification
            .acknowledge(&seen, &seen);
        let jump = cx.update(|_, cx| awaiting_agent_surfaces(&workspaces, None, cx));
        assert_eq!(
            jump,
            [surfaces_by_workspace[2]],
            "an acknowledged result leaves the cycle"
        );

        let hidden_view =
            cx.update(|_, cx| awaiting_agent_surfaces(&workspaces, Some(&hidden), cx));
        assert_eq!(hidden_view, [surfaces_by_workspace[2]]);
    }
}
