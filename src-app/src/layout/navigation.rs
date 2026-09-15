use std::cmp::Ordering;

use gpui::{App, Entity, Focusable, Window};

use crate::pane::Pane;

use super::tree::{LayoutChild, LayoutTree, SplitDirection};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FocusNav {
    NotHere,
    FocusedHere,
    Moved,
}

#[derive(Clone)]
struct LeafRect {
    pane: Entity<Pane>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl LeafRect {
    fn center_x(&self) -> f32 {
        self.x + self.w / 2.0
    }

    fn center_y(&self) -> f32 {
        self.y + self.h / 2.0
    }
}

fn ratio_sum(children: &[&LayoutChild]) -> f32 {
    children
        .iter()
        .map(|child| child.ratio.get())
        .filter(|ratio| ratio.is_finite() && *ratio > 0.0)
        .sum()
}

fn child_fraction(child: &LayoutChild, sum: f32, fallback: f32) -> f32 {
    let ratio = child.ratio.get();
    if sum <= 0.0 {
        return fallback;
    }
    if ratio.is_finite() && ratio > 0.0 {
        ratio / sum
    } else {
        0.0
    }
}

fn focus_score(
    dir: FocusDirection,
    current: &LeafRect,
    candidate: &LeafRect,
) -> Option<(f32, f32)> {
    const EPS: f32 = 0.0001;
    match dir {
        FocusDirection::Left => {
            let primary = current.center_x() - candidate.center_x();
            (primary > EPS).then_some((primary, (current.center_y() - candidate.center_y()).abs()))
        }
        FocusDirection::Right => {
            let primary = candidate.center_x() - current.center_x();
            (primary > EPS).then_some((primary, (current.center_y() - candidate.center_y()).abs()))
        }
        FocusDirection::Up => {
            let primary = current.center_y() - candidate.center_y();
            (primary > EPS).then_some((primary, (current.center_x() - candidate.center_x()).abs()))
        }
        FocusDirection::Down => {
            let primary = candidate.center_y() - current.center_y();
            (primary > EPS).then_some((primary, (current.center_x() - candidate.center_x()).abs()))
        }
    }
}

fn compare_focus_score(a: (f32, f32, usize), b: (f32, f32, usize)) -> Ordering {
    a.0.total_cmp(&b.0)
        .then_with(|| a.1.total_cmp(&b.1))
        .then_with(|| a.2.cmp(&b.2))
}

impl LayoutTree {
    fn collect_leaf_rects(&self, bounds: [f32; 4], out: &mut Vec<LeafRect>, cx: &App) {
        let [x, y, w, h] = bounds;
        if !self.has_docked_panes(cx) {
            return;
        }
        match self {
            LayoutTree::Leaf(pane) => out.push(LeafRect {
                pane: pane.clone(),
                x,
                y,
                w,
                h,
            }),
            LayoutTree::Container {
                direction,
                children,
                ..
            } => {
                let children: Vec<_> = children
                    .iter()
                    .filter(|child| child.node.has_docked_panes(cx))
                    .collect();
                if children.is_empty() {
                    return;
                }
                let sum = ratio_sum(&children);
                let fallback = 1.0 / children.len() as f32;
                let mut offset = 0.0;
                for child in children {
                    let fraction = child_fraction(child, sum, fallback);
                    match direction {
                        SplitDirection::Horizontal => {
                            let child_h = h * fraction;
                            child
                                .node
                                .collect_leaf_rects([x, y + offset, w, child_h], out, cx);
                            offset += child_h;
                        }
                        SplitDirection::Vertical => {
                            let child_w = w * fraction;
                            child
                                .node
                                .collect_leaf_rects([x + offset, y, child_w, h], out, cx);
                            offset += child_w;
                        }
                    }
                }
            }
        }
    }

    pub fn focus_first(&self, window: &mut Window, cx: &mut App) {
        match self {
            LayoutTree::Leaf(pane) => {
                if !pane.read(cx).is_detached() {
                    pane.read(cx).focus_handle(cx).focus(window, cx);
                }
            }
            LayoutTree::Container { children, .. } => {
                if let Some(first) = children
                    .iter()
                    .find(|child| child.node.has_docked_panes(cx))
                {
                    first.node.focus_first(window, cx);
                }
            }
        }
    }

