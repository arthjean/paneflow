use gpui::{App, Entity, Focusable, Window};

use crate::pane::Pane;

use super::tree::LayoutTree;

impl LayoutTree {
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

    pub fn sync_unfocused_dim(&self, window: &Window, cx: &mut App) {
        let mut count = 0usize;
        let mut focused = None;
        self.for_each_leaf(&mut |pane| {
            if focused.is_none() && pane.read(cx).focus_handle(cx).is_focused(window) {
                focused = Some(count);
            }
            count += 1;
        });
        match dim_policy(count, focused) {
            DimPolicy::Keep => {}
            DimPolicy::ClearAll => {
                self.for_each_leaf(&mut |pane| {
                    pane.update(cx, |pane, cx| pane.set_dimmed(false, cx));
                });
            }
            DimPolicy::DimAllExcept(idx) => {
                let mut index = 0usize;
                self.for_each_leaf(&mut |pane| {
                    pane.update(cx, |pane, cx| pane.set_dimmed(index != idx, cx));
                    index += 1;
                });
            }
        }
    }

    pub fn for_each_leaf(&self, visit: &mut impl FnMut(&Entity<Pane>)) {
        self.any_leaf(&mut |pane| {
            visit(pane);
            false
        });
    }

    pub fn leaf_count(&self) -> usize {
        match self {
            LayoutTree::Leaf(_) => 1,
            LayoutTree::Container { children, .. } => {
                children.iter().map(|c| c.node.leaf_count()).sum()
            }
        }
    }

    pub fn collect_leaves(&self) -> Vec<Entity<Pane>> {
        match self {
            LayoutTree::Leaf(pane) => vec![pane.clone()],
            LayoutTree::Container { children, .. } => children
                .iter()
                .flat_map(|c| c.node.collect_leaves())
                .collect(),
        }
    }

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

    pub fn contains_leaf(&self, pane: &Entity<Pane>) -> bool {
        self.any_leaf(&mut |p| p == pane)
    }

    pub fn equalize_ratios(&self) {
        if let LayoutTree::Container { children, .. } = self {
            let n = children.len();
            let equal = 1.0 / n as f32;
            for (i, child) in children.iter().enumerate() {
                if i == n - 1 {
                    child.ratio.set(1.0 - equal * (n - 1) as f32);
                } else {
                    child.ratio.set(equal);
                }
                child.node.equalize_ratios();
            }
        }
    }

    pub fn first_leaf(&self) -> Option<Entity<Pane>> {
        match self {
            LayoutTree::Leaf(pane) => Some(pane.clone()),
            LayoutTree::Container { children, .. } => {
                children.first().and_then(|c| c.node.first_leaf())
            }
        }
    }

    pub fn last_leaf(&self) -> Option<Entity<Pane>> {
        match self {
            LayoutTree::Leaf(pane) => Some(pane.clone()),
            LayoutTree::Container { children, .. } => {
                children.last().and_then(|c| c.node.last_leaf())
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DimPolicy {
    ClearAll,
    DimAllExcept(usize),
    Keep,
}

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
        assert_eq!(dim_policy(4, None), DimPolicy::Keep);
    }
}
