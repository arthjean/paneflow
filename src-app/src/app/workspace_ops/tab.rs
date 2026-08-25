//! Workspace-tab handlers (add/close/select/rename/move) for `PaneFlowApp`.
//!
//! EP-002 US-005 (prd-cli-tab-hierarchy): `New tab` / `Close tab` used to add
//! and close a tab *inside* a pane. A pane is now mono-surface, so multiplicity
//! moved one level up: they operate on workspace tabs, and the workspace always
//! keeps at least one (FR-01).
//!
//! EP-003 (US-009 / US-010 / US-011) adds the sidebar-driven half of the same
//! lifecycle - select, create, close, inline rename, reorder, reattach - so the
//! keyboard actions and the sidebar rows share one implementation instead of
//! two drifting ones.

use gpui::{AppContext, Context, Window};
use paneflow_config::schema::TerminalSurfaceProfile;

use crate::PaneFlowApp;
use crate::layout::LayoutTree;
use crate::terminal::TerminalView;
use crate::workspace::Tab;
use crate::{CloseTab, NewTab, NextTab, PreviousTab, TabDrag};

impl PaneFlowApp {
    /// US-008: toggle the sidebar folder row for `ws_idx`. Session-only state,
    /// so nothing is persisted.
    pub(crate) fn toggle_workspace_expanded(&mut self, ws_idx: usize, cx: &mut Context<Self>) {
        if let Some(ws) = self.workspaces.get_mut(ws_idx) {
            ws.sidebar_expanded = !ws.sidebar_expanded;
            cx.notify();
        }
    }

    /// The single "open a tab holding a fresh surface" path: build the pane,
    /// hand it to the workspace, focus it, and - when the caller supplied one -
    /// write the launch command once the tab is actually in place. Returns
    /// `false` (after a toast) when the workspace is at
    /// [`crate::workspace::MAX_TABS_PER_WORKSPACE`].
    ///
    /// EP-005: `profile` and `command` are passed in rather than derived from
    /// an agent, so the preset palette opens a shell, an agent, an agent
    /// variant or a custom command through one implementation.
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
        let cwd = (!ws.cwd.is_empty()).then(|| std::path::PathBuf::from(&ws.cwd));
        // A preset label reaches the sidebar verbatim, and a custom command's
        // name is user input: strip CLI decoration (spinners, zero-width
        // glyphs) the way every other title path does.
        let title = crate::project::clean_sidebar_title(&title).unwrap_or_default();
        let terminal =
            cx.new(|cx| TerminalView::with_cwd_and_profile(ws_id, cwd, None, profile, cx));
        cx.subscribe(&terminal, Self::handle_terminal_event)
            .detach();
        let pane = self.create_pane(terminal.clone(), ws_id, cx);
        let root = LayoutTree::Leaf(pane);
        // EP-005: the palette is the content of an empty tab, so the preset
        // fills *that* tab rather than opening a second one behind it. Any
        // other paneless tab (last pane closed) is filled the same way.
        let opened = self.workspaces.get_mut(ws_idx).is_some_and(|ws| {
            let active = ws.active_tab_mut();
            if active.root.is_none() && active.saved_layout.is_none() {
                active.title = title;
                active.root = Some(root);
                true
            } else {
                ws.open_tab(Tab::new(title, Some(root)))
            }
        });
        if !opened {
            // Tab cap reached: `open_tab` already logged and mutated nothing,
            // so the freshly built pane is simply dropped - and with it the
            // terminal, which is why the launch command is only written below.
            self.show_toast("Tab limit reached for this workspace", cx);
            return false;
        }
        if let Some(command) = command {
            // Safe before the PTY is live: `send_command` buffers into the
            // display-only terminal's pending input and `TerminalState::promote`
            // flushes it when the real PTY arrives (US-012), the same contract
            // the Agents view relies on.
            terminal.read(cx).send_command(&command);
        }
        if let Some(ws) = self.workspaces.get_mut(ws_idx) {
            // A tab created from a collapsed folder row must be visible.
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
        // EP-005: `New tab` is the palette's primary entry point - the user
        // picks *what* to launch before a surface exists, instead of getting a
        // bare shell and reaching for an agent afterwards.
        self.open_pane_palette(self.active_idx, window, cx);
    }

