use gpui::{Context, Focusable, Window};

use crate::PaneFlowApp;
use crate::layout::{FocusDirection, FocusNav, LayoutTree};
use crate::{FocusDown, FocusLeft, FocusReturn, FocusRight, FocusUp, JumpNextWaiting};

impl PaneFlowApp {
    pub(crate) fn nav_root(&self) -> Option<&LayoutTree> {
        self.active_workspace()
            .and_then(|ws| ws.active_tab().root.as_ref())
    }

    pub(crate) fn nav_root_mut(&mut self) -> Option<&mut LayoutTree> {
        self.active_workspace_mut()
            .and_then(|ws| ws.active_tab_mut().root.as_mut())
    }

    pub(crate) fn take_nav_root(&mut self) -> Option<LayoutTree> {
        self.active_workspace_mut()
            .and_then(|ws| ws.active_tab_mut().root.take())
    }

    pub(crate) fn put_nav_root(&mut self, root: Option<LayoutTree>) {
        if let Some(ws) = self.active_workspace_mut() {
            ws.active_tab_mut().root = root;
        }
    }

    pub(crate) fn pane_grid_mounted(&self) -> bool {
        self.settings_section.is_none()
            && self.diff_dock.maximized.is_none()
            && self
                .active_workspace()
                .is_some_and(|ws| ws.active_tab().root.is_some())
    }

    pub(crate) fn focus_active_pane(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pane_grid_mounted()
            && let Some(ws) = self.active_workspace()
        {
            ws.focus_first(window, cx);
        }
    }

    pub(crate) fn return_focus(
        &self,
        origin: &FocusReturn,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !origin.restore(window, cx) {
            self.focus_active_pane(window, cx);
        }
    }

    pub(crate) fn handle_focus_lost(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match window.focus_lost_restore_target(cx) {
            Some(ancestor) => window.focus(&ancestor, cx),
            None => self.focus_active_pane(window, cx),
        }
    }

    pub(crate) fn exit_nav_zoom(&mut self, cx: &mut Context<Self>) {
        if let Some(ws) = self.active_workspace_mut() {
            ws.exit_zoom(cx);
        }
    }

    pub(crate) fn handle_focus(
        &mut self,
        dir: FocusDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(source) = self.end_swap_mode(cx) {
            let Some(source) = source.upgrade() else {
                self.show_toast("Swap source pane is no longer available", cx);
                cx.notify();
                return;
            };
            if let Some(root) = self.nav_root() {
                let moved = matches!(root.focus_in_direction(dir, window, cx), FocusNav::Moved);
                if let Some(target) = root.focused_pane(window, cx)
                    && target != source
                {
                    let swapped = if let Some(root) = self.nav_root_mut() {
                        root.swap_panes(&source, &target)
                    } else {
                        false
                    };
                    if swapped {
                        source.read(cx).focus_handle(cx).focus(window, cx);
                    } else {
                        self.show_toast("Swap source pane is no longer available", cx);
                    }
                } else if !moved {
                    self.show_toast("No pane in that direction", cx);
                }
            }
            self.save_session(cx);
            cx.notify();
            return;
        }

        if let Some(root) = self.nav_root()
            && !matches!(root.focus_in_direction(dir, window, cx), FocusNav::Moved)
        {
            self.show_toast("No pane in that direction", cx);
        }
        cx.notify();
    }

    pub(crate) fn handle_focus_left(
        &mut self,
        _: &FocusLeft,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_focus(FocusDirection::Left, w, cx);
    }
    pub(crate) fn handle_focus_right(
        &mut self,
        _: &FocusRight,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_focus(FocusDirection::Right, w, cx);
    }
    pub(crate) fn handle_focus_up(&mut self, _: &FocusUp, w: &mut Window, cx: &mut Context<Self>) {
        self.handle_focus(FocusDirection::Up, w, cx);
    }
    pub(crate) fn handle_focus_down(
        &mut self,
        _: &FocusDown,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_focus(FocusDirection::Down, w, cx);
    }

    pub(crate) fn handle_jump_next_waiting(
        &mut self,
        _: &JumpNextWaiting,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ids: Vec<u64> = crate::app::sidebar::agents::awaiting_agent_surfaces(
            &self.workspaces,
            self.active_space.as_ref(),
            cx,
        )
        .into_iter()
        .filter(|&surface_id| {
            crate::app::ipc_handler::find_pane_by_surface_id(&self.workspaces, surface_id, cx)
                .is_some()
        })
        .collect();
        let Some(next) = next_in_cycle(&ids, self.jump_cursor) else {
            return;
        };
        self.activate_attention_surface(next, window, cx);
    }
}

fn next_in_cycle(order: &[u64], last: Option<u64>) -> Option<u64> {
    if order.is_empty() {
        return None;
    }
    match last.and_then(|l| order.iter().position(|&x| x == l)) {
        Some(pos) => Some(order[(pos + 1) % order.len()]),
        None => Some(order[0]),
    }
}

#[cfg(test)]
mod tests {
    use super::next_in_cycle;

    #[test]
    fn empty_set_is_none() {
        assert_eq!(next_in_cycle(&[], None), None);
        assert_eq!(next_in_cycle(&[], Some(7)), None);
    }

    #[test]
    fn unset_or_stale_cursor_starts_at_first() {
        assert_eq!(next_in_cycle(&[10, 20, 30], None), Some(10));
        assert_eq!(next_in_cycle(&[10, 20, 30], Some(99)), Some(10));
    }

    #[test]
    fn cycles_and_wraps() {
        assert_eq!(next_in_cycle(&[10, 20, 30], Some(10)), Some(20));
        assert_eq!(next_in_cycle(&[10, 20, 30], Some(30)), Some(10));
        assert_eq!(next_in_cycle(&[10], Some(10)), Some(10));
    }
}
