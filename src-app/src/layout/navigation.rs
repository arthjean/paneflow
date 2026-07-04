//! Focus navigation: directional movement through normalized layout geometry
//! plus first/last leaf focus helpers.

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

fn ratio_sum(children: &[LayoutChild]) -> f32 {
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
    fn collect_leaf_rects(&self, x: f32, y: f32, w: f32, h: f32, out: &mut Vec<LeafRect>) {
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
                if children.is_empty() {
                    return;
                }
                let sum = ratio_sum(children);
                let fallback = 1.0 / children.len() as f32;
                let mut offset = 0.0;
                for child in children {
                    let fraction = child_fraction(child, sum, fallback);
                    match direction {
                        SplitDirection::Horizontal => {
                            let child_h = h * fraction;
                            child
                                .node
                                .collect_leaf_rects(x, y + offset, w, child_h, out);
                            offset += child_h;
                        }
                        SplitDirection::Vertical => {
                            let child_w = w * fraction;
                            child
                                .node
                                .collect_leaf_rects(x + offset, y, child_w, h, out);
                            offset += child_w;
                        }
                    }
                }
            }
        }
    }

    /// Focus the first (leftmost/topmost) leaf in the tree.
    pub fn focus_first(&self, window: &mut Window, cx: &mut App) {
        match self {
            LayoutTree::Leaf(pane) => {
                pane.read(cx).focus_handle(cx).focus(window, cx);
            }
            LayoutTree::Container { children, .. } => {
                if let Some(first) = children.first() {
                    first.node.focus_first(window, cx);
                }
            }
        }
    }

    /// Move focus in the given direction. Returns the navigation result.
    pub fn focus_in_direction(
        &self,
        dir: FocusDirection,
        window: &mut Window,
        cx: &mut App,
    ) -> FocusNav {
        let mut leaves = Vec::new();
        self.collect_leaf_rects(0.0, 0.0, 1.0, 1.0, &mut leaves);
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
