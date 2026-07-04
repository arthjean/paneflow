//! Tree-shrinking mutations: `close_focused` (focus-driven removal with
//! neighbour-focus propagation) and `remove_pane` (entity-identity removal).
//!
//! Kept separate from `mutations.rs` to respect the 280-LOC cap (US-029).

use gpui::{App, Entity, Focusable, Window};

use crate::pane::Pane;

use super::tree::{LayoutChild, LayoutTree, normalize_ratios, redistribute_equal};

impl LayoutTree {
    /// Close the focused pane. Consumes self and returns:
    /// - `tree`: the surviving layout (None if the last pane was closed)
    /// - `closed`: whether a pane was actually closed
    /// - `focus_neighbor`: the pane that should receive focus (previous sibling,
    ///   or next sibling if the closed pane was first)
    pub fn close_focused(
        self,
        window: &Window,
        cx: &App,
    ) -> (Option<LayoutTree>, bool, Option<Entity<Pane>>) {
        match self {
            LayoutTree::Leaf(pane) => {
                if pane.read(cx).focus_handle(cx).is_focused(window) {
                    (None, true, None)
                } else {
                    (Some(LayoutTree::Leaf(pane)), false, None)
                }
            }
            LayoutTree::Container {
                direction,
                children,
                drag,
                container_size,
            } => {
                let mut new_children = Vec::with_capacity(children.len());
                let mut closed = false;
                let mut closed_idx: Option<usize> = None;
                let mut removed_ratio = 0.0_f32;
                let mut focus_neighbor: Option<Entity<Pane>> = None;

                for (i, child) in children.into_iter().enumerate() {
                    if closed {
                        new_children.push(child);
                        continue;
                    }
                    let (new_node, was_closed, nested_focus) = child.node.close_focused(window, cx);
                    if was_closed {
                        closed = true;
                        closed_idx = Some(i);
                        // Propagate focus neighbor from deeper levels
                        focus_neighbor = nested_focus;
                        if let Some(node) = new_node {
                            new_children.push(LayoutChild {
                                node,
                                ratio: child.ratio,
                            });
                        } else {
                            // Direct child leaf was removed - record its ratio
                            removed_ratio = child.ratio.get();
                        }
                    } else {
                        new_children.push(LayoutChild {
                            node: new_node
                                .expect("close_focused: non-closed child must return Some"),
                            ratio: child.ratio,
                        });
                    }
                }

                if !closed {
                    return (
                        Some(LayoutTree::Container {
                            direction,
                            children: new_children,
                            drag,
                            container_size,
                        }),
                        false,
                        None,
                    );
                }

                // Cancel any in-progress drag before structural changes
                drag.set(None);

                // Determine focus neighbor when a direct child was removed
                // (only if no nested focus was already determined)
                if focus_neighbor.is_none()
                    && let Some(idx) = closed_idx
                {
                    // Prefer previous sibling, fall back to next
                    if idx > 0 {
                        focus_neighbor = new_children.get(idx - 1).and_then(|c| c.node.last_leaf());
                    } else {
                        focus_neighbor = new_children.first().and_then(|c| c.node.first_leaf());
                    }
                }

                match new_children.len() {
                    0 => (None, true, None),
                    1 => {
                        // Collapse single-child container
                        let child = new_children.into_iter().next().unwrap();
                        (Some(child.node), true, focus_neighbor)
                    }
                    _ => {
                        // Redistribute removed child's ratio equally
                        if removed_ratio > 0.0 {
                            redistribute_equal(&new_children, removed_ratio);
                        } else {
                            normalize_ratios(&new_children);
                        }
                        (
                            Some(LayoutTree::Container {
                                direction,
                                children: new_children,
                                drag,
                                container_size,
                            }),
                            true,
                            focus_neighbor,
                        )
                    }
                }
            }
        }
    }

