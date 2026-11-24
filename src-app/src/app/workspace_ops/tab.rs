use gpui::{App, AppContext, Context, Entity, Window};
use paneflow_config::schema::{TabTitleSource, TerminalSurfaceProfile};

use crate::PaneFlowApp;
use crate::app::close_policy::CloseTarget;
use crate::app::workspace_ops::SurfaceLaunch;
use crate::layout::LayoutTree;
use crate::pane::Pane;
use crate::terminal::TerminalView;
use crate::workspace::Tab;
use crate::{CloseTab, NewTab, NextTab, PreviousTab, TabDrag};

impl PaneFlowApp {
    pub(crate) fn toggle_workspace_expanded(&mut self, ws_idx: usize, cx: &mut Context<Self>) {
        if let Some(ws) = self.workspaces.get_mut(ws_idx) {
            ws.sidebar_expanded = !ws.sidebar_expanded;
            self.save_session(cx);
            cx.notify();
        }
    }

    pub(crate) fn toggle_workspace_muted(&mut self, ws_idx: usize, cx: &mut Context<Self>) {
        if let Some(ws) = self.workspaces.get_mut(ws_idx) {
            ws.muted = !ws.muted;
            self.save_session(cx);
            cx.notify();
        }
    }

    pub(crate) fn workspace_is_muted(&self, ws_id: u64) -> bool {
        self.workspaces
            .iter()
            .find(|ws| ws.id == ws_id)
            .is_some_and(|ws| ws.muted)
    }

    pub(crate) fn mark_workspace_read(&mut self, ws_idx: usize, cx: &mut Context<Self>) {
        if let Some(ws) = self.workspaces.get_mut(ws_idx) {
            ws.agent_completion_notification.clear();
            self.save_session(cx);
            cx.notify();
        }
    }

    pub(crate) fn open_tab_with_surface(
        &mut self,
        ws_idx: usize,
        title: String,
        profile: TerminalSurfaceProfile,
        launch: SurfaceLaunch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(ws) = self.workspaces.get(ws_idx) else {
            return false;
        };
        let ws_id = ws.id;
        let fills_active_tab =
            ws.active_tab().root.is_none() && ws.active_tab().saved_layout.is_none();
        let cwd = launch.cwd.clone().or_else(|| {
            fills_active_tab
                .then(|| ws.active_tab().worktree.clone())
                .flatten()
                .or_else(|| (!ws.cwd.is_empty()).then(|| std::path::PathBuf::from(&ws.cwd)))
        });
        let title = crate::sidebar_title::clean_sidebar_title(&title).unwrap_or_default();
        let terminal = cx.new(|cx| {
            TerminalView::with_cwd_env_and_profile(ws_id, cwd, None, launch.env, profile, cx)
        });
        cx.subscribe(&terminal, Self::handle_terminal_event)
            .detach();
        let pane = self.create_pane(terminal.clone(), ws_id, cx);
        let root = LayoutTree::Leaf(pane);
        let opened = self
            .open_tab_through_model(ws_idx, title, Some(root), None, fills_active_tab, cx)
            .is_ok();
        if !opened {
            self.show_toast("Tab limit reached for this workspace", cx);
            return false;
        }
        if let Some(command) = launch.command.as_deref() {
            terminal.read(cx).send_command(command);
            terminal.update(cx, |view, _cx| view.declare_agent_from_command(command));
        }
        if let Some(ws) = self.workspaces.get_mut(ws_idx) {
            ws.sidebar_expanded = true;
        }
        let tab_idx = self.workspaces[ws_idx].active_tab_idx();
        self.focus_workspace_tab(ws_idx, tab_idx, window, cx);
        cx.notify();
        true
    }

    pub(crate) fn handle_new_tab(
        &mut self,
        _: &NewTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(idx) = self.shown_workspace_idx() {
            self.open_pane_palette(idx, window, cx);
        }
    }