    /// US-020: cycle to the next tab of the active workspace, wrapping around.
    /// A single-tab workspace is a no-op, which keeps the shortcut harmless
    /// when the hierarchy is not in use.
    pub(crate) fn handle_next_tab(
        &mut self,
        _: &NextTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_active_workspace_tab(1, window, cx);
    }

    /// US-020: cycle to the previous tab of the active workspace, wrapping.
    pub(crate) fn handle_previous_tab(
        &mut self,
        _: &PreviousTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_active_workspace_tab(-1, window, cx);
    }

    /// Shared body of the two cycling shortcuts. Routes through
    /// `focus_workspace_tab`, the single path every "this tab is now visible"
    /// caller uses, so focus, zoom restore and persistence behave exactly as
    /// they do when the tab is picked in the sidebar.
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
        // `focus_workspace_tab` already persists the new active tab.
        self.focus_workspace_tab(self.active_idx, next, window, cx);
        cx.notify();
    }

    /// US-009: make `tab_idx` of `ws_idx` the visible tab, activating the
    /// workspace first when it is not the active one, then focusing the tab's
    /// first pane.
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
        if let Some(ws) = self.workspaces.get_mut(ws_idx) {
            ws.agent_completion_notification.acknowledge();
        }
        self.focus_workspace_tab(ws_idx, tab_idx, window, cx);
        cx.notify();
    }

    /// Shared tail of every "this tab is now the one you look at" path: set the
    /// active tab *before* activating the workspace, so the workspace
    /// activation (focus, files tree, sessions sidebar, diff reconcile) already
    /// sees the right tab.
    pub(crate) fn focus_workspace_tab(
        &mut self,
        ws_idx: usize,
        tab_idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ws) = self.workspaces.get_mut(ws_idx) {
            ws.set_active_tab(tab_idx);
        }
        if ws_idx == self.active_idx {
            self.workspaces[ws_idx].focus_first(window, cx);
            self.save_session(cx);
        } else {
            self.select_workspace(ws_idx, window, cx);
        }
    }

    /// US-010: close one tab of one workspace. Closing the last tab leaves an
    /// empty tab behind (FR-01) and never closes the workspace. The removed
    /// `Tab` drops here, which drops its pane entities and their terminals -
    /// the same teardown path a pane close uses, so no PTY is orphaned.
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
            self.rename_text.clear();
        }
        self.dismiss_transient_surfaces();
        if ws_idx == self.active_idx {
            self.workspaces[ws_idx].focus_first(window, cx);
        }
        self.save_session(cx);
        cx.notify();
        // The closed tab's panes may have carried a Composer target, queued
        // prompts, or group memberships - refresh the same way a workspace
        // close does so nothing points at a dropped terminal.
        self.refresh_composer_slot(cx);
        self.sync_broadcast_stripes(cx);
        self.flush_pending_prefill(cx);
        self.sync_pending_chips(cx);
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

    /// US-010: start the inline rename of a sidebar tab row. Mirrors
    /// `begin_workspace_rename`: any live rename commits first, and the input
    /// seeds with the tab's current title.
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
            .map(|tab| tab.title.clone())
        else {
            return;
        };
        self.rename_text = title;
        self.renaming_tab = Some((ws_idx, tab_idx));
        cx.notify();
    }

    /// US-011: reorder a dragged tab inside its own workspace. `target_idx` is
    /// the index of the row it was dropped on; the insertion side matches the
    /// workspace-row drop-edge convention (drop below when dragging down).
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
        // Re-resolve by id: the sidebar may have re-rendered since the drag
        // started, so the captured index can be stale.
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

    /// US-011: reattach a dragged tab to another workspace, keeping its pane
    /// tree and its live terminals. Refused - with the tab left untouched, so
    /// nothing is killed - when the destination is already at
    /// [`crate::workspace::MAX_TABS_PER_WORKSPACE`].
    ///
    /// `insert_idx` is the gap the sidebar's insertion line pointed at, so the
    /// tab lands where the line showed it rather than at the end of the list.
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
            // Cap reached: bail *before* detaching, so the tab stays where it
            // is with every terminal alive.
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
        // The panes move with the tab, so their workspace identity must move
        // too: port scans, agent sessions and IPC surface lookups are all keyed
        // by the pane's workspace id. The terminal's own cwd is untouched - a
        // tab dropped on a workspace with a different cwd keeps running where
        // it was started.
        for pane in tab.collect_panes() {
            pane.update(cx, |pane, cx| {
                pane.workspace_id = dest_id;
                cx.notify();
            });
        }
        if !self.workspaces[dest_ws_idx].open_tab(tab) {
            // Unreachable: the cap was checked above and nothing else can have
            // opened a tab in between. Kept as a fail-safe rather than a panic.
            log::warn!("tab move: destination refused the tab after the cap check");
            return;
        }
        // `open_tab` appends; slide the newcomer to the gap the line marked.
        // `reorder_tab` re-resolves the active tab by id, so the moved tab
        // stays the visible one.
        let last = self.workspaces[dest_ws_idx].tab_count().saturating_sub(1);
        self.workspaces[dest_ws_idx].reorder_tab(last, insert_idx.min(last));
        self.renaming_tab = None;
        self.rename_text.clear();
        self.workspaces[dest_ws_idx].sidebar_expanded = true;
        let dest_tab_idx = self.workspaces[dest_ws_idx].active_tab_idx();
        self.focus_workspace_tab(dest_ws_idx, dest_tab_idx, window, cx);
        self.save_session(cx);
        cx.notify();
    }

    /// Sidebar drop target for a dragged pane: detach it from wherever it sits
    /// and reopen it as a brand-new tab of `dest_ws_idx` (the folder row it was
    /// dropped on).
    ///
    /// The pane entity survives the detach - this handler holds a strong
    /// handle throughout - so its terminal keeps running across the move; only
    /// the tree it hangs from changes. The source is re-resolved by entity id
    /// because the layout re-renders during the drag, which invalidates any
    /// index captured when the gesture started. `insert_idx` is the gap the
    /// sidebar's insertion line pointed at.
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

        // Already alone in a tab of that same workspace: the move would close
        // one tab only to reopen an identical one, losing the tab's title on
        // the way.
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
            // Cap reached: bail *before* detaching, so the pane stays where it
            // is with its terminal alive.
            self.show_toast("Tab limit reached for this workspace", cx);
            return;
        }

        // Zoom parks the real tree in `saved_layout` and leaves the zoomed pane
        // alone in `root`. Detaching from that root would strand the siblings,
        // so leave zoom first and move from the restored tree.
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
            // Stale drag: `remove_pane` hands the tree back intact, so the tab
            // is restored exactly as it was rather than left rootless.
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
            // The pane *was* the whole tab, so the now-empty tab leaves with
            // it. `close_tab` keeps the workspace's last-tab placeholder
            // (FR-01), which `open_tab` fills in place when the destination is
            // this same workspace.
            None => {
                self.workspaces[src_ws_idx].close_tab(src_tab_idx);
                // Every tab after the closed one slid up by one, and so did the
                // gap the line pointed at.
                if src_ws_idx == dest_ws_idx && src_tab_idx < insert_idx {
                    insert_idx -= 1;
                }
            }
        }

        // Port scans, agent sessions and IPC surface lookups are all keyed by
        // the pane's workspace id, so a cross-workspace move must carry it.
        let dest_id = self.workspaces[dest_ws_idx].id;
        pane.update(cx, |pane, cx| {
            pane.workspace_id = dest_id;
            cx.notify();
        });

        if !self.open_pane_in_new_workspace_tab(dest_ws_idx, pane.clone(), cx) {
            // Unreachable: the cap was checked above and the detach can only
            // have freed a slot. Re-attach rather than orphan the pane - its
            // terminal would keep running with no way back to it.
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

        // `open_tab` appends; slide the newcomer to the gap the line marked.
        let last = self.workspaces[dest_ws_idx].tab_count().saturating_sub(1);
        self.workspaces[dest_ws_idx].reorder_tab(last, insert_idx.min(last));
        self.workspaces[dest_ws_idx].sidebar_expanded = true;
        let dest_tab_idx = self.workspaces[dest_ws_idx].active_tab_idx();
        self.focus_workspace_tab(dest_ws_idx, dest_tab_idx, window, cx);
        self.save_session(cx);
        cx.notify();
    }
}
