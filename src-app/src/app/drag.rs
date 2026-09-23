use gpui::{
    Context, FontWeight, IntoElement, ParentElement, Render, SharedString, Styled, Window, div, px,
};

#[derive(Clone)]
pub(crate) struct WorkspaceDrag {
    pub(crate) id: u64,
    pub(crate) title: SharedString,
}

#[derive(Clone)]
pub(crate) struct TabDrag {
    pub(crate) workspace_id: u64,
    pub(crate) tab_id: u64,
    pub(crate) title: SharedString,
}

pub(crate) struct WorkspaceDragPreview {
    pub(crate) title: SharedString,
}

impl Render for WorkspaceDragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        div()
            .w(px(crate::app::constants::SIDEBAR_WIDTH - 16.))
            .px(px(8.))
            .py(px(6.))
            .rounded(px(6.))
            .bg(ui.overlay)
            .border_1()
            .border_color(ui.text.opacity(0.12))
            .shadow_lg()
            .flex()
            .flex_col()
            .text_size(crate::ui_primitives::BODY_EMPHASIS)
            .font_weight(FontWeight::MEDIUM)
            .text_color(ui.text)
            .child(self.title.clone())
    }
}
