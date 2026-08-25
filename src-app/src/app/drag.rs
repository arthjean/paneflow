//! Drag-and-drop payloads for the sidebar: workspace reordering, and (US-011
//! of prd-cli-tab-hierarchy) tab reordering / reattachment.
//!
//! Extracted from `main.rs` per US-002. `WorkspaceDrag` is the payload
//! used as the drag value; `WorkspaceDragPreview` is a small floating
//! GPUI entity rendered under the cursor during the drag - it backs both
//! payloads, so a dragged tab and a dragged workspace read the same.

use gpui::{
    Context, FontWeight, IntoElement, ParentElement, Render, SharedString, Styled, Window, div, px,
};

/// Drag payload used when reordering workspace cards in the sidebar.
#[derive(Clone)]
pub(crate) struct WorkspaceDrag {
    pub(crate) id: u64,
    pub(crate) title: SharedString,
}

/// Drag payload used when reordering a tab inside its workspace, or dropping
/// it on another workspace's folder row to reattach it (US-011).
///
/// The payload carries ids only, never an index: the drop target is a gap
/// between two rows, and the handler re-resolves the tab by id after the
/// sidebar has re-rendered, so a stale index can never move the wrong tab.
#[derive(Clone)]
pub(crate) struct TabDrag {
    pub(crate) workspace_id: u64,
    pub(crate) tab_id: u64,
    pub(crate) title: SharedString,
}

/// Floating preview entity rendered under the cursor during a workspace drag.
pub(crate) struct WorkspaceDragPreview {
    pub(crate) title: SharedString,
}

impl Render for WorkspaceDragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        div()
            .w(px(crate::app::constants::SIDEBAR_WIDTH - 16.))
            .min_h(px(44.))
            .px(px(8.))
            .py(px(4.))
            .rounded(px(8.))
            .bg(ui.overlay)
            .border_1()
            .border_color(ui.text.opacity(0.12))
            .shadow_lg()
            .flex()
            .flex_col()
            .gap(px(4.))
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(ui.text)
            .child(self.title.clone())
    }
}
