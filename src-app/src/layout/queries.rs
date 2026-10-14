use gpui::{App, Edges, Entity, Focusable, Window};

use crate::pane::{Pane, SplitFocus};

use super::density::{CardFrame, PaneDensity, child_edges};
use super::tree::{LayoutTree, SplitDirection};

impl LayoutTree {
    pub(crate) fn cancel_resize(&self) {
        if let Self::Container { children, drag, .. } = self {
            drag.set(None);
            for child in children {
                child.node.cancel_resize();
            }
        }
    }

    pub(crate) fn has_docked_panes(&self, cx: &App) -> bool {
        self.any_leaf(&mut |pane| !pane.read(cx).is_detached())
    }

    pub fn focused_pane(&self, window: &Window, cx: &App) -> Option<Entity<Pane>> {
        match self {
            LayoutTree::Leaf(pane) => {
                if !pane.read(cx).is_detached() && pane.read(cx).focus_handle(cx).is_focused(window)
                {
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
            if pane.read(cx).is_detached() {
                return;
            }
            if focused.is_none() && pane.read(cx).focus_handle(cx).is_focused(window) {
                focused = Some(count);
            }
            count += 1;
        });
        match dim_policy(count, focused) {
            DimPolicy::Keep => {}
            DimPolicy::ClearAll => {
                self.for_each_leaf(&mut |pane| {
                    if pane.read(cx).is_detached() {
                        return;
                    }
                    pane.update(cx, |pane, cx| pane.set_split_focus(SplitFocus::Alone, cx));
                });
            }
            DimPolicy::DimAllExcept(idx) => {
                let mut index = 0usize;
                self.for_each_leaf(&mut |pane| {
                    if pane.read(cx).is_detached() {
                        return;
                    }
                    let focus = if index == idx {
                        SplitFocus::Focused
                    } else {
                        SplitFocus::Unfocused
                    };
                    pane.update(cx, |pane, cx| pane.set_split_focus(focus, cx));
                    index += 1;
                });
            }
        }
    }

    pub(crate) fn docked_leaf_edges(
        &self,
        edges: Edges<bool>,
        cx: &App,
        visit: &mut impl FnMut(&Entity<Pane>, Edges<bool>),
    ) {
        match self {
            LayoutTree::Leaf(pane) => {
                if !pane.read(cx).is_detached() {
                    visit(pane, edges);
                }
            }
            LayoutTree::Container {
                direction,
                children,
                ..
            } => {
                let docked: Vec<_> = children
                    .iter()
                    .filter(|child| child.node.has_docked_panes(cx))
                    .collect();
                let last = docked.len().saturating_sub(1);
                for (index, child) in docked.into_iter().enumerate() {
                    let edges = child_edges(edges, *direction, index == 0, index == last);
                    child.node.docked_leaf_edges(edges, cx, visit);
                }
            }
        }
    }

    pub(crate) fn sync_card_frames(
        &self,
        density: PaneDensity,
        outer: Edges<bool>,
        split_preview: Option<(&Entity<Pane>, SplitDirection)>,
        cx: &mut App,
    ) -> CardFrame {
        let mut frames = Vec::new();
        let mut preview_frame = density.frame(outer);
        self.docked_leaf_edges(outer, cx, &mut |pane, edges| {
            let edges = match split_preview {
                Some((target, direction)) if target == pane => {
                    preview_frame = density.frame(child_edges(edges, direction, false, true));
                    child_edges(edges, direction, true, false)
                }
                _ => edges,
            };
            frames.push((pane.clone(), density.frame(edges)));
        });
        for (pane, frame) in frames {
            pane.update(cx, |pane, cx| pane.set_card_frame(frame, cx));
        }
        preview_frame
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
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui::{AppContext, Edges, Entity, TestAppContext};

    use super::{DimPolicy, dim_policy};
    use crate::layout::density::{CardFrame, PaneDensity, grid_edges};
    use crate::layout::tree::{LayoutChild, LayoutTree, SplitDirection};
    use crate::pane::Pane;
    use crate::terminal::TerminalView;

    fn test_pane(cx: &mut TestAppContext) -> Entity<Pane> {
        cx.update(|cx| {
            let terminal = cx.new(|cx| TerminalView::display_only_for_test(1, cx));
            cx.new(|cx| Pane::new(terminal, 1, cx))
        })
    }

    fn split(direction: SplitDirection, nodes: Vec<LayoutTree>) -> LayoutTree {
        let ratio = 1.0 / nodes.len() as f32;
        LayoutTree::Container {
            direction,
            children: nodes
                .into_iter()
                .map(|node| LayoutChild {
                    node,
                    ratio: Rc::new(Cell::new(ratio)),
                })
                .collect(),
            drag: Rc::new(Cell::new(None)),
            container_size: Rc::new(Cell::new(0.0)),
        }
    }

    fn edges(top: bool, right: bool, bottom: bool, left: bool) -> CardFrame {
        CardFrame::Flush(Edges {
            top,
            right,
            bottom,
            left,
        })
    }

    #[gpui::test]
    fn compact_frames_follow_each_pane_position_in_the_grid(cx: &mut TestAppContext) {
        let left = test_pane(cx);
        let top_right = test_pane(cx);
        let bottom_right = test_pane(cx);
        let tree = split(
            SplitDirection::Vertical,
            vec![
                LayoutTree::Leaf(left.clone()),
                split(
                    SplitDirection::Horizontal,
                    vec![
                        LayoutTree::Leaf(top_right.clone()),
                        LayoutTree::Leaf(bottom_right.clone()),
                    ],
                ),
            ],
        );
        let frame_of = |pane: &Entity<Pane>, cx: &mut TestAppContext| {
            cx.update(|cx| pane.read(cx).card_frame())
        };

        cx.update(|cx| tree.sync_card_frames(PaneDensity::Compact, grid_edges(true), None, cx));
        assert_eq!(frame_of(&left, cx), edges(true, false, true, true));
        assert_eq!(frame_of(&top_right, cx), edges(true, true, false, false));
        assert_eq!(frame_of(&bottom_right, cx), edges(false, true, true, false));

        cx.update(|cx| tree.sync_card_frames(PaneDensity::Compact, grid_edges(false), None, cx));
        assert_eq!(frame_of(&top_right, cx), edges(true, false, false, false));

        let palette = cx.update(|cx| {
            tree.sync_card_frames(
                PaneDensity::Compact,
                grid_edges(true),
                Some((&left, SplitDirection::Horizontal)),
                cx,
            )
        });
        assert_eq!(frame_of(&left, cx), edges(true, false, false, true));
        assert_eq!(palette, edges(false, false, true, true));

        cx.update(|cx| tree.sync_card_frames(PaneDensity::Comfortable, grid_edges(true), None, cx));
        for pane in [&left, &top_right, &bottom_right] {
            assert_eq!(frame_of(pane, cx), CardFrame::Card);
        }
    }

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
