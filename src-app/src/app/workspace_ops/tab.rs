use gpui::{AppContext, Context, Window};
use paneflow_config::schema::{TabTitleSource, TerminalSurfaceProfile};

use crate::PaneFlowApp;
use crate::layout::LayoutTree;
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

    pub(crate) fn open_tab_with_surface(
        &mut self,
        ws_idx: usize,
        title: String,
        profile: TerminalSurfaceProfile,
        command: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(ws) = self.workspaces.get(ws_idx) else {
            return false;
        };
        let ws_id = ws.id;
        let fills_active_tab =
            ws.active_tab().root.is_none() && ws.active_tab().saved_layout.is_none();
        let cwd = fills_active_tab
            .then(|| ws.active_tab().worktree.clone())
            .flatten()
            .or_else(|| (!ws.cwd.is_empty()).then(|| std::path::PathBuf::from(&ws.cwd)));
        let title = crate::sidebar_title::clean_sidebar_title(&title).unwrap_or_default();
        let terminal =
            cx.new(|cx| TerminalView::with_cwd_and_profile(ws_id, cwd, None, profile, cx));
        cx.subscribe(&terminal, Self::handle_terminal_event)
            .detach();
        let pane = self.create_pane(terminal.clone(), ws_id, cx);
        let root = LayoutTree::Leaf(pane);
        let opened = self.workspaces.get_mut(ws_idx).is_some_and(|ws| {
            let active = ws.active_tab_mut();
            if active.root.is_none() && active.saved_layout.is_none() {
                active.set_title(&title, TabTitleSource::Preset);
                active.root = Some(root);
                true
            } else {
                ws.open_tab(Tab::new(title, Some(root)))
            }
        });
        if !opened {
            self.show_toast("Tab limit reached for this workspace", cx);
            return false;
        }
        if let Some(command) = command {
            terminal.read(cx).send_command(&command);
            terminal.update(cx, |view, _cx| view.declare_agent_from_command(&command));
        }
        if let Some(ws) = self.workspaces.get_mut(ws_idx) {
            ws.sidebar_expanded = true;
        }
        let tab_idx = self.workspaces[ws_idx].active_tab_idx();
        self.focus_workspace_tab(ws_idx, tab_idx, window, cx);
        self.save_session(cx);
        cx.notify();
        true
    }

    pub(crate) fn handle_new_tab(
        &mut self,
        _: &NewTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_pane_palette(self.active_idx, window, cx);
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
        self.focus_workspace_tab(self.active_idx, next, window, cx);
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
        let checkout_before = self.active_checkout();
        if let Some(ws) = self.workspaces.get_mut(ws_idx) {
            ws.set_active_tab(tab_idx);
        }
        if ws_idx == self.active_idx {
            self.workspaces[ws_idx].focus_first(window, cx);
            self.save_session(cx);
            if self.active_checkout() != checkout_before {
                self.reconcile_diff_after_workspace_change(cx);
            }
        } else {
            self.select_workspace(ws_idx, window, cx);
        }
        self.acknowledge_visible_completions(cx);
    }

    pub(crate) fn acknowledge_visible_completions(&mut self, cx: &mut Context<Self>) {
        let Some(ws) = self.workspaces.get(self.active_idx) else {
            return;
        };
        if !ws.agent_completion_notification.is_unread() {
            return;
        }
        let seen = ws.active_tab().surface_ids(cx);
        let live: std::collections::HashSet<u64> = ws
            .tabs()
            .iter()
            .flat_map(|tab| tab.surface_ids(cx))
            .collect();
        if let Some(ws) = self.workspaces.get_mut(self.active_idx) {
            ws.agent_completion_notification.acknowledge(&seen, &live);
        }
        cx.notify();
    }

    pub(crate) fn close_workspace_tab(
        &mut self,
        ws_idx: usize,
        tab_idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.workspaces.get_mut(ws_idx) else {
            return;
        };
        if ws.close_tab(tab_idx).is_none() {
            return;
        }
        if self.renaming_tab.is_some_and(|(w, _)| w == ws_idx) {
            self.renaming_tab = None;
        }
        self.dismiss_transient_surfaces();
        if ws_idx == self.active_idx {
            self.workspaces[ws_idx].focus_first(window, cx);
        }
        self.save_session(cx);
        cx.notify();
        self.refresh_composer_slot(cx);
        self.sync_broadcast_stripes(cx);
        self.flush_pending_prefill(cx);
        self.sync_pending_chips(cx);
        self.prune_parked_diff_docks();
        self.prune_worktree_states();
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
        self.close_workspace_tab(self.active_idx, tab_idx, window, cx);
    }

    pub(crate) fn begin_tab_rename(
        &mut self,
        ws_idx: usize,
        tab_idx: usize,
        cx: &mut Context<Self>,
    ) {
        self.commit_rename(cx);
        let Some(title) = self
            .workspaces
            .get(ws_idx)
            .and_then(|ws| ws.tabs().get(tab_idx))
            .map(|tab| tab.title().to_string())
        else {
            return;
        };
        self.rename_input
            .update(cx, |input, cx| input.set_value(title, cx));
        self.renaming_tab = Some((ws_idx, tab_idx));
        cx.notify();
    }

    pub(crate) fn reset_tab_name(&mut self, ws_idx: usize, tab_idx: usize, cx: &mut Context<Self>) {
        if self
            .workspaces
            .get_mut(ws_idx)
            .and_then(|ws| ws.tab_mut(tab_idx))
            .is_some_and(Tab::unlock_title)
        {
            self.save_session(cx);
            cx.notify();
        }
    }

    pub(crate) fn reorder_workspace_tab(
        &mut self,
        drag: &TabDrag,
        target_ws_idx: usize,
        target_idx: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.workspaces.get_mut(target_ws_idx) else {
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
        ws.reorder_tab(from, target_idx);
        self.save_session(cx);
        cx.notify();
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
        let dest_id = dest.id;
        let Some(tab_idx) = self
            .workspaces
            .get(source_ws_idx)
            .and_then(|ws| ws.tabs().iter().position(|tab| tab.id == drag.tab_id))
        else {
            return;
        };
        let Some(tab) = self.workspaces[source_ws_idx].close_tab(tab_idx) else {
            return;
        };
        for pane in tab.collect_panes() {
            pane.update(cx, |pane, cx| {
                pane.workspace_id = dest_id;
                cx.notify();
            });
        }
        if !self.workspaces[dest_ws_idx].open_tab(tab) {
            log::warn!("tab move: destination refused the tab after the cap check");
            return;
        }
        let last = self.workspaces[dest_ws_idx].tab_count().saturating_sub(1);
        self.workspaces[dest_ws_idx].reorder_tab(last, insert_idx.min(last));
        self.renaming_tab = None;
        self.workspaces[dest_ws_idx].sidebar_expanded = true;
        let dest_tab_idx = self.workspaces[dest_ws_idx].active_tab_idx();
        self.focus_workspace_tab(dest_ws_idx, dest_tab_idx, window, cx);
        self.save_session(cx);
        cx.notify();
    }

    pub(crate) fn move_pane_to_new_tab(
        &mut self,
        pane_id: u64,
        dest_ws_idx: usize,
        mut insert_idx: usize,
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

        if self.workspaces[src_ws_idx]
            .tabs()
            .get(src_tab_idx)
            .is_some_and(|tab| tab.is_zoomed())
            && let Some(tab) = self.workspaces[src_ws_idx].tab_mut(src_tab_idx)
        {
            tab.exit_zoom(cx);
        }

        let Some(tree) = self.workspaces[src_ws_idx]
            .tab_mut(src_tab_idx)
            .and_then(|tab| tab.root.take())
        else {
            return;
        };
        let (pruned, removed) = tree.remove_pane(&pane);
        if !removed {
            if let Some(tab) = self.workspaces[src_ws_idx].tab_mut(src_tab_idx) {
                tab.root = pruned;
            }
            return;
        }
        match pruned {
            Some(rest) => {
                if let Some(tab) = self.workspaces[src_ws_idx].tab_mut(src_tab_idx) {
                    tab.root = Some(rest);
                }
            }
            None => {
                self.workspaces[src_ws_idx].close_tab(src_tab_idx);
                if src_ws_idx == dest_ws_idx && src_tab_idx < insert_idx {
                    insert_idx -= 1;
                }
            }
        }

        let dest_id = self.workspaces[dest_ws_idx].id;
        pane.update(cx, |pane, cx| {
            pane.workspace_id = dest_id;
            cx.notify();
        });

        if !self.open_pane_in_new_workspace_tab(dest_ws_idx, pane.clone(), cx) {
            log::warn!("pane move: destination refused the tab after the cap check");
            let reattached = self.workspaces[src_ws_idx]
                .tab_mut(src_tab_idx)
                .and_then(|tab| tab.root.as_mut())
                .is_some_and(|root| {
                    root.first_leaf().is_some_and(|anchor| {
                        root.split_at_pane(&anchor, crate::layout::SplitDirection::Vertical, pane)
                    })
                });
            if !reattached {
                log::error!("pane move: dropped pane could not be re-attached");
            }
            cx.notify();
            return;
        }

        let last = self.workspaces[dest_ws_idx].tab_count().saturating_sub(1);
        self.workspaces[dest_ws_idx].reorder_tab(last, insert_idx.min(last));
        self.workspaces[dest_ws_idx].sidebar_expanded = true;
        let dest_tab_idx = self.workspaces[dest_ws_idx].active_tab_idx();
        self.focus_workspace_tab(dest_ws_idx, dest_tab_idx, window, cx);
        self.save_session(cx);
        cx.notify();
    }
}
