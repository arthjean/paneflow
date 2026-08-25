//! Lifecycle of the diff dock's tabs: opening a terminal tab from the `+`
//! menu, selecting a tab, and closing one.
//!
//! The `Changes` tab is permanent and always index 0, so a dock that has never
//! been given a second tab behaves exactly as before this strip existed.

use gpui::{AppContext, Context, Entity, Focusable, Window};

use super::model::DiffDockTab;
use crate::PaneFlowApp;
use crate::terminal::{TerminalEvent, TerminalView};

impl PaneFlowApp {
    /// Open a terminal tab in the dock and focus it. The shell lands in the
    /// folder the dock is diffing, falling back to the active workspace root
    /// (the same chain `new_terminal_cwd` uses for a split).
    pub(crate) fn open_diff_terminal_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws) = self.active_workspace() else {
            return;
        };
        let ws_id = ws.id;
        let cwd = self
            .agents_view
            .agents_diff
            .as_ref()
            .map(|data| data.cwd.clone())
            .filter(|cwd| !cwd.is_empty())
            .map(std::path::PathBuf::from);
        let cwd = self.new_terminal_cwd(cwd);

        let terminal = cx.new(|cx| TerminalView::with_cwd(ws_id, cwd, None, cx));
        // Only the exit is wired: the dock terminal has no pane in the layout
        // tree, so the app-level CWD / port-scan / open-path handlers have
        // nothing to act on for it.
        cx.subscribe(
            &terminal,
            |this, terminal: Entity<TerminalView>, event: &TerminalEvent, cx| {
                if matches!(event, TerminalEvent::ChildExited) {
                    this.close_diff_terminal_tab(&terminal, cx);
                }
            },
        )
        .detach();

        let focus = terminal.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        self.agents_view
            .diff_tabs
            .push(DiffDockTab::Terminal(terminal));
        self.agents_view.diff_active_tab = self.agents_view.diff_tabs.len() - 1;
        cx.notify();
    }

    pub(crate) fn select_diff_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.agents_view.diff_tabs.len() && self.agents_view.diff_active_tab != index {
            self.agents_view.diff_active_tab = index;
            cx.notify();
        }
    }

    /// Close the tab at `index`. Index 0 (`Changes`) is permanent, so the call
    /// is a no-op there. The selection falls back to the previous tab.
    pub(crate) fn close_diff_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index == 0 || index >= self.agents_view.diff_tabs.len() {
            return;
        }
        self.agents_view.diff_tabs.remove(index);
        if self.agents_view.diff_active_tab >= index {
            self.agents_view.diff_active_tab = self.agents_view.diff_active_tab.saturating_sub(1);
        }
        cx.notify();
    }

    /// Close whichever tab hosts `terminal` (the shell exited under it).
    fn close_diff_terminal_tab(&mut self, terminal: &Entity<TerminalView>, cx: &mut Context<Self>) {
        let found = self
            .agents_view
            .diff_tabs
            .iter()
            .position(|tab| matches!(tab, DiffDockTab::Terminal(t) if t == terminal));
        if let Some(index) = found {
            self.close_diff_tab(index, cx);
        }
    }
}