    pub(crate) fn handle_next_tab(
        &mut self,
        _: &NextTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_active_workspace_tab(1, window, cx);
    }

    pub(crate) fn handle_previous_tab(
        &mut self,
        _: &PreviousTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_active_workspace_tab(-1, window, cx);
    }

    fn cycle_active_workspace_tab(
        &mut self,
        step: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.active_workspace() else {
            return;
        };
        let count = ws.tab_count();
        if count < 2 {
            return;
        }
        let current = ws.active_tab_idx() as isize;
        let next = (current + step).rem_euclid(count as isize) as usize;
        if let Some(idx) = self.shown_workspace_idx() {
            self.focus_workspace_tab(idx, next, window, cx);
        }
        cx.notify();
    }

    pub(crate) fn select_workspace_tab(
        &mut self,
        ws_idx: usize,
        tab_idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .workspaces
            .get(ws_idx)
            .is_none_or(|ws| tab_idx >= ws.tab_count())
        {
            return;
        }
        self.commit_rename(cx);
        self.dismiss_transient_surfaces();
        self.focus_workspace_tab(ws_idx, tab_idx, window, cx);
        cx.notify();
    }

    pub(crate) fn focus_workspace_tab(
        &mut self,
        ws_idx: usize,
        tab_idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let shown = self.shown_workspace_idx() == Some(ws_idx);
        let pending = self.pending_pane_focus.clone();
        self.select_tab_through_model(ws_idx, tab_idx, cx);
        if shown {
            self.pending_pane_focus = pending;
            self.workspaces[ws_idx].focus_first(window, cx);
        } else if ws_idx < self.workspaces.len() {
            self.pending_pane_focus = pending;
            self.after_workspace_activated(
                ws_idx,
                true,
                crate::app::workspace_ops::WorkspaceFocusTarget::FirstPane,
                window,
                cx,
            );
        }
        if self.pane_palette_fills_active_tab(ws_idx) {
            window.focus(&self.pane_palette_focus, cx);
        }
        self.acknowledge_visible_completions(cx);
    }

    pub(crate) fn sync_terminal_visibility(&self, cx: &App) {
        sync_terminal_visibility(&self.workspaces, self.shown_workspace_idx(), cx);
    }

    pub(crate) fn acknowledge_visible_completions(&mut self, cx: &mut Context<Self>) {
        let Some(shown) = self.shown_workspace_idx() else {
            return;
        };
        let ws = &self.workspaces[shown];
        let badge_unread = ws.agent_completion_notification.is_unread();
        let seen = ws.active_tab().surface_ids(cx);
        let acknowledged = self.unread_sessions_for_surfaces(&seen, cx);
        if acknowledged.is_empty() && !badge_unread {
            return;
        }
        let live: std::collections::HashSet<u64> = self.workspaces[shown]
            .tabs()
            .iter()
            .flat_map(|tab| tab.surface_ids(cx))
            .collect();
        if badge_unread && let Some(ws) = self.active_workspace_mut() {
            ws.agent_completion_notification.acknowledge(&seen, &live);
        }
        crate::app::host_agents::acknowledge_worker_unread(
            acknowledged,
            cx.background_executor().clone(),
        );
        cx.notify();
    }

    pub(crate) fn close_workspace_tab(
        &mut self,
        ws_idx: usize,
        tab_idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .workspaces
            .get(ws_idx)
            .and_then(|ws| ws.tabs().get(tab_idx))
            .is_none()
        {
            return;
        }
        self.request_close(CloseTarget::Tab { ws_idx, tab_idx }, Some(window), cx);
    }

