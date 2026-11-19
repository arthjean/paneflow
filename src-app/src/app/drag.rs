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

#[derive(Clone)]
pub(crate) struct FileTreeDrag {
    pub(crate) path: std::path::PathBuf,
    pub(crate) root: std::path::PathBuf,
    pub(crate) label: SharedString,
    pub(crate) icon: Option<SharedString>,
}

impl Render for FileTreeDrag {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        div()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(6.))
            .bg(ui.overlay)
            .border_1()
            .border_color(ui.text.opacity(0.12))
            .shadow_lg()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .text_size(crate::ui_primitives::BODY_EMPHASIS)
            .text_color(ui.text)
            .children(
                self.icon
                    .clone()
                    .map(|icon| gpui::img(icon).size(px(16.)).flex_none()),
            )
            .child(self.label.clone())
    }
}

pub(crate) struct WorkspaceDragPreview {
    pub(crate) title: SharedString,
    pub(crate) width: f32,
}

impl Render for WorkspaceDragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        div()
            .w(px(self.width))
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
