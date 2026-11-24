use gpui::{Context, Focusable, Window};
use paneflow_config::schema::LayoutNode;

use crate::layout::MAX_PANES;
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
        let Some(ws_idx) = self.shown_workspace_idx() else {
            return;
        };
        let ws = &self.workspaces[ws_idx];
        if ws.is_zoomed() {
            if let Some(pane) = self.exit_zoom_through_model(ws_idx, cx) {
                self.pending_pane_focus = None;
                pane.read(cx).focus_handle(cx).focus(window, cx);
            }
            return;
        }
        let Some(root) = &ws.active_tab().root else {
            return;
        };
        if root.leaf_count() <= 1 {
            return;
        }
        let Some(focused) = root.focused_pane(window, cx) else {
            return;
        };
        if self.zoom_pane_through_model(ws_idx, &focused, cx) {
            self.pending_pane_focus = None;
            focused.read(cx).focus_handle(cx).focus(window, cx);
        }
    }

    fn apply_layout_preset(
        &mut self,
        preset: paneflow_layout::Preset,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ws_idx) = self.shown_workspace_idx() else {
            return;
        };
        let tab = self.workspaces[ws_idx].active_tab();
        let Some(tree) = tab.saved_layout.as_ref().or(tab.root.as_ref()) else {
            return;
        };
        if tree.leaf_count() <= 1 {
            if tab.is_zoomed() {
                self.exit_zoom_through_model(ws_idx, cx);
            }
            return;
        }
        let main = matches!(preset, paneflow_layout::Preset::MainVertical)
            .then(|| {
                let root = self.workspaces[ws_idx].active_tab().root.as_ref()?;
                root.focused_pane(window, cx)
            })
            .flatten();
        self.adopt_layout(cx);
        if let Some(main) = main {
            self.hint_layout_focus(&main);
        }
        if self.apply_preset_through_model(preset, cx) {
            self.settle_layout_focus(window, cx);
        }
    }

    pub(crate) fn apply_layout_from_json(
        &mut self,
        layout: &mut LayoutNode,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        paneflow_config::schema::validate_layout(layout);
        let Some(ws_idx) = self.shown_workspace_idx() else {
            let needed = layout.leaf_count();
            if needed == 0 {
                return Err("Layout has no panes".into());
            }
            if needed > MAX_PANES {
                return Err(format!("Layout exceeds maximum pane count ({MAX_PANES})"));
            }
            return Err("No active workspace".into());
        };
        self.restore_layout_through_model(ws_idx, layout, cx)
    }

    pub(crate) fn apply_layout_to(
        &mut self,
        ws_idx: usize,
        layout: &mut LayoutNode,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        paneflow_config::schema::validate_layout(layout);
        self.restore_layout_through_model(ws_idx, layout, cx)
    }

    pub(crate) fn handle_layout_even_h(
        &mut self,
        _: &LayoutEvenHorizontal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_layout_preset(paneflow_layout::Preset::EvenHorizontal, window, cx);
    }

    pub(crate) fn handle_layout_even_v(
        &mut self,
        _: &LayoutEvenVertical,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_layout_preset(paneflow_layout::Preset::EvenVertical, window, cx);
    }

    pub(crate) fn handle_layout_main_v(
        &mut self,
        _: &LayoutMainVertical,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_layout_preset(paneflow_layout::Preset::MainVertical, window, cx);
    }

    pub(crate) fn handle_layout_tiled(
        &mut self,
        _: &LayoutTiled,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_layout_preset(paneflow_layout::Preset::Tiled, window, cx);
    }

    pub(crate) fn handle_split_equalize(
        &mut self,
        _: &SplitEqualize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.nav_root().is_some() {
            self.equalize_through_model(cx);
        }
    }
}
