//! Tab handlers (add/close) for `PaneFlowApp`.
//!
//! Part of the US-023 workspace_ops decomposition.

use gpui::{AppContext, Context, Focusable, Window};

use crate::PaneFlowApp;
use crate::terminal::TerminalView;
use crate::{CloseTab, NewTab};

impl PaneFlowApp {
    pub(crate) fn handle_new_tab(
        &mut self,
        _: &NewTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ws) = self.active_workspace()
            && let Some(root) = &ws.root
            && let Some(pane) = root.focused_pane(window, cx)
        {
            let ws_id = ws.id;
            let cwd = (!ws.cwd.is_empty()).then(|| std::path::PathBuf::from(&ws.cwd));
            let terminal = cx.new(|cx| TerminalView::with_cwd(ws_id, cwd, None, cx));
            cx.subscribe(&terminal, Self::handle_terminal_event)
                .detach();
            pane.update(cx, |p, cx| {
                p.add_tab(terminal, cx);
            });
            pane.read(cx).focus_handle(cx).focus(window, cx);
            self.save_session(cx);
            cx.notify();
        }
    }

    pub(crate) fn handle_close_tab(
        &mut self,
        _: &CloseTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ws) = self.active_workspace()
            && let Some(root) = &ws.root
            && let Some(pane) = root.focused_pane(window, cx)
        {
            // close_selected_tab emits PaneEvent::Remove if last tab,
            // which is handled by handle_pane_event via cx.subscribe.
            pane.update(cx, |p, cx| {
                p.close_selected_tab(cx);
            });
            // If pane still has tabs, refocus
            if !pane.read(cx).tabs.is_empty() {
                pane.read(cx).focus_handle(cx).focus(window, cx);
            } else if let Some(ws) = self.active_workspace()
                && let Some(root) = &ws.root
            {
                root.focus_first(window, cx);
            }
            self.save_session(cx);
            cx.notify();
        }
    }
}
