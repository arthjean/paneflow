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
