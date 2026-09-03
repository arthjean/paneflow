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
            let Some(root) = &ws.active_tab().root else {
                return;
            };

            if root.leaf_count() <= 1 {
                return;
            }

            let Some(focused) = root.focused_pane(window, cx) else {
                return;
            };

            focused.update(cx, |p, _| p.zoomed = true);
            let full_tree = ws.active_tab_mut().root.take().unwrap();
            ws.active_tab_mut().saved_layout = Some(full_tree);
            ws.active_tab_mut().root = Some(LayoutTree::Leaf(focused.clone()));
            focused.read(cx).focus_handle(cx).focus(window, cx);
        }
        self.save_session(cx);
        cx.notify();
    }

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
        let Some(root) = ws.active_tab_mut().root.take() else {
            return;
        };
        let panes = root.collect_leaves();

        if panes.len() <= 1 {
            ws.active_tab_mut().root = Some(root);
            return;
        }

        drop(root);
        ws.active_tab_mut().root = build(panes);
        if let Some(ref r) = ws.active_tab().root {
            r.focus_first(window, cx);
        }
        self.save_session(cx);
        cx.notify();
    }

    pub(crate) fn apply_layout_from_json(
        &mut self,
        layout: &mut LayoutNode,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        paneflow_config::schema::validate_layout(layout);

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

        let existing: Vec<Entity<Pane>> = ws
            .active_tab_mut()
            .root
            .take()
            .map(|r| r.collect_leaves())
            .unwrap_or_default();

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
        ws.active_tab_mut().root = Some(tree);
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
        let Some(root) = &ws.active_tab().root else {
            return;
        };

        if root.leaf_count() <= 1 {
            return;
        }

        let main_pane = root
            .focused_pane(window, cx)
            .or_else(|| root.first_leaf())
            .unwrap();

        let panes = root.collect_leaves();
        let others: Vec<_> = panes.into_iter().filter(|p| *p != main_pane).collect();

        let ws = self.active_workspace_mut().unwrap();
        drop(ws.active_tab_mut().root.take());
        ws.active_tab_mut().root = LayoutTree::main_vertical(main_pane.clone(), others);
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
            && let Some(ref root) = ws.active_tab().root
        {
            root.equalize_ratios();
            self.save_session(cx);
            cx.notify();
        }
    }
}
