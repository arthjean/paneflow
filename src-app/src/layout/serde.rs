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
                // EP-002 US-004: a pane holds exactly one surface, so the
                // serialized list has at most one entry and it is always the
                // focused one.
                let surfaces: Vec<SurfaceDefinition> = std::iter::once(&pane_ref.surface)
                    .map(|tab| match tab {
                        crate::pane::PaneSurface::Terminal(tv) => {
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
                                focus: Some(true),
                                scrollback,
                                agent: tv_ref.terminal.detected_agent.map(|a| a.tag().to_string()),
                                font_size: tv_ref.terminal.font_size_override,
                            }
                        }
                        crate::pane::PaneSurface::Markdown(markdown) => {
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
                                focus: Some(true),
                                scrollback: None,
                                agent: None,
                                font_size: None,
                            }
                        }
                        crate::pane::PaneSurface::Diff(_) => SurfaceDefinition {
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

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use gpui::{AppContext, Entity, TestAppContext};
    use paneflow_config::schema::LayoutNode;

    use crate::pane::Pane;
    use crate::terminal::TerminalView;
    use crate::workspace::{Tab, Workspace};

    use super::super::tree::{LayoutTree, SplitDirection};

    fn test_pane(cx: &mut impl AppContext) -> Entity<Pane> {
        let terminal = cx.new(|cx| TerminalView::display_only_for_test(1, cx));
        cx.new(|cx| Pane::new(terminal, 1, cx))
    }

    fn leaf_ids(tree: &LayoutTree) -> Vec<gpui::EntityId> {
        tree.collect_leaves()
            .into_iter()
            .map(|pane| pane.entity_id())
            .collect()
    }

    fn pane_leaf_count(node: &LayoutNode) -> usize {
        match node {
            LayoutNode::Pane { .. } => 1,
            LayoutNode::Split { children, .. } => children.iter().map(pane_leaf_count).sum(),
        }
    }

    /// US-002 (prd-cli-tab-hierarchy): serialization emits one `LayoutNode` per
    /// tab, the zoomed tab contributes its *saved* (un-zoomed) arrangement, and
    /// the tree survives a round-trip through `from_layout_node`. Switching the
    /// visible tab must not disturb either tab's zoom state.
    #[gpui::test]
    fn two_tab_workspace_with_one_zoomed_round_trips(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let (a, b, c, d) = (test_pane(cx), test_pane(cx), test_pane(cx), test_pane(cx));

        let flat = LayoutTree::new_split(
            SplitDirection::Vertical,
            LayoutTree::Leaf(a.clone()),
            LayoutTree::Leaf(b.clone()),
        );
        let mut ws = Workspace::with_layout_and_id(1, "ws", std::path::PathBuf::new(), flat);

        // Second tab, zoomed on `c`: root holds the single zoomed leaf while
        // `saved_layout` keeps the full arrangement.
        let mut zoomed = Tab::new("zoomed", Some(LayoutTree::Leaf(c.clone())));
        zoomed.saved_layout = Some(LayoutTree::new_split(
            SplitDirection::Horizontal,
            LayoutTree::Leaf(c.clone()),
            LayoutTree::Leaf(d.clone()),
        ));
        assert!(ws.open_tab(zoomed), "opening the second tab must succeed");
        assert_eq!(ws.active_tab_idx(), 1);
        assert!(ws.is_zoomed(), "the active tab is the zoomed one");

        // Per-tab serialization: the flat tab yields its two leaves, the zoomed
        // one yields the saved arrangement (two leaves), not the single leaf
        // `root` currently displays.
        let nodes: Vec<LayoutNode> = cx.update(|_, cx| {
            ws.tabs()
                .iter()
                .map(|tab| tab.serialize(cx).expect("every tab has a layout"))
                .collect()
        });
        assert_eq!(nodes.len(), 2, "one LayoutNode per tab");
        assert_eq!(pane_leaf_count(&nodes[0]), 2);
        assert_eq!(
            pane_leaf_count(&nodes[1]),
            2,
            "zoomed tab serializes its saved layout, not the zoomed leaf"
        );

        // Round-trip the zoomed tab's node back into a tree with the same panes.
        let mut panes: VecDeque<Entity<Pane>> = VecDeque::from(vec![c.clone(), d.clone()]);
        let restored = LayoutTree::from_layout_node(&nodes[1], &mut panes, &mut |_| {
            panic!("round-trip must not need to spawn a pane")
        });
        assert_eq!(
            leaf_ids(&restored),
            vec![c.entity_id(), d.entity_id()],
            "round-trip preserves the saved arrangement"
        );

        // Zoom is per tab and survives switching the visible tab.
        ws.set_active_tab(0);
        assert!(!ws.is_zoomed(), "the flat tab is not zoomed");
        ws.set_active_tab(1);
        assert!(ws.is_zoomed(), "the zoomed tab is still zoomed");
        assert_eq!(
            cx.update(|_, cx| ws.serialize_layout(cx).map(|n| pane_leaf_count(&n))),
            Some(2)
        );
    }
}
