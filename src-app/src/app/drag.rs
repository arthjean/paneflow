//! Drag-and-drop payloads for sidebar workspace reordering.
//!
//! Extracted from `main.rs` per US-002. `WorkspaceDrag` is the payload
//! used as the drag value; `WorkspaceDragPreview` is a small floating
//! GPUI entity rendered under the cursor during the drag.

use gpui::{
    Context, FontWeight, IntoElement, ParentElement, Render, SharedString, Styled, Window, div, px,
    svg,
};

/// Drag payload used when reordering workspace cards in the sidebar.
#[derive(Clone)]
pub(crate) struct WorkspaceDrag {
    pub(crate) id: u64,
    pub(crate) source_idx: usize,
    pub(crate) title: SharedString,
    pub(crate) branch: Option<SharedString>,
}

/// Floating preview entity rendered under the cursor during a workspace drag.
pub(crate) struct WorkspaceDragPreview {
    pub(crate) title: SharedString,
    pub(crate) branch: Option<SharedString>,
}

impl Render for WorkspaceDragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        let mut preview = div()
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
            .child(self.title.clone());

        if let Some(branch) = self.branch.clone() {
            preview = preview.child(
                div()
                    .h(px(14.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.))
                    .text_xs()
                    .font_weight(FontWeight::NORMAL)
                    .text_color(ui.muted)
                    .child(
                        svg()
                            .size(px(10.))
                            .flex_none()
                            .path("icons/git-branch-sidebar.svg")
                            .text_color(ui.muted),
                    )
                    .child(branch),
            );
        }

        preview
    }
}
