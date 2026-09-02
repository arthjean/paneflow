use gpui::{App, Entity, Focusable, Window};

use crate::pane::Pane;

use super::tree::{LayoutTree, SplitDirection, insert_sibling};

impl LayoutTree {
    pub fn split_at_focused(
        &mut self,
        direction: SplitDirection,
        new_pane: Entity<Pane>,
        window: &Window,
        cx: &App,
    ) -> bool {
        match self {
            LayoutTree::Leaf(pane) => {
                if pane.read(cx).focus_handle(cx).is_focused(window) {
                    let old = std::mem::replace(self, LayoutTree::Leaf(new_pane.clone()));
                    *self = LayoutTree::new_split(direction, old, LayoutTree::Leaf(new_pane));
                    true
                } else {
                    false
                }
            }
            LayoutTree::Container {
                direction: dir,
                children,
                ..
            } => {
                if *dir == direction {
                    for i in 0..children.len() {
                        if let LayoutTree::Leaf(pane) = &children[i].node
                            && pane.read(cx).focus_handle(cx).is_focused(window)
                        {
                            insert_sibling(children, i, new_pane);
                            return true;
                        }
                    }
                }
                for child in children.iter_mut() {
                    if child
                        .node
                        .split_at_focused(direction, new_pane.clone(), window, cx)
                    {
                        return true;
                    }
                }
                false
            }
        }
    }

    pub fn split_first_leaf(&mut self, direction: SplitDirection, new_pane: Entity<Pane>) {
        match self {
            LayoutTree::Leaf(_) => {
                let old = std::mem::replace(self, LayoutTree::Leaf(new_pane.clone()));
                *self = LayoutTree::new_split(direction, old, LayoutTree::Leaf(new_pane));
            }
            LayoutTree::Container {
                direction: dir,
                children,
                ..
            } => {
                if *dir == direction
                    && matches!(children.first(), Some(c) if matches!(c.node, LayoutTree::Leaf(_)))
                {
                    insert_sibling(children, 0, new_pane);
                    return;
                }
                if let Some(first) = children.first_mut() {
                    first.node.split_first_leaf(direction, new_pane);
                }
            }
        }
    }

    pub fn split_at_pane(
        &mut self,
        target: &Entity<Pane>,
        direction: SplitDirection,
        new_pane: Entity<Pane>,
    ) -> bool {
        match self {
            LayoutTree::Leaf(pane) => {
                if pane == target {
                    let old = std::mem::replace(self, LayoutTree::Leaf(new_pane.clone()));
                    *self = LayoutTree::new_split(direction, old, LayoutTree::Leaf(new_pane));
                    true
                } else {
                    false
                }
            }
            LayoutTree::Container {
                direction: dir,
                children,
                ..
            } => {
                if *dir == direction {
                    for i in 0..children.len() {
                        if let LayoutTree::Leaf(pane) = &children[i].node
                            && pane == target
                        {
                            insert_sibling(children, i, new_pane);
                            return true;
                        }
                    }
                }
                for child in children.iter_mut() {
                    if child
                        .node
                        .split_at_pane(target, direction, new_pane.clone())
                    {
                        return true;
                    }
                }
                false
            }
        }
    }

    pub fn swap_panes(&mut self, a: &Entity<Pane>, b: &Entity<Pane>) -> bool {
        if a == b || !self.contains_leaf(a) || !self.contains_leaf(b) {
            return false;
        }
        self.swap_panes_unchecked(a, b);
        true
    }

    fn swap_panes_unchecked(&mut self, a: &Entity<Pane>, b: &Entity<Pane>) {
        match self {
            LayoutTree::Leaf(pane) => {
                if pane == a {
                    *pane = b.clone();
                } else if pane == b {
                    *pane = a.clone();
                }
            }
            LayoutTree::Container { children, .. } => {
                for child in children {
                    child.node.swap_panes_unchecked(a, b);
                }
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

    fn child_ratios(tree: &LayoutTree) -> Vec<f32> {
        match tree {
            LayoutTree::Container { children, .. } => {
                children.iter().map(|child| child.ratio.get()).collect()
            }
            LayoutTree::Leaf(_) => Vec::new(),
        }
    }

    #[gpui::test]
    fn split_at_pane_inserts_sibling_for_matching_direction(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let a = test_pane(cx, 1);
        let b = test_pane(cx, 1);
        let c = test_pane(cx, 1);
        let mut tree = LayoutTree::new_split(
            SplitDirection::Vertical,
            LayoutTree::Leaf(a.clone()),
            LayoutTree::Leaf(b.clone()),
        );

        assert!(tree.split_at_pane(&a, SplitDirection::Vertical, c.clone()));

        assert_eq!(tree.leaf_count(), 3);
        assert_eq!(
            leaf_ids(&tree),
            vec![a.entity_id(), c.entity_id(), b.entity_id()]
        );
        let ratios = child_ratios(&tree);
        assert_eq!(ratios.len(), 3);
        assert!((ratios[0] - 0.25).abs() < f32::EPSILON);
        assert!((ratios[1] - 0.25).abs() < f32::EPSILON);
        assert!((ratios[2] - 0.5).abs() < f32::EPSILON);
    }

    #[gpui::test]
    fn split_at_pane_wraps_cross_direction_target(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let a = test_pane(cx, 2);
        let b = test_pane(cx, 2);
        let c = test_pane(cx, 2);
        let mut tree = LayoutTree::new_split(
            SplitDirection::Vertical,
            LayoutTree::Leaf(a.clone()),
            LayoutTree::Leaf(b.clone()),
        );

        assert!(tree.split_at_pane(&a, SplitDirection::Horizontal, c.clone()));

        assert_eq!(tree.leaf_count(), 3);
        assert_eq!(
            leaf_ids(&tree),
            vec![a.entity_id(), c.entity_id(), b.entity_id()]
        );
        match tree {
            LayoutTree::Container { children, .. } => {
                assert!(matches!(children[0].node, LayoutTree::Container { .. }));
                assert!(matches!(children[1].node, LayoutTree::Leaf(_)));
            }
            LayoutTree::Leaf(_) => panic!("split should keep a container root"),
        }
    }

    #[gpui::test]
    fn swap_panes_refuses_absent_source(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let a = test_pane(cx, 3);
        let b = test_pane(cx, 3);
        let stale = test_pane(cx, 3);
        let mut tree = LayoutTree::new_split(
            SplitDirection::Vertical,
            LayoutTree::Leaf(a.clone()),
            LayoutTree::Leaf(b.clone()),
        );

        assert!(!tree.swap_panes(&stale, &b));
        assert_eq!(leaf_ids(&tree), vec![a.entity_id(), b.entity_id()]);
    }

    #[gpui::test]
    fn swap_panes_swaps_when_both_exist(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let a = test_pane(cx, 4);
        let b = test_pane(cx, 4);
        let mut tree = LayoutTree::new_split(
            SplitDirection::Vertical,
            LayoutTree::Leaf(a.clone()),
            LayoutTree::Leaf(b.clone()),
        );

        assert!(tree.swap_panes(&a, &b));
        assert_eq!(leaf_ids(&tree), vec![b.entity_id(), a.entity_id()]);
    }
}
