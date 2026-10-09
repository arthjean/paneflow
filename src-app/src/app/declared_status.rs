use gpui::{Context, Entity};

use crate::PaneFlowApp;
use crate::terminal::TerminalView;

impl PaneFlowApp {
    pub(crate) fn refresh_declared_status(&mut self, cx: &mut Context<Self>) {
        for ws_idx in 0..self.workspaces.len() {
            for pane in self.workspaces[ws_idx].collect_panes() {
                let terminals: Vec<Entity<TerminalView>> =
                    pane.read(cx).terminals().cloned().collect();
                let mut pane_changed = false;
                for terminal in terminals {
                    let row = self.host_agents.row(&terminal.read(cx).terminal.session_id);
                    let declared = row.and_then(|row| row.declared_status.clone());
                    if terminal.read(cx).terminal.declared_status != declared {
                        terminal.update(cx, |view, _| view.terminal.declared_status = declared);
                        pane_changed = true;
                    }
                }
                if pane_changed {
                    pane.update(cx, |_, cx| cx.notify());
                }
            }
        }
    }
}
