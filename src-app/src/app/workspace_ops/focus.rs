use gpui::{Context, Focusable, Window};

use super::WorkspaceFocusTarget;
use crate::PaneFlowApp;
use crate::layout::{FocusDirection, FocusNav};
use crate::{FocusDown, FocusLeft, FocusRight, FocusUp, JumpNextWaiting, SWAP_MODE};

impl PaneFlowApp {
    pub(crate) fn handle_focus(
        &mut self,
        dir: FocusDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(source) = self.swap_source.take() {
            SWAP_MODE.store(false, std::sync::atomic::Ordering::Relaxed);

            if let Some(ws) = self.active_workspace()
                && let Some(root) = &ws.active_tab().root
            {
                let moved = matches!(root.focus_in_direction(dir, window, cx), FocusNav::Moved);
                if let Some(target) = root.focused_pane(window, cx)
                    && target != source
                {
                    let swapped = if let Some(ws) = self.active_workspace_mut()
                        && let Some(ref mut root) = ws.active_tab_mut().root
                    {
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

        if let Some(ws) = self.active_workspace()
            && let Some(root) = &ws.active_tab().root
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
        self.jump_next_session_where(
            |s| *s == crate::ai_types::AgentState::WaitingForInput,
            window,
            cx,
        );
    }

    pub(crate) fn jump_next_session_where(
        &mut self,
        state_matches: impl Fn(&crate::ai_types::AgentState) -> bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut order: Vec<(usize, gpui::Entity<crate::pane::Pane>, u64)> = Vec::new();
        for (ws_idx, ws) in self.workspaces.iter().enumerate() {
            let matching: std::collections::HashSet<u64> = ws
                .agent_sessions
                .values()
                .filter(|s| state_matches(&s.state))
                .filter_map(|s| s.surface_id)
                .collect();
            if matching.is_empty() {
                continue;
            }
            if let Some(root) = &ws.active_tab().root {
                for pane in root.collect_leaves() {
                    if let Some(t) = pane.read(cx).active_terminal_opt() {
                        let sid = t.entity_id().as_u64();
                        if matching.contains(&sid) {
                            order.push((ws_idx, pane.clone(), sid));
                        }
                    }
                }
            }
        }
        let ids: Vec<u64> = order.iter().map(|(_, _, sid)| *sid).collect();
        let Some(next) = next_in_cycle(&ids, self.jump_cursor) else {
            return;
        };
        let Some((ws_idx, pane, sid)) = order.into_iter().find(|(_, _, s)| *s == next) else {
            return;
        };
        self.activate_workspace_at(ws_idx, WorkspaceFocusTarget::Pane { pane }, window, cx);
        self.jump_cursor = Some(sid);
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
