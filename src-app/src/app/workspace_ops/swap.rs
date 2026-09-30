use std::sync::atomic::Ordering;

use gpui::{Context, WeakEntity, Window, WindowId};

use crate::pane::Pane;
use crate::{PaneFlowApp, SWAP_WINDOW, SwapMode, SwapPane};

pub(crate) fn arm_swap_window(window: WindowId) {
    SWAP_WINDOW.store(window.as_u64(), Ordering::Relaxed);
}

pub(crate) fn disarm_swap_window() {
    SWAP_WINDOW.store(0, Ordering::Relaxed);
}

pub(crate) fn swap_mode_active_in(window: WindowId) -> bool {
    SWAP_WINDOW.load(Ordering::Relaxed) == window.as_u64()
}

impl PaneFlowApp {
    pub(crate) fn handle_swap_pane(
        &mut self,
        _: &SwapPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.end_swap_mode(cx).is_none()
            && let Some(root) = self.nav_root()
            && root.leaf_count() > 1
            && let Some(pane) = root.focused_pane(window, cx)
        {
            let released = cx.observe_release(&pane, |app, _, cx| app.cancel_swap_mode(cx));
            pane.update(cx, |pane, cx| pane.set_swap_source(true, cx));
            self.swap_mode = Some(SwapMode {
                source: pane.downgrade(),
                _source_released: released,
            });
            arm_swap_window(window.window_handle().window_id());
        }
        cx.notify();
    }

    pub(crate) fn end_swap_mode(&mut self, cx: &mut Context<Self>) -> Option<WeakEntity<Pane>> {
        let swap = self.swap_mode.take()?;
        disarm_swap_window();
        if let Some(source) = swap.source.upgrade() {
            source.update(cx, |pane, cx| pane.set_swap_source(false, cx));
        }
        Some(swap.source)
    }

    pub(crate) fn cancel_swap_mode(&mut self, cx: &mut Context<Self>) {
        if self.end_swap_mode(cx).is_some() {
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_mode_only_claims_escape_in_its_own_window() {
        let armed = WindowId::from(0x1_0000_0003);
        let other = WindowId::from(0x1_0000_0004);

        arm_swap_window(armed);
        assert!(swap_mode_active_in(armed));
        assert!(!swap_mode_active_in(other));

        disarm_swap_window();
        assert!(!swap_mode_active_in(armed));
    }
}