    pub fn focus_in_direction(
        &self,
        dir: FocusDirection,
        window: &mut Window,
        cx: &mut App,
    ) -> FocusNav {
        let mut leaves = Vec::new();
        self.collect_leaf_rects([0.0, 0.0, 1.0, 1.0], &mut leaves, cx);
        let Some(current_idx) = leaves
            .iter()
            .position(|leaf| leaf.pane.read(cx).focus_handle(cx).is_focused(window))
        else {
            return FocusNav::NotHere;
        };

        let current = &leaves[current_idx];
        let mut best: Option<(usize, (f32, f32, usize))> = None;
        for (idx, candidate) in leaves.iter().enumerate() {
            if idx == current_idx {
                continue;
            }
            let Some((primary, cross)) = focus_score(dir, current, candidate) else {
                continue;
            };
            let score = (primary, cross, idx);
            if best
                .as_ref()
                .is_none_or(|(_, best_score)| compare_focus_score(score, *best_score).is_lt())
            {
                best = Some((idx, score));
            }
        }

        let Some((target_idx, _)) = best else {
            return FocusNav::FocusedHere;
        };
        leaves[target_idx]
            .pane
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
        FocusNav::Moved
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext, Entity, Focusable, TestAppContext};

    use crate::pane::Pane;
    use crate::terminal::TerminalView;

    use super::*;

    fn test_pane(cx: &mut impl AppContext, workspace_id: u64) -> Entity<Pane> {
        let terminal = cx.new(|cx| TerminalView::display_only_for_test(workspace_id, cx));
        cx.new(|cx| Pane::new(terminal, workspace_id, cx))
    }

    #[gpui::test]
    fn detached_panes_are_excluded_from_focus_and_projected_geometry(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let detached = test_pane(cx, 1);
        let first = test_pane(cx, 1);
        let last = test_pane(cx, 1);
        let tree = LayoutTree::new_split(
            SplitDirection::Vertical,
            LayoutTree::new_split(
                SplitDirection::Horizontal,
                LayoutTree::Leaf(detached.clone()),
                LayoutTree::Leaf(first.clone()),
            ),
            LayoutTree::Leaf(last.clone()),
        );
        cx.update(|window, cx| {
            detached.update(cx, |pane, _| {
                pane.detached = Some(crate::app::detached_panes::DetachedPanePlacement {
                    window: window.window_handle(),
                    bounds: gpui::Bounds::default(),
                });
            });
            assert_eq!(tree.leaf_count(), 3);
            let mut rects = Vec::new();
            tree.collect_leaf_rects([0.0, 0.0, 1.0, 1.0], &mut rects, cx);
            assert_eq!(rects.len(), 2);
            assert_eq!(rects[0].h, 1.0);
            tree.focus_first(window, cx);
            assert!(first.read(cx).focus_handle(cx).is_focused(window));
            assert_eq!(
                tree.focus_in_direction(FocusDirection::Right, window, cx),
                FocusNav::Moved
            );
            assert!(last.read(cx).focus_handle(cx).is_focused(window));
            assert_eq!(
                tree.focus_in_direction(FocusDirection::Left, window, cx),
                FocusNav::Moved
            );
            assert!(first.read(cx).focus_handle(cx).is_focused(window));
        });
    }

    #[gpui::test]
    fn focus_right_selects_same_row_spatial_neighbor(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let top_left = test_pane(cx, 1);
        let bottom_left = test_pane(cx, 1);
        let top_right = test_pane(cx, 1);
        let bottom_right = test_pane(cx, 1);
        let left_column = LayoutTree::new_split(
            SplitDirection::Horizontal,
            LayoutTree::Leaf(top_left),
            LayoutTree::Leaf(bottom_left.clone()),
        );
        let right_column = LayoutTree::new_split(
            SplitDirection::Horizontal,
            LayoutTree::Leaf(top_right),
            LayoutTree::Leaf(bottom_right.clone()),
        );
        let tree = LayoutTree::new_split(SplitDirection::Vertical, left_column, right_column);

        cx.update(|window, cx| {
            bottom_left.read(cx).focus_handle(cx).focus(window, cx);

            assert_eq!(
                tree.focus_in_direction(FocusDirection::Right, window, cx),
                FocusNav::Moved
            );
            assert!(bottom_right.read(cx).focus_handle(cx).is_focused(window));
        });
    }
}
