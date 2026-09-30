use std::cell::Cell;
use std::rc::Rc;

use gpui::Entity;

use crate::pane::Pane;

use super::tree::{LayoutChild, LayoutTree, SplitDirection};

impl LayoutTree {
    pub fn from_panes_equal(direction: SplitDirection, panes: Vec<Entity<Pane>>) -> Option<Self> {
        match panes.len() {
            0 => None,
            1 => Some(LayoutTree::Leaf(panes.into_iter().next().unwrap())),
            n => {
                let ratio = 1.0 / n as f32;
                let children = panes
                    .into_iter()
                    .map(|pane| LayoutChild {
                        node: LayoutTree::Leaf(pane),
                        ratio: Rc::new(Cell::new(ratio)),
                    })
                    .collect();
                Some(LayoutTree::Container {
                    direction,
                    children,
                    drag: Rc::new(Cell::new(None)),
                    container_size: Rc::new(Cell::new(0.0)),
                })
            }
        }
    }

    pub fn main_vertical(main_pane: Entity<Pane>, others: Vec<Entity<Pane>>) -> Option<Self> {
        if others.is_empty() {
            return Some(LayoutTree::Leaf(main_pane));
        }

        let right = LayoutTree::from_panes_equal(SplitDirection::Horizontal, others)
            .expect("others is non-empty");

        Some(LayoutTree::Container {
            direction: SplitDirection::Vertical,
            children: vec![
                LayoutChild {
                    node: LayoutTree::Leaf(main_pane),
                    ratio: Rc::new(Cell::new(0.5)),
                },
                LayoutChild {
                    node: right,
                    ratio: Rc::new(Cell::new(0.5)),
                },
            ],
            drag: Rc::new(Cell::new(None)),
            container_size: Rc::new(Cell::new(0.0)),
        })
    }

    pub fn tiled(panes: Vec<Entity<Pane>>) -> Option<Self> {
        match panes.len() {
            0 => return None,
            1 => return Some(LayoutTree::Leaf(panes.into_iter().next().unwrap())),
            _ => {}
        }

        let n = panes.len();
        let mut rows = 1usize;
        let mut cols = 1usize;
        while rows * cols < n {
            if cols <= rows {
                cols += 1;
            } else {
                rows += 1;
            }
        }

        if rows == 1 {
            return LayoutTree::from_panes_equal(SplitDirection::Vertical, panes);
        }

        let row_ratio = 1.0 / rows as f32;
        let mut pane_iter = panes.into_iter();
        let mut row_children: Vec<LayoutChild> = Vec::with_capacity(rows);

        for r in 0..rows {
            let panes_in_row = if r < rows - 1 {
                cols
            } else {
                n - cols * (rows - 1)
            };

            let row_panes: Vec<Entity<Pane>> = pane_iter.by_ref().take(panes_in_row).collect();
            let row_tree = LayoutTree::from_panes_equal(SplitDirection::Vertical, row_panes)
                .expect("row is non-empty");

            row_children.push(LayoutChild {
                node: row_tree,
                ratio: Rc::new(Cell::new(row_ratio)),
            });
        }

        Some(LayoutTree::Container {
            direction: SplitDirection::Horizontal,
            children: row_children,
            drag: Rc::new(Cell::new(None)),
            container_size: Rc::new(Cell::new(0.0)),
        })
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext, Entity, TestAppContext};

    use crate::pane::Pane;
    use crate::terminal::TerminalView;

    use super::*;

    fn test_pane(cx: &mut impl AppContext) -> Entity<Pane> {
        let terminal = cx.new(|cx| TerminalView::display_only_for_test(1, cx));
        cx.new(|cx| Pane::new(terminal, 1, cx))
    }

    fn shape(tree: &LayoutTree) -> String {
        match tree {
            LayoutTree::Leaf(_) => "L".to_string(),
            LayoutTree::Container {
                direction,
                children,
                ..
            } => {
                let tag = match direction {
                    SplitDirection::Horizontal => "H",
                    SplitDirection::Vertical => "V",
                };
                let inner: Vec<String> = children.iter().map(|child| shape(&child.node)).collect();
                format!("{tag}[{}]", inner.join(","))
            }
        }
    }

    fn assert_no_single_child_container(tree: &LayoutTree, panes: usize) {
        if let LayoutTree::Container { children, .. } = tree {
            assert!(
                children.len() > 1,
                "{panes} panes: a container holds a single child in {}",
                shape(tree)
            );
            for child in children {
                assert_no_single_child_container(&child.node, panes);
            }
        }
    }

    #[gpui::test]
    fn tiled_grids_from_one_to_nine_panes(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let expected = [
            "L",
            "V[L,L]",
            "H[V[L,L],L]",
            "H[V[L,L],V[L,L]]",
            "H[V[L,L,L],V[L,L]]",
            "H[V[L,L,L],V[L,L,L]]",
            "H[V[L,L,L],V[L,L,L],L]",
            "H[V[L,L,L],V[L,L,L],V[L,L]]",
            "H[V[L,L,L],V[L,L,L],V[L,L,L]]",
        ];
        for (index, expected) in expected.into_iter().enumerate() {
            let count = index + 1;
            let panes: Vec<Entity<Pane>> = (0..count).map(|_| test_pane(cx)).collect();
            let ids: Vec<_> = panes.iter().map(Entity::entity_id).collect();
            let tree = LayoutTree::tiled(panes).expect("a non-empty grid");
            assert_eq!(shape(&tree), expected, "{count} panes");
            assert_no_single_child_container(&tree, count);
            let leaves: Vec<_> = tree
                .collect_leaves()
                .into_iter()
                .map(|pane| pane.entity_id())
                .collect();
            assert_eq!(leaves, ids, "{count} panes keep their order");
        }
        assert!(LayoutTree::tiled(Vec::new()).is_none());
    }
}
