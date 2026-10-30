use std::collections::{HashMap, HashSet};

use gpui::{Context, Window};

use paneflow_config::schema::SpaceId;

use crate::PaneFlowApp;
use crate::ai_types::AgentState;
use crate::app::declared_status::surface_has_agent;
use crate::app::ipc_handler::find_pane_by_surface_id;
use crate::app::workspace_ops::WorkspaceFocusTarget;
use crate::workspace::Workspace;

pub(crate) struct AttentionItem {
    pub(crate) surface_id: Option<u64>,
    pub(crate) workspace_idx: Option<usize>,
    pub(crate) waiting_secs: u64,
    pub(crate) errored: bool,
}

pub(crate) fn sort_items(items: &mut [AttentionItem]) {
    items.sort_by(|a, b| {
        b.surface_id
            .is_some()
            .cmp(&a.surface_id.is_some())
            .then(b.waiting_secs.cmp(&a.waiting_secs))
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AttentionLevel {
    Input,
    Error,
    Finished,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SpaceAttention {
    pub(crate) input: usize,
    pub(crate) errors: usize,
    pub(crate) finished: usize,
}

impl SpaceAttention {
    pub(crate) fn total(&self) -> usize {
        self.input + self.errors + self.finished
    }

    pub(crate) fn most_urgent(&self) -> Option<AttentionLevel> {
        [
            (AttentionLevel::Input, self.input),
            (AttentionLevel::Error, self.errors),
            (AttentionLevel::Finished, self.finished),
        ]
        .into_iter()
        .find(|(_, count)| *count > 0)
        .map(|(level, _)| level)
    }
}

pub(crate) fn space_attention(
    workspaces: &[Workspace],
    items: &[AttentionItem],
) -> HashMap<Option<SpaceId>, SpaceAttention> {
    let mut spaces: HashMap<Option<SpaceId>, SpaceAttention> = HashMap::new();
    for item in items {
        let Some(workspace) = item.workspace_idx.and_then(|idx| workspaces.get(idx)) else {
            continue;
        };
        let counts = spaces.entry(workspace.space.clone()).or_default();
        if item.errored {
            counts.errors += 1;
        } else {
            counts.input += 1;
        }
    }
    for workspace in workspaces.iter().filter(|workspace| !workspace.muted) {
        let finished = workspace.agent_completion_notification.unread_count();
        if finished > 0 {
            spaces.entry(workspace.space.clone()).or_default().finished += finished;
        }
    }
    spaces
}

impl PaneFlowApp {
    pub(crate) fn space_attention(
        &self,
        cx: &Context<Self>,
    ) -> HashMap<Option<SpaceId>, SpaceAttention> {
        space_attention(&self.workspaces, &self.attention_items(cx))
    }

    pub(crate) fn attention_items(&self, cx: &Context<Self>) -> Vec<AttentionItem> {
        let mut live_surfaces: HashSet<u64> = HashSet::new();
        let mut session_surfaces = HashMap::new();
        for (ws_idx, ws) in self.workspaces.iter().enumerate() {
            for pane in ws.collect_panes() {
                for t in pane.read(cx).terminals() {
                    let surface_id = t.entity_id().as_u64();
                    live_surfaces.insert(surface_id);
                    session_surfaces
                        .insert(t.read(cx).terminal.session_id.clone(), (surface_id, ws_idx));
                }
            }
        }
        let mut items = Vec::new();
        for pending in &self.write_approvals.pending {
            let place = session_surfaces.get(&pending.target);
            items.push(AttentionItem {
                surface_id: place.map(|(surface_id, _)| *surface_id),
                workspace_idx: place.map(|(_, ws_idx)| *ws_idx),
                waiting_secs: pending.asked_at.elapsed().as_secs(),
                errored: false,
            });
        }
        for (ws_idx, ws) in self.workspaces.iter().enumerate() {
            for session in ws.agent_sessions.values() {
                if session.state != AgentState::WaitingForInput {
                    continue;
                }
                let surface_id = match session.surface_id {
                    Some(sid) if live_surfaces.contains(&sid) => Some(sid),
                    Some(_) => continue,
                    None => None,
                };
                items.push(AttentionItem {
                    surface_id,
                    workspace_idx: Some(ws_idx),
                    waiting_secs: session
                        .waiting_since
                        .map(|t| t.elapsed().as_secs())
                        .unwrap_or(0),
                    errored: false,
                });
            }
        }
        for (ws_idx, ws) in self.workspaces.iter().enumerate() {
            for pane in ws.collect_panes() {
                for terminal in pane.read(cx).terminals() {
                    let surface_id = terminal.entity_id().as_u64();
                    let view = terminal.read(cx);
                    let row = self.host_agent_row(&view.terminal.session_id);
                    if surface_has_agent(ws, surface_id, row) {
                        continue;
                    }
                    let Some(level) = self
                        .host_agents
                        .declared_watch
                        .attention(surface_id, view.terminal.declared_status.as_ref())
                    else {
                        continue;
                    };
                    items.push(AttentionItem {
                        surface_id: Some(surface_id),
                        workspace_idx: Some(ws_idx),
                        waiting_secs: self
                            .declared_since(surface_id)
                            .map(|since| since.elapsed().as_secs())
                            .unwrap_or(0),
                        errored: level == AttentionLevel::Error,
                    });
                }
            }
        }
        sort_items(&mut items);
        items
    }

    pub(crate) fn activate_attention_surface(
        &mut self,
        surface_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(loc) = find_pane_by_surface_id(&self.workspaces, surface_id, cx) else {
            cx.notify();
            return;
        };
        let (ws_idx, pane) = (loc.workspace_idx, loc.pane);
        self.activate_workspace_at(ws_idx, WorkspaceFocusTarget::Pane { pane }, window, cx);
        self.jump_cursor = Some(surface_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(surface_id: Option<u64>, waiting_secs: u64) -> AttentionItem {
        AttentionItem {
            surface_id,
            workspace_idx: None,
            waiting_secs,
            errored: false,
        }
    }

    #[test]
    fn sorts_longest_wait_first_unmapped_last() {
        let mut items = vec![
            item(Some(1), 10),
            item(None, 9_999),
            item(Some(2), 300),
            item(Some(3), 60),
        ];
        sort_items(&mut items);
        let order: Vec<Option<u64>> = items.iter().map(|i| i.surface_id).collect();
        assert_eq!(order, vec![Some(2), Some(3), Some(1), None]);
    }

    fn space(n: u8) -> SpaceId {
        SpaceId::parse(&format!("550e8400-e29b-41d4-a716-4466554400{n:02}")).unwrap()
    }

    fn workspace(id: u64, space: Option<SpaceId>) -> Workspace {
        let mut workspace =
            Workspace::empty_with_cwd_and_id(id, format!("ws{id}"), std::path::PathBuf::new());
        workspace.space = space;
        workspace
    }

    fn located(workspace_idx: Option<usize>, errored: bool) -> AttentionItem {
        AttentionItem {
            workspace_idx,
            errored,
            ..item(Some(1), 0)
        }
    }

    #[test]
    fn each_space_counts_its_waiting_failed_and_unseen_finished_agents() {
        let mut workspaces = vec![
            workspace(1, None),
            workspace(2, Some(space(1))),
            workspace(3, Some(space(1))),
            workspace(4, Some(space(2))),
        ];
        workspaces[2]
            .agent_completion_notification
            .record_finished(false, Some(30));
        workspaces[3]
            .agent_completion_notification
            .record_finished(false, Some(40));
        let items = vec![
            located(Some(1), false),
            located(Some(2), true),
            located(Some(0), true),
            located(None, false),
        ];

        let counts = space_attention(&workspaces, &items);

        let work = counts[&Some(space(1))];
        assert_eq!(
            work,
            SpaceAttention {
                input: 1,
                errors: 1,
                finished: 1
            }
        );
        assert_eq!(work.total(), 3);
        assert_eq!(work.most_urgent(), Some(AttentionLevel::Input));
        assert_eq!(counts[&None].most_urgent(), Some(AttentionLevel::Error));
        assert_eq!(
            counts[&Some(space(2))].most_urgent(),
            Some(AttentionLevel::Finished)
        );
        assert_eq!(SpaceAttention::default().most_urgent(), None);
    }

    #[test]
    fn a_finish_in_a_muted_workspace_does_not_count() {
        let mut workspaces = vec![workspace(1, None), workspace(2, Some(space(1)))];
        workspaces[1].muted = true;
        workspaces[1]
            .agent_completion_notification
            .record_finished(false, Some(20));

        let counts = space_attention(&workspaces, &[]);

        assert_eq!(counts.get(&Some(space(1))), None);
    }

    #[gpui::test]
    fn activating_a_hidden_agent_switches_to_its_space(cx: &mut gpui::TestAppContext) {
        use crate::pane::Pane;
        use crate::terminal::TerminalView;
        use gpui::AppContext as _;

        let cx = cx.add_empty_window();
        let mut terminals = Vec::new();
        let workspaces: Vec<Workspace> = [(1, None), (2, Some(space(1)))]
            .into_iter()
            .map(|(id, space)| {
                let terminal = cx.new(|cx| TerminalView::display_only_for_test(id, cx));
                terminals.push(terminal.entity_id().as_u64());
                let pane = cx.new(|cx| Pane::new(terminal, id, cx));
                let mut workspace = Workspace::with_id(id, format!("ws{id}"), pane);
                workspace.space = space;
                workspace
            })
            .collect();
        let mut active_idx = 0;
        let mut active_space = None;
        let mut memory = crate::app::spaces::SpaceMemory::new();
        let closed = cx.new(|cx| TerminalView::display_only_for_test(3, cx));
        let closed = closed.entity_id().as_u64();

        let gone = cx.update(|_, cx| find_pane_by_surface_id(&workspaces, closed, cx));
        assert!(gone.is_none());
        assert_eq!((active_idx, active_space.clone()), (0, None));

        let destination = cx
            .update(|_, cx| find_pane_by_surface_id(&workspaces, terminals[1], cx))
            .expect("the waiting agent's pane is found");
        assert!(crate::app::spaces::point_at(
            &workspaces,
            &mut active_idx,
            &mut active_space,
            &mut memory,
            destination.workspace_idx,
        ));

        assert_eq!(active_space, Some(space(1)));
        assert_eq!(active_idx, 1);
        let focused = cx.update(|_, cx| {
            destination
                .pane
                .read(cx)
                .terminals()
                .next()
                .map(|terminal| terminal.entity_id().as_u64())
        });
        assert_eq!(focused, Some(terminals[1]));
    }
}
