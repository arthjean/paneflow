//! Layout presets, JSON layout application, zoom, and split-equalize.
//!
//! Part of the US-023 workspace_ops decomposition.

use std::collections::VecDeque;
use std::path::PathBuf;

use gpui::{Context, Entity, Focusable, Window};
use paneflow_config::schema::LayoutNode;

use crate::layout::{LayoutTree, MAX_PANES, SplitDirection};
use crate::pane::Pane;
use crate::{
    LayoutEvenHorizontal, LayoutEvenVertical, LayoutMainVertical, LayoutTiled, PaneFlowApp,
    SplitEqualize, ToggleZoom,
};

impl PaneFlowApp {
    pub(crate) fn handle_toggle_zoom(
        &mut self,
        _: &ToggleZoom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.active_workspace_mut() else {
            return;
        };

        if ws.is_zoomed() {
            if let Some(pane) = ws.exit_zoom(cx) {
                pane.read(cx).focus_handle(cx).focus(window, cx);
            }
        } else {
            // Zoom: save the full tree, replace root with the focused pane
            let Some(root) = &ws.root else { return };

            if root.leaf_count() <= 1 {
                return;
            }

            let Some(focused) = root.focused_pane(window, cx) else {
                return;
            };

            focused.update(cx, |p, _| p.zoomed = true);
            let full_tree = ws.root.take().unwrap();
            ws.saved_layout = Some(full_tree);
            ws.root = Some(LayoutTree::Leaf(focused.clone()));
            focused.read(cx).focus_handle(cx).focus(window, cx);
        }
        self.save_session(cx);
        cx.notify();
    }

    /// Apply a layout preset: collect all panes, rebuild tree with the given factory.
    fn apply_layout_preset(
        &mut self,
        build: impl FnOnce(Vec<Entity<Pane>>) -> Option<LayoutTree>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ws) = self.active_workspace_mut() {
            ws.exit_zoom(cx);
        }

        let Some(ws) = self.active_workspace_mut() else {
            return;
        };
        let Some(root) = ws.root.take() else { return };
        let panes = root.collect_leaves();

        // No-op for single pane
        if panes.len() <= 1 {
            ws.root = Some(root);
            return;
        }

        // Rebuild tree and focus first pane
        // (root is consumed by collect_leaves moving entities out - but collect_leaves
        //  clones Entity refs, so root is still valid. We drop it explicitly.)
        drop(root);
        ws.root = build(panes);
        if let Some(ref r) = ws.root {
            r.focus_first(window, cx);
        }
        self.save_session(cx);
        cx.notify();
    }

    /// Apply a layout from a `LayoutNode` (deserialized JSON) to the active workspace.
    ///
    /// Handles pane count mismatch: spawns new panes when the layout has more
    /// leaves than available, drops extras when fewer. Exits zoom first.
    pub(crate) fn apply_layout_from_json(
        &mut self,
        layout: &mut LayoutNode,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        // Validate the layout (clamps ratios, pads children, etc.)
        paneflow_config::loader::validate_layout(layout);

        let needed = layout.leaf_count();
        if needed == 0 {
            return Err("Layout has no panes".into());
        }
        if needed > MAX_PANES {
            return Err(format!("Layout exceeds maximum pane count ({MAX_PANES})"));
        }

        if let Some(ws) = self.active_workspace_mut() {
            ws.exit_zoom(cx);
        }

        let Some(ws) = self.active_workspace_mut() else {
            return Err("No active workspace".into());
        };

        // Collect existing panes and drop the old tree
        let existing: Vec<Entity<Pane>> = ws
            .root
            .take()
            .map(|r| r.collect_leaves())
            .unwrap_or_default();

        // Keep only the panes we need; extras are dropped with the old tree
        let mut pane_deque: VecDeque<Entity<Pane>> = existing.into_iter().take(needed).collect();

        let ws_id = ws.id;
        let fallback_cwd = PathBuf::from(&ws.cwd);
        let tree = LayoutTree::from_layout_node(layout, &mut pane_deque, &mut |node| {
            let surfaces = match node {
                LayoutNode::Pane { surfaces } => surfaces.as_slice(),
                _ => &[],
            };
            PaneFlowApp::spawn_pane_from_surfaces(ws_id, surfaces, &fallback_cwd, cx)
        });

        let Some(ws) = self.active_workspace_mut() else {
            return Err("No active workspace".into());
        };
        ws.root = Some(tree);
        self.save_session(cx);
        cx.notify();

        Ok(())
    }

    pub(crate) fn handle_layout_even_h(
        &mut self,
        _: &LayoutEvenHorizontal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_layout_preset(
            |panes| LayoutTree::from_panes_equal(SplitDirection::Vertical, panes),
            window,
            cx,
        );
    }

    pub(crate) fn handle_layout_even_v(
        &mut self,
        _: &LayoutEvenVertical,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_layout_preset(
            |panes| LayoutTree::from_panes_equal(SplitDirection::Horizontal, panes),
            window,
            cx,
        );
    }

    pub(crate) fn handle_layout_main_v(
        &mut self,
        _: &LayoutMainVertical,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ws) = self.active_workspace_mut() {
            ws.exit_zoom(cx);
        }

        let Some(ws) = self.active_workspace() else {
            return;
        };
        let Some(root) = &ws.root else { return };

        if root.leaf_count() <= 1 {
            return;
        }

        // The main pane is the focused one, or the first leaf
        let main_pane = root
            .focused_pane(window, cx)
            .or_else(|| root.first_leaf())
            .unwrap();

        let panes = root.collect_leaves();
        let others: Vec<_> = panes.into_iter().filter(|p| *p != main_pane).collect();

        let ws = self.active_workspace_mut().unwrap();
        drop(ws.root.take());
        ws.root = LayoutTree::main_vertical(main_pane.clone(), others);
        main_pane.read(cx).focus_handle(cx).focus(window, cx);
        self.save_session(cx);
        cx.notify();
    }

    pub(crate) fn handle_layout_tiled(
        &mut self,
        _: &LayoutTiled,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_layout_preset(LayoutTree::tiled, window, cx);
    }

    pub(crate) fn handle_split_equalize(
        &mut self,
        _: &SplitEqualize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ws) = self.active_workspace_mut()
            && let Some(ref root) = ws.root
        {
            root.equalize_ratios();
            self.save_session(cx);
            cx.notify();
        }
    }
}