    pub(crate) fn remove_workspace_tab(
        &mut self,
        ws_idx: usize,
        tab_idx: usize,
        keep_sessions: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ws_id) = self.workspaces.get(ws_idx).map(|ws| ws.id) else {
            return;
        };
        let Some(closed) = self.close_tab_through_model(ws_idx, tab_idx, keep_sessions, cx) else {
            if self.workspaces.get(ws_idx).is_some_and(|ws| ws.id == ws_id) {
                self.prune_parked_diff_docks(cx);
                self.prune_worktree_states();
            }
            return;
        };
        if self
            .renaming_tab
            .is_some_and(|key| key.workspace_id == ws_id)
        {
            self.renaming_tab = None;
        }
        self.dismiss_transient_surfaces();
        self.settle_layout_focus(window, cx);
        self.prune_parked_diff_docks(cx);
        self.prune_worktree_states();
        self.show_tab_closed_toast(closed.get(), cx);
    }

    pub(crate) fn reopen_closed_tab(
        &mut self,
        tab_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.undo_close_through_model(Some(paneflow_layout::TabId(tab_id)), window, cx);
    }

    pub(crate) fn handle_close_tab(
        &mut self,
        _: &CloseTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab_idx) = self.active_workspace().map(|ws| ws.active_tab_idx()) else {
            return;
        };
        if let Some(idx) = self.shown_workspace_idx() {
            self.close_workspace_tab(idx, tab_idx, window, cx);
        }
    }

    pub(crate) fn begin_tab_rename(
        &mut self,
        ws_idx: usize,
        tab_idx: usize,
        cx: &mut Context<Self>,
    ) {
        self.commit_rename(cx);
        let Some(ws) = self.workspaces.get(ws_idx) else {
            return;
        };
        let Some((key, title)) = crate::TabKey::of(ws, tab_idx)
            .zip(ws.tabs().get(tab_idx).map(|tab| tab.title().to_string()))
        else {
            return;
        };
        self.rename_input
            .update(cx, |input, cx| input.set_value(title, cx));
        self.renaming_tab = Some(key);
        cx.notify();
    }

    pub(crate) fn reset_tab_name(&mut self, ws_idx: usize, tab_idx: usize, cx: &mut Context<Self>) {
        if !self.unlock_tab_title_through_model(ws_idx, tab_idx, cx) {
            return;
        }
        if let Some(terminal) = self
            .workspaces
            .get(ws_idx)
            .and_then(|ws| ws.tabs().get(tab_idx))
            .and_then(|tab| sole_terminal(tab, cx))
        {
            self.apply_process_title(&terminal, cx);
        }
        cx.notify();
    }

    pub(crate) fn apply_process_title(
        &mut self,
        terminal: &Entity<TerminalView>,
        cx: &mut Context<Self>,
    ) {
        let Some(ws_idx) = self.workspace_idx_for_terminal(terminal, cx) else {
            return;
        };
        let surface_id = terminal.read(cx).surface_id();
        let Some((tab_idx, 1)) =
            crate::app::ipc_handler::tab_for_surface(&self.workspaces[ws_idx], surface_id, cx)
        else {
            return;
        };
        let Some(title) =
            crate::sidebar_title::clean_sidebar_title(&terminal.read(cx).terminal.title)
        else {
            return;
        };
        self.retitle_tab_through_model(ws_idx, tab_idx, &title, TabTitleSource::Process, cx);
    }

    pub(crate) fn reorder_workspace_tab(
        &mut self,
        drag: &TabDrag,
        target_ws_idx: usize,
        target_idx: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.workspaces.get(target_ws_idx) else {
            return;
        };
        if ws.id != drag.workspace_id {
            return;
        }
        let Some(from) = ws.tabs().iter().position(|tab| tab.id == drag.tab_id) else {
            return;
        };
        if from == target_idx {
            return;
        }
        self.adopt_layout(cx);
        let Some(tab) = self.layout_tab_id(drag.tab_id) else {
            return;
        };
        let _ = self.apply_layout_op(
            paneflow_layout::Op::ReorderTab {
                tab,
                to: target_idx,
            },
            cx,
        );
    }

    pub(crate) fn move_tab_to_workspace(
        &mut self,
        drag: &TabDrag,
        dest_ws_idx: usize,
        insert_idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(source_ws_idx) = self
            .workspaces
            .iter()
            .position(|ws| ws.id == drag.workspace_id)
        else {
            return;
        };
        if source_ws_idx == dest_ws_idx {
            return;
        }
        let Some(dest) = self.workspaces.get(dest_ws_idx) else {
            return;
        };
        if !dest.can_open_tab() {
            self.show_toast("Tab limit reached for this workspace", cx);
            return;
        }
        let Some(tab) = self
            .workspaces
            .get(source_ws_idx)
            .and_then(|ws| ws.tabs().iter().find(|tab| tab.id == drag.tab_id))
        else {
            return;
        };
        let surfaces = tab.surface_ids(cx);
        self.hand_surfaces_between(source_ws_idx, dest_ws_idx, &surfaces);
        self.adopt_layout(cx);
        let (Some(model_tab), Some(workspace)) = (
            self.layout_tab_id(drag.tab_id),
            self.layout_workspace_id(self.workspaces[dest_ws_idx].id),
        ) else {
            return;
        };
        if self
            .apply_layout_op(
                paneflow_layout::Op::MoveTab {
                    tab: model_tab,
                    workspace,
                    index: Some(insert_idx),
                },
                cx,
            )
            .is_err()
        {
            log::warn!("tab move: destination refused the tab after the cap check");
            return;
        }
        let Some(dest_ws_idx) = self
            .workspaces
            .iter()
            .position(|ws| self.layout_workspace_id(ws.id) == Some(workspace))
        else {
            return;
        };
        self.renaming_tab = None;
        self.workspaces[dest_ws_idx].sidebar_expanded = true;
        let dest_tab_idx = self.workspaces[dest_ws_idx].active_tab_idx();
        self.focus_workspace_tab(dest_ws_idx, dest_tab_idx, window, cx);
    }

    pub(crate) fn rehome_panes(
        &self,
        panes: &[Entity<Pane>],
        dest_ws_idx: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(dest) = self.workspaces.get(dest_ws_idx) else {
            return;
        };
        let durable = dest.durable_id.clone();
        let sessions = move_panes_to_workspace(panes, dest.id, cx);
        if sessions.is_empty() {
            return;
        }
        #[cfg(test)]
        if crate::app::fake_host::installed(cx) {
            for session in sessions {
                crate::app::fake_host::record(
                    cx,
                    crate::app::fake_host::HostRequest::Assign {
                        session,
                        workspace: durable.clone(),
                    },
                );
            }
            return;
        }
        cx.background_executor()
            .spawn(async move {
                crate::terminal::host_link::assign_workspace(&sessions, &durable);
            })
            .detach();
    }

    fn hand_surfaces_between(
        &mut self,
        source_ws_idx: usize,
        dest_ws_idx: usize,
        surfaces: &std::collections::HashSet<u64>,
    ) {
        if source_ws_idx == dest_ws_idx
            || source_ws_idx >= self.workspaces.len()
            || dest_ws_idx >= self.workspaces.len()
        {
            return;
        }
        let (source, dest) = if source_ws_idx < dest_ws_idx {
            let (head, tail) = self.workspaces.split_at_mut(dest_ws_idx);
            (&mut head[source_ws_idx], &mut tail[0])
        } else {
            let (head, tail) = self.workspaces.split_at_mut(source_ws_idx);
            (&mut tail[0], &mut head[dest_ws_idx])
        };
        source.hand_surfaces_to(dest, surfaces);
    }

    pub(crate) fn move_pane_to_new_tab(
        &mut self,
        pane_id: u64,
        dest_ws_idx: usize,
        insert_idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((src_ws_idx, src_tab_idx, pane)) =
            self.workspaces.iter().enumerate().find_map(|(ws_idx, ws)| {
                ws.tabs().iter().enumerate().find_map(|(tab_idx, tab)| {
                    tab.collect_panes()
                        .into_iter()
                        .find(|p| p.entity_id().as_u64() == pane_id)
                        .map(|p| (ws_idx, tab_idx, p))
                })
            })
        else {
            return;
        };

        if src_ws_idx == dest_ws_idx
            && self.workspaces[src_ws_idx]
                .tabs()
                .get(src_tab_idx)
                .is_some_and(|tab| tab.pane_count() <= 1)
        {
            return;
        }

        if !self
            .workspaces
            .get(dest_ws_idx)
            .is_some_and(|ws| ws.can_open_tab())
        {
            self.show_toast("Tab limit reached for this workspace", cx);
            return;
        }
        let source_tab = self.workspaces[src_ws_idx]
            .tabs()
            .get(src_tab_idx)
            .map(|tab| (tab.id, tab.pane_count() <= 1));
        let surfaces: std::collections::HashSet<u64> = pane
            .read(cx)
            .terminals()
            .map(|terminal| terminal.read(cx).surface_id())
            .collect();
        self.hand_surfaces_between(src_ws_idx, dest_ws_idx, &surfaces);
        self.adopt_layout(cx);
        let (Some(model_pane), Some(workspace)) = (
            self.layout_pane_id(&pane),
            self.layout_workspace_id(self.workspaces[dest_ws_idx].id),
        ) else {
            return;
        };
        let new_tab = paneflow_layout::TabId(self.reserve_layout_id());
        if self
            .apply_layout_op(
                paneflow_layout::Op::MovePaneToNewTab {
                    pane: model_pane,
                    workspace,
                    tab: new_tab,
                    index: Some(insert_idx),
                },
                cx,
            )
            .is_err()
        {
            return;
        }
        let Some(dest_ws_idx) = self
            .workspaces
            .iter()
            .position(|ws| self.layout_workspace_id(ws.id) == Some(workspace))
        else {
            return;
        };
        if let (Some((from, true)), Some(to)) = (source_tab, self.layout_app_tab_id(new_tab)) {
            self.transfer_diff_dock(from, to);
        }
        self.workspaces[dest_ws_idx].sidebar_expanded = true;
        let dest_tab_idx = self.workspaces[dest_ws_idx].active_tab_idx();
        self.focus_workspace_tab(dest_ws_idx, dest_tab_idx, window, cx);
    }
}

