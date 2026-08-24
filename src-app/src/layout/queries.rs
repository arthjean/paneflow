//! Read-only traversal: focused-pane lookup, leaf counting, leaf extraction,
//! equalize-ratios mutator (mutates interior `Rc<Cell>` state, not tree shape).

use gpui::{App, Entity, Focusable, Window};

use crate::pane::Pane;

use super::tree::LayoutTree;

impl LayoutTree {
    /// Find the focused pane entity in the tree.
    pub fn focused_pane(&self, window: &Window, cx: &App) -> Option<Entity<Pane>> {
        match self {
            LayoutTree::Leaf(pane) => {
                if pane.read(cx).focus_handle(cx).is_focused(window) {
                    Some(pane.clone())
                } else {
                    None
                }
            }
            LayoutTree::Container { children, .. } => {
                for child in children {
                    if let Some(pane) = child.node.focused_pane(window, cx) {
                        return Some(pane);
                    }
                }
                None
            }
        }
    }

    /// Push the Ghostty-style unfocused dim onto every leaf of this tree.
    ///
    /// Focus is the single source of truth: this is a pure projection of
    /// "which leaf holds focus" onto per-pane state, so no call site has to
    /// remember to clear a stale dim. Called once per frame from
    /// `PaneFlowApp::render` (focus changes always repaint the window, and
    /// GPUI has no window-level focus-changed hook here); every write is
    /// idempotent, so a steady frame does nothing at all.
    pub fn sync_unfocused_dim(&self, window: &Window, cx: &mut App) {
        let leaves = self.collect_leaves();
        let focused = leaves
            .iter()
            .position(|pane| pane.read(cx).focus_handle(cx).is_focused(window));
        match dim_policy(leaves.len(), focused) {
            DimPolicy::Keep => {}
            DimPolicy::ClearAll => {
                for pane in &leaves {
                    pane.update(cx, |pane, cx| pane.set_dimmed(false, cx));
                }
            }
            DimPolicy::DimAllExcept(idx) => {
                for (i, pane) in leaves.iter().enumerate() {
                    pane.update(cx, |pane, cx| pane.set_dimmed(i != idx, cx));
                }
            }
        }
    }

    /// Count the number of leaf (terminal) panes in the tree.
    pub fn leaf_count(&self) -> usize {
        match self {
            LayoutTree::Leaf(_) => 1,
            LayoutTree::Container { children, .. } => {
                children.iter().map(|c| c.node.leaf_count()).sum()
            }
        }
    }

    /// Collect all leaf pane entities in left-to-right (top-to-bottom) order.
    pub fn collect_leaves(&self) -> Vec<Entity<Pane>> {
        match self {
            LayoutTree::Leaf(pane) => vec![pane.clone()],
            LayoutTree::Container { children, .. } => children
                .iter()
                .flat_map(|c| c.node.collect_leaves())
                .collect(),
        }
    }

    /// Zero-alloc short-circuiting `any` over leaf panes - replaces
    /// `collect_leaves().iter().any(pred)`, which allocates a
    /// `Vec<Entity<Pane>>` (cloning every leaf) before scanning. The `&mut`
    /// predicate is reborrowed across recursion so a capturing closure (e.g.
    /// reading each pane via `cx`) composes naturally.
    pub fn any_leaf(&self, pred: &mut impl FnMut(&Entity<Pane>) -> bool) -> bool {
        match self {
            LayoutTree::Leaf(p) => pred(p),
            LayoutTree::Container { children, .. } => {
                for c in children {
                    if c.node.any_leaf(pred) {
                        return true;
                    }
                }
                false
            }
        }
    }

    /// True iff `pane` is a leaf anywhere in this tree. Zero-alloc membership
    /// test that short-circuits on the first match - replaces
    /// `collect_leaves().contains(&pane)`. Hot on the pane-event path where the
    /// owning workspace is resolved by membership.
    pub fn contains_leaf(&self, pane: &Entity<Pane>) -> bool {
        self.any_leaf(&mut |p| p == pane)
    }

    /// Set all split ratios to equal values at every level of the tree.
    /// Each container's children get `1.0 / n` where `n` is the child count.
    /// The last child absorbs floating-point remainder to ensure exact sum of 1.0.
    /// Leaf nodes are unchanged. No-op on a single-pane or zoomed workspace.
    /// Mutates interior state via `Rc<Cell<f32>>` ratios.
    pub fn equalize_ratios(&self) {
        if let LayoutTree::Container { children, .. } = self {
            let n = children.len();
            let equal = 1.0 / n as f32;
            for (i, child) in children.iter().enumerate() {
                if i == n - 1 {
                    // Last child absorbs rounding error
                    child.ratio.set(1.0 - equal * (n - 1) as f32);
                } else {
                    child.ratio.set(equal);
                }
                child.node.equalize_ratios();
            }
        }
    }

    /// Return the first (leftmost/topmost) leaf entity without focusing it.
    pub fn first_leaf(&self) -> Option<Entity<Pane>> {
        match self {
            LayoutTree::Leaf(pane) => Some(pane.clone()),
            LayoutTree::Container { children, .. } => {
                children.first().and_then(|c| c.node.first_leaf())
            }
        }
    }

    /// Return the last (rightmost/bottommost) leaf entity without focusing it.
    pub fn last_leaf(&self) -> Option<Entity<Pane>> {
        match self {
            LayoutTree::Leaf(pane) => Some(pane.clone()),
            LayoutTree::Container { children, .. } => {
                children.last().and_then(|c| c.node.last_leaf())
            }
        }
    }
}

/// Outcome of the unfocused-dim decision, split out from
/// [`LayoutTree::sync_unfocused_dim`] so the policy is unit-testable without a
/// GPUI window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DimPolicy {
    /// A single pane has nothing to contrast against: never dim it.
    ClearAll,
    /// Dim every leaf except the one at this index.
    DimAllExcept(usize),
    /// Focus left the pane tree entirely (sidebar, settings, a fleet search
    /// field). Keep the previous decision instead of flashing the whole
    /// cockpit back to full brightness. This replaces Ghostty's
    /// `lastFocusedSurface` bookkeeping: the last decision *is* the memory.
    Keep,
}

/// Pure dim policy: see [`DimPolicy`].
pub(crate) fn dim_policy(leaf_count: usize, focused: Option<usize>) -> DimPolicy {
    if leaf_count < 2 {
        return DimPolicy::ClearAll;
    }
    match focused {
        Some(idx) => DimPolicy::DimAllExcept(idx),
        None => DimPolicy::Keep,
    }
}

#[cfg(test)]
mod tests {
    use super::{DimPolicy, dim_policy};

    #[test]
    fn dim_policy_never_dims_a_lone_pane() {
        assert_eq!(dim_policy(0, None), DimPolicy::ClearAll);
        assert_eq!(dim_policy(1, Some(0)), DimPolicy::ClearAll);
        assert_eq!(dim_policy(1, None), DimPolicy::ClearAll);
    }

    #[test]
    fn dim_policy_dims_every_pane_but_the_focused_one() {
        assert_eq!(dim_policy(3, Some(1)), DimPolicy::DimAllExcept(1));
        assert_eq!(dim_policy(2, Some(0)), DimPolicy::DimAllExcept(0));
    }

    #[test]
    fn dim_policy_is_sticky_when_focus_leaves_the_tree() {
        // Clicking the sidebar or opening Settings must not undim everything.
        assert_eq!(dim_policy(4, None), DimPolicy::Keep);
    }
}
