//! Focus-movement handlers for `PaneFlowApp`.
//!
//! Part of the US-023 workspace_ops decomposition - behaviour identical to
//! the pre-refactor `main.rs` implementation.

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
        // When swap mode is active, perform the swap instead of just moving focus
        if let Some(source) = self.swap_source.take() {
            SWAP_MODE.store(false, std::sync::atomic::Ordering::Relaxed);

            if let Some(ws) = self.active_workspace()
                && let Some(root) = &ws.root
            {
                // Move focus to find the target pane
                let moved = matches!(root.focus_in_direction(dir, window, cx), FocusNav::Moved);
                if let Some(target) = root.focused_pane(window, cx)
                    && target != source
                {
                    let swapped = if let Some(ws) = self.active_workspace_mut()
                        && let Some(ref mut root) = ws.root
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
            && let Some(root) = &ws.root
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

    /// US-019 (orchestration-v2): teleport to the next pane whose agent is
    /// `WaitingForInput`, cross-workspace, in a stable order (workspace
    /// index, then layout traversal, then tab order). Repeated presses cycle
    /// through the waiting set via `jump_cursor`; activating a background
    /// tab is part of the jump (the waiting surface may be hidden). No
    /// waiting agent → silent no-op (an empty queue is the good news).
    /// Sessions without a resolved surface are skipped (US-017 fallback -
    /// never jump to a guessed pane).
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

    /// EP-005 US-015: the teleport body of `handle_jump_next_waiting`,
    /// parametrized by the state predicate so the Fleet Bar's waiting AND
    /// errored chips reuse the exact same stable order + cursor cycling
    /// (`next_in_cycle`). The cursor is shared across predicates: switching
    /// chip kinds simply restarts the cycle at the first match (the cursor
    /// no longer appears in the new order), which is the existing
    /// stale-cursor behavior.
    pub(crate) fn jump_next_session_where(
        &mut self,
        state_matches: impl Fn(&crate::ai_types::AgentState) -> bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut order: Vec<(usize, gpui::Entity<crate::pane::Pane>, usize, u64)> = Vec::new();
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
            if let Some(root) = &ws.root {
                for pane in root.collect_leaves() {
                    for (tab_idx, tab) in pane.read(cx).tabs.iter().enumerate() {
                        if let Some(t) = tab.as_terminal() {
                            let sid = t.entity_id().as_u64();
                            if matching.contains(&sid) {
                                order.push((ws_idx, pane.clone(), tab_idx, sid));
                            }
                        }
                    }
                }
            }
        }
        let ids: Vec<u64> = order.iter().map(|(_, _, _, sid)| *sid).collect();
        let Some(next) = next_in_cycle(&ids, self.jump_cursor) else {
            return;
        };
        let Some((ws_idx, pane, tab_idx, sid)) = order.into_iter().find(|(_, _, _, s)| *s == next)
        else {
            return;
        };
        self.activate_workspace_at(
            ws_idx,
            WorkspaceFocusTarget::PaneTab {
                pane: pane.clone(),
                tab_idx,
            },
            window,
            cx,
        );
        self.jump_cursor = Some(sid);
    }
}

/// Pure cycle rule (unit-tested): first waiting surface when the cursor is
/// unset or gone from the set; otherwise the one after it, wrapping.
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
        // Cursor points at a surface that stopped waiting: restart at first.
        assert_eq!(next_in_cycle(&[10, 20, 30], Some(99)), Some(10));
    }

    #[test]
    fn cycles_and_wraps() {
        assert_eq!(next_in_cycle(&[10, 20, 30], Some(10)), Some(20));
        assert_eq!(next_in_cycle(&[10, 20, 30], Some(30)), Some(10));
        // Single waiting pane: jumping again stays on it.
        assert_eq!(next_in_cycle(&[10], Some(10)), Some(10));
    }
}