fn sole_terminal(tab: &Tab, cx: &App) -> Option<Entity<TerminalView>> {
    let mut terminals = tab
        .collect_panes()
        .into_iter()
        .flat_map(|pane| pane.read(cx).terminals().cloned().collect::<Vec<_>>());
    let first = terminals.next()?;
    terminals.next().is_none().then_some(first)
}

fn move_panes_to_workspace(
    panes: &[Entity<Pane>],
    workspace_id: u64,
    cx: &mut App,
) -> Vec<paneflow_config::schema::SessionId> {
    let mut sessions = Vec::new();
    for pane in panes {
        let terminals: Vec<Entity<TerminalView>> = pane.read(cx).terminals().cloned().collect();
        for terminal in terminals {
            sessions.push(terminal.update(cx, |view, _| {
                view.move_to_workspace(workspace_id);
                view.terminal.session_id.clone()
            }));
        }
        pane.update(cx, |pane, cx| {
            pane.workspace_id = workspace_id;
            cx.notify();
        });
    }
    sessions
}

fn sync_terminal_visibility(
    workspaces: &[crate::workspace::Workspace],
    shown: Option<usize>,
    cx: &App,
) {
    for (ws_idx, ws) in workspaces.iter().enumerate() {
        let active_tab = ws.active_tab_idx();
        for (tab_idx, tab) in ws.tabs().iter().enumerate() {
            let tab_shown = shown == Some(ws_idx) && tab_idx == active_tab;
            for pane_entity in tab.collect_panes() {
                let pane = pane_entity.read(cx);
                let pane_shown = pane.detached.is_some()
                    || (tab_shown
                        && tab
                            .root
                            .as_ref()
                            .is_some_and(|root| root.contains_leaf(&pane_entity)));
                let active = pane.active_surface_idx();
                for (idx, surface) in pane.surfaces().iter().enumerate() {
                    if let Some(terminal) = surface.as_terminal() {
                        terminal.read(cx).set_shown(pane_shown && idx == active);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn only_the_front_surface_of_the_shown_tab_publishes_at_full_rate(
        cx: &mut gpui::TestAppContext,
    ) {
        let cx = cx.add_empty_window();
        let terminal = |cx: &mut gpui::VisualTestContext| {
            cx.new(|cx| TerminalView::display_only_for_test(1, cx))
        };
        let (front, behind, other_tab, other_workspace) =
            (terminal(cx), terminal(cx), terminal(cx), terminal(cx));
        let stacked = cx.new(|cx| Pane::new(behind.clone(), 1, cx));
        stacked.update(cx, |pane, cx| {
            pane.push_surface(crate::pane::PaneSurface::Terminal(front.clone()), cx);
        });
        let second_tab_pane = cx.new(|cx| Pane::new(other_tab.clone(), 1, cx));
        let second_workspace_pane = cx.new(|cx| Pane::new(other_workspace.clone(), 2, cx));
        let mut first = crate::workspace::Workspace::with_id(1, "first", stacked.clone());
        assert!(first.open_tab(crate::workspace::Tab::new(
            "second",
            Some(LayoutTree::Leaf(second_tab_pane))
        )));
        first.set_active_tab(0);
        let workspaces = vec![
            first,
            crate::workspace::Workspace::with_id(2, "second", second_workspace_pane),
        ];
        let shown = |cx: &mut gpui::VisualTestContext| {
            cx.update(|_, cx| {
                [&front, &behind, &other_tab, &other_workspace]
                    .map(|terminal| terminal.read(cx).is_shown())
            })
        };

        cx.update(|_, cx| sync_terminal_visibility(&workspaces, Some(0), cx));
        assert_eq!(shown(cx), [true, false, false, false]);

        cx.update(|_, cx| sync_terminal_visibility(&workspaces, Some(1), cx));
        assert_eq!(shown(cx), [false, false, false, true]);

        stacked.update(cx, |pane, cx| pane.activate_surface(0, cx));
        cx.update(|_, cx| sync_terminal_visibility(&workspaces, Some(0), cx));
        assert_eq!(shown(cx), [false, true, false, false]);
    }

    #[gpui::test]
    fn moved_panes_name_every_stacked_session_and_relaunch_in_the_new_workspace(
        cx: &mut gpui::TestAppContext,
    ) {
        let cx = cx.add_empty_window();
        let front = cx.new(|cx| TerminalView::display_only_for_test(1, cx));
        let behind = cx.new(|cx| TerminalView::display_only_for_test(1, cx));
        let other = cx.new(|cx| TerminalView::display_only_for_test(1, cx));
        let stacked = cx.new(|cx| Pane::new(behind.clone(), 1, cx));
        stacked.update(cx, |pane, cx| {
            pane.push_surface(crate::pane::PaneSurface::Terminal(front.clone()), cx);
        });
        let single = cx.new(|cx| Pane::new(other.clone(), 1, cx));

        let sessions =
            cx.update(|_, cx| move_panes_to_workspace(&[stacked.clone(), single.clone()], 7, cx));

        cx.update(|_, cx| {
            let mut expected: Vec<_> = [&behind, &front, &other]
                .iter()
                .map(|terminal| terminal.read(cx).terminal.session_id.clone())
                .collect();
            let mut got = sessions.clone();
            expected.sort_by_key(ToString::to_string);
            got.sort_by_key(ToString::to_string);
            assert_eq!(got, expected);
            for terminal in [&behind, &front, &other] {
                assert_eq!(terminal.read(cx).launch_workspace_id(), 7);
            }
            assert_eq!(stacked.read(cx).workspace_id, 7);
            assert_eq!(single.read(cx).workspace_id, 7);
        });
    }
}