    /// Remove a specific pane entity from the tree. Consumes `self` and returns
    /// the surviving tree plus whether a pane was removed.
    pub fn remove_pane(self, target: &Entity<Pane>) -> (Option<LayoutTree>, bool) {
        match self {
            LayoutTree::Leaf(ref pane) => {
                if pane == target {
                    (None, true)
                } else {
                    (Some(self), false)
                }
            }
            LayoutTree::Container {
                direction,
                children,
                drag,
                container_size,
            } => {
                let mut new_children = Vec::with_capacity(children.len());
                let mut removed_ratio = 0.0_f32;
                let mut removed = false;
                for child in children {
                    let (new_node, child_removed) = child.node.remove_pane(target);
                    if child_removed {
                        removed = true;
                    }
                    if let Some(node) = new_node {
                        new_children.push(LayoutChild {
                            node,
                            ratio: child.ratio,
                        });
                    } else {
                        removed_ratio += child.ratio.get();
                    }
                }

                if !removed {
                    return (
                        Some(LayoutTree::Container {
                            direction,
                            children: new_children,
                            drag,
                            container_size,
                        }),
                        false,
                    );
                }

                // Cancel any in-progress drag before structural changes.
                drag.set(None);

                let tree = match new_children.len() {
                    0 => None,
                    1 => Some(new_children.into_iter().next().unwrap().node),
                    _ => {
                        if removed_ratio > 0.0 {
                            redistribute_equal(&new_children, removed_ratio);
                        } else {
                            normalize_ratios(&new_children);
                        }
                        Some(LayoutTree::Container {
                            direction,
                            children: new_children,
                            drag,
                            container_size,
                        })
                    }
                };
                (tree, true)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext, Entity, TestAppContext};

    use crate::pane::Pane;
    use crate::terminal::TerminalView;

    use super::*;
    use crate::layout::SplitDirection;

    fn test_pane(cx: &mut impl AppContext, workspace_id: u64) -> Entity<Pane> {
        let terminal = cx.new(|cx| TerminalView::display_only_for_test(workspace_id, cx));
        cx.new(|cx| Pane::new(terminal, workspace_id, cx))
    }

    fn leaf_ids(tree: &LayoutTree) -> Vec<gpui::EntityId> {
        tree.collect_leaves()
            .into_iter()
            .map(|pane| pane.entity_id())
            .collect()
    }

    #[gpui::test]
    fn remove_pane_redistributes_removed_ratio(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let a = test_pane(cx, 1);
        let b = test_pane(cx, 1);
        let c = test_pane(cx, 1);
        let tree = LayoutTree::from_panes_equal(
            SplitDirection::Vertical,
            vec![a.clone(), b.clone(), c.clone()],
        )
        .expect("non-empty layout");

        let (tree, removed) = tree.remove_pane(&b);
        let tree = tree.expect("two panes should remain");

        assert!(removed);
        assert_eq!(tree.leaf_count(), 2);
        assert_eq!(leaf_ids(&tree), vec![a.entity_id(), c.entity_id()]);
        match tree {
            LayoutTree::Container { children, .. } => {
                assert_eq!(children.len(), 2);
                assert!((children[0].ratio.get() - 0.5).abs() < 0.0001);
                assert!((children[1].ratio.get() - 0.5).abs() < 0.0001);
            }
            LayoutTree::Leaf(_) => panic!("two remaining panes should stay in a container"),
        }
    }

    #[gpui::test]
    fn remove_pane_collapses_single_child_container(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let a = test_pane(cx, 2);
        let b = test_pane(cx, 2);
        let tree = LayoutTree::new_split(
            SplitDirection::Horizontal,
            LayoutTree::Leaf(a.clone()),
            LayoutTree::Leaf(b.clone()),
        );

        let (tree, removed) = tree.remove_pane(&b);
        let tree = tree.expect("one pane should remain");

        assert!(removed);
        assert_eq!(tree.leaf_count(), 1);
        assert_eq!(leaf_ids(&tree), vec![a.entity_id()]);
        assert!(matches!(tree, LayoutTree::Leaf(_)));
    }

    #[gpui::test]
    fn remove_pane_absent_target_preserves_layout(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let a = test_pane(cx, 3);
        let b = test_pane(cx, 3);
        let missing = test_pane(cx, 3);
        let tree = LayoutTree::new_split(
            SplitDirection::Horizontal,
            LayoutTree::Leaf(a.clone()),
            LayoutTree::Leaf(b.clone()),
        );

        let (tree, removed) = tree.remove_pane(&missing);
        let tree = tree.expect("absent target must preserve the tree");

        assert!(!removed);
        assert_eq!(leaf_ids(&tree), vec![a.entity_id(), b.entity_id()]);
    }
}
