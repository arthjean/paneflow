//! Conversion between `LayoutTree` and `LayoutNode` (the session-persistence
//! schema). `serialize` captures each leaf's tabs + CWD + scrollback, while
//! `serialize_without_scrollback` keeps terminal output process-local. The
//! reverse `from_layout_node` consumes a pane deque and calls `spawn` for any
//! leaves beyond what was handed in.

use std::cell::Cell;
use std::collections::VecDeque;
use std::rc::Rc;

use gpui::{App, Entity};
use paneflow_config::schema::{LayoutNode, SurfaceDefinition};

use crate::pane::Pane;

use super::tree::{LayoutChild, LayoutTree, SplitDirection};

#[derive(Clone, Copy)]
enum ScrollbackCapture {
    Inline,
    Omit,
}

impl LayoutTree {
    /// Serialize the layout tree to a `LayoutNode` (config schema type).
    ///
    /// Each leaf produces a `LayoutNode::Pane` with one `SurfaceDefinition` per
    /// tab, capturing the terminal's CWD and OSC title. The active tab is marked
    /// with `focus: true`. Each container produces a `LayoutNode::Split` with
    /// per-child `ratios` and recursive children.
    pub fn serialize(&self, cx: &App) -> LayoutNode {
        self.serialize_with(cx, ScrollbackCapture::Inline)
    }

    /// Serialize session metadata without carrying terminal output into the
    /// next process. The schema field remains present as `None` for backward
    /// compatibility with existing session files.
    pub fn serialize_without_scrollback(&self, cx: &App) -> LayoutNode {
        self.serialize_with(cx, ScrollbackCapture::Omit)
    }

    /// Inner serializer parametrised by the [`ScrollbackCapture`] strategy.
    fn serialize_with(&self, cx: &App, capture: ScrollbackCapture) -> LayoutNode {
        match self {
            LayoutTree::Leaf(pane) => {
                let pane_ref = pane.read(cx);
                let surfaces: Vec<SurfaceDefinition> = pane_ref
                    .tabs
                    .iter()
                    .enumerate()
                    .map(|(i, tab)| match tab {
                        crate::pane::TabContent::Terminal(tv) => {
                            let tv_ref = tv.read(cx);
                            let name = if tv_ref.terminal.title.is_empty() {
                                None
                            } else {
                                Some(tv_ref.terminal.title.clone())
                            };
                            let cwd = tv_ref.terminal.current_cwd.clone().or_else(|| {
                                tv_ref.terminal.cwd_now().map(|p| p.display().to_string())
                            });
                            let scrollback = match capture {
                                ScrollbackCapture::Inline => tv_ref.terminal.extract_scrollback(),
                                ScrollbackCapture::Omit => None,
                            };
                            SurfaceDefinition {
                                surface_type: Some("terminal".to_string()),
                                name,
                                custom_name: tv_ref.terminal.custom_name.clone(),
                                command: None,
                                prompt: None,
                                cwd,
                                path: None,
                                env: None,
                                focus: (i == pane_ref.selected_idx).then_some(true),
                                scrollback,
                                agent: tv_ref.terminal.detected_agent.map(|a| a.tag().to_string()),
                                font_size: tv_ref.terminal.font_size_override,
                            }
                        }
                        crate::pane::TabContent::Markdown(markdown) => {
                            let path = markdown.read(cx).path.display().to_string();
                            SurfaceDefinition {
                                surface_type: Some("markdown".to_string()),
                                name: None,
                                custom_name: None,
                                command: None,
                                prompt: None,
                                cwd: None,
                                path: Some(path),
                                env: None,
                                focus: (i == pane_ref.selected_idx).then_some(true),
                                scrollback: None,
                                agent: None,
                                font_size: None,
                            }
                        }
                        crate::pane::TabContent::Diff(_) => SurfaceDefinition {
                            surface_type: Some("diff".to_string()),
                            name: None,
                            custom_name: None,
                            command: None,
                            prompt: None,
                            cwd: None,
                            path: None,
                            env: None,
                            focus: None,
                            scrollback: None,
                            agent: None,
                            font_size: None,
                        },
                    })
                    .filter(|surface| surface.surface_type.as_deref() != Some("diff"))
                    .collect();
                LayoutNode::Pane { surfaces }
            }
            LayoutTree::Container {
                direction,
                children,
                ..
            } => {
                let dir_str = match direction {
                    SplitDirection::Horizontal => "horizontal",
                    SplitDirection::Vertical => "vertical",
                };
                let ratios: Vec<f64> = children.iter().map(|c| c.ratio.get() as f64).collect();
                let mut child_nodes: Vec<LayoutNode> = Vec::with_capacity(children.len());
                for c in children.iter() {
                    child_nodes.push(c.node.serialize_with(cx, capture));
                }
                LayoutNode::Split {
                    direction: dir_str.to_string(),
                    ratio: None,
                    ratios: Some(ratios),
                    children: child_nodes,
                }
            }
        }
    }

    /// Rebuild a `LayoutTree` from a `LayoutNode` (config schema).
    ///
    /// Panes are consumed from `panes` in left-to-right order for each leaf.
    /// When `panes` is exhausted, `spawn` is called with the current `LayoutNode`
    /// so the caller can extract per-surface metadata (e.g. CWD) for new panes.
    pub fn from_layout_node(
        node: &LayoutNode,
        panes: &mut VecDeque<Entity<Pane>>,
        spawn: &mut impl FnMut(&LayoutNode) -> Entity<Pane>,
    ) -> Self {
        match node {
            LayoutNode::Pane { .. } => {
                let pane = panes.pop_front().unwrap_or_else(|| spawn(node));
                LayoutTree::Leaf(pane)
            }
            LayoutNode::Split {
                direction,
                children,
                ..
            } => {
                let dir = match direction.as_str() {
                    "vertical" => SplitDirection::Vertical,
                    _ => SplitDirection::Horizontal,
                };
                let resolved = node.resolved_ratios();
                let child_trees: Vec<LayoutChild> = children
                    .iter()
                    .enumerate()
                    .map(|(i, child_node)| {
                        let ratio = resolved
                            .get(i)
                            .copied()
                            .unwrap_or(1.0 / children.len() as f64);
                        LayoutChild {
                            node: LayoutTree::from_layout_node(child_node, panes, spawn),
                            ratio: Rc::new(Cell::new(ratio as f32)),
                        }
                    })
                    .collect();
                LayoutTree::Container {
                    direction: dir,
                    children: child_trees,
                    drag: Rc::new(Cell::new(None)),
                    container_size: Rc::new(Cell::new(0.0)),
                }
            }
        }
    }
}
