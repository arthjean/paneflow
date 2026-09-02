use gpui::{Context, Window};

use crate::{PaneFlowApp, SWAP_MODE, SwapPane};

impl PaneFlowApp {
    pub(crate) fn handle_swap_pane(
        &mut self,
        _: &SwapPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.swap_source.is_some() {
            self.swap_source = None;
            SWAP_MODE.store(false, std::sync::atomic::Ordering::Relaxed);
        } else if let Some(ws) = self.active_workspace()
            && let Some(root) = &ws.active_tab().root
            && root.leaf_count() > 1
            && let Some(pane) = root.focused_pane(window, cx)
        {
            self.swap_source = Some(pane);
            SWAP_MODE.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        cx.notify();
    }

    pub(crate) fn cancel_swap_mode(&mut self, cx: &mut Context<Self>) {
        if self.swap_source.is_some() {
            self.swap_source = None;
            SWAP_MODE.store(false, std::sync::atomic::Ordering::Relaxed);
            cx.notify();
        }
    }
}
