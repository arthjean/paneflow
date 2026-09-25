use super::*;

pub(crate) const SIDEBAR_ROW_MARGIN_X: f32 = 8.0;
pub(crate) const SIDEBAR_ROW_PADDING_X: f32 = 7.0;
pub(crate) const SIDEBAR_ROW_PADDING_Y: f32 = 6.0;
pub(super) const SIDEBAR_ROW_GAP: f32 = 3.0;
pub(crate) const SIDEBAR_ROW_LINE_HEIGHT: f32 = 20.0;
pub(super) const SIDEBAR_TITLE_ROW_GAP: f32 = 8.0;
const SIDEBAR_ROW_MIN_HEIGHT: f32 = 32.0;
pub(super) const SIDEBAR_FOLDER_SLOT_WIDTH: f32 = 20.0;
pub(super) const ROW_RADIUS: gpui::Pixels = px(9.0);
pub(super) const SIDEBAR_ACTION_BUTTON_SIZE: f32 = 22.0;
pub(super) const SIDEBAR_ACTION_BUTTON_GAP: f32 = 1.0;
pub(super) const SIDEBAR_ROW_SPACING: f32 = 2.0;
pub(super) const SIDEBAR_GROUP_SPACING: f32 = SIDEBAR_ROW_SPACING + SIDEBAR_TITLE_ROW_GAP;
pub(super) const SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH: f32 =
    SIDEBAR_WIDTH - SIDEBAR_ROW_MARGIN_X * 2.0 - SIDEBAR_ROW_PADDING_X * 2.0;
pub(super) const SIDEBAR_HEADER_ICON_WIDTH: f32 = 15.0;
pub(super) const SIDEBAR_WORKSPACE_FOLDER_ICON_WIDTH: f32 = 15.0;
pub(super) const SIDEBAR_TITLE_TOOLTIP_MIN_CHARS: usize = 13;
pub(super) const SESSION_ROW_ICON_SIZE: f32 = 12.0;

pub(super) fn sidebar_row_shell() -> gpui::Div {
    div()
        .px(px(SIDEBAR_ROW_PADDING_X))
        .py(px(SIDEBAR_ROW_PADDING_Y))
        .min_h(px(SIDEBAR_ROW_MIN_HEIGHT))
        .justify_center()
        .flex_none()
        .relative()
        .overflow_x_hidden()
        .flex()
        .flex_col()
        .gap(px(SIDEBAR_ROW_GAP))
}

pub(super) fn render_sidebar_indent_guide(ui: crate::theme::UiColors) -> gpui::Div {
    let color = ui.text.opacity(0.08);
    let left = px(SIDEBAR_ROW_PADDING_X + (SIDEBAR_FOLDER_SLOT_WIDTH / 2.).floor());
    div()
        .absolute()
        .left(left)
        .top(px(-SIDEBAR_ROW_SPACING))
        .bottom_0()
        .w(px(1.))
        .bg(color)
}

pub(super) fn sidebar_row(
    shell: gpui::Stateful<gpui::Div>,
    group: SharedString,
    resting: Option<gpui::Hsla>,
    hovered: Option<gpui::Hsla>,
    body: impl IntoElement,
) -> gpui::Stateful<gpui::Div> {
    squircle_skin(shell, group, ROW_RADIUS, resting, hovered).child(body)
}

pub(super) fn sidebar_cursor_ring(ui: crate::theme::UiColors) -> impl IntoElement {
    crate::ui_primitives::squircle::squircle_border(ROW_RADIUS, px(1.), ui.text.opacity(0.35))
}

pub(super) fn sidebar_hover_actions(group: SharedString) -> gpui::Div {
    div()
        .absolute()
        .top(px(
            (SIDEBAR_ROW_LINE_HEIGHT - SIDEBAR_ACTION_BUTTON_SIZE) / 2.
        ))
        .right(px(0.))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(SIDEBAR_ACTION_BUTTON_GAP))
        .invisible()
        .group_hover(group, |style| style.visible())
}

pub(super) fn sidebar_action_button(
    id: SharedString,
    icon: &'static str,
    icon_size: f32,
    label: SharedString,
    ui: crate::theme::UiColors,
) -> gpui::Stateful<gpui::Div> {
    let active_bg = crate::app::constants::sidebar_tab_active_background();
    let group = SharedString::from(format!("{id}-hover"));
    squircle_skin(
        div()
            .id(id)
            .flex_none()
            .size(px(SIDEBAR_ACTION_BUTTON_SIZE))
            .flex()
            .items_center()
            .justify_center()
            .text_color(ui.muted),
        group.clone(),
        px(6.),
        None,
        Some(active_bg),
    )
    .role(Role::Button)
    .aria_label(label.clone())
    .delayed_tooltip(crate::ui_primitives::text_tooltip(label))
    .child(
        svg()
            .size(px(icon_size))
            .flex_none()
            .path(icon)
            .text_color(ui.muted)
            .group_hover(group, move |style| style.text_color(ui.text)),
    )
}

pub(super) fn tabular_numerals() -> gpui::FontFeatures {
    gpui::FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)]))
}

pub(super) fn render_diffstat_counts(
    stats: &crate::workspace::GitDiffStats,
    ui: crate::theme::UiColors,
) -> gpui::Div {
    div()
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(5.))
        .text_size(crate::ui_primitives::BODY)
        .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
        .font_features(tabular_numerals())
        .child(
            div()
                .text_color(ui.vc_added)
                .child(format!("+{}", stats.insertions)),
        )
        .child(
            div()
                .text_color(ui.vc_deleted)
                .child(format!("\u{2212}{}", stats.deletions)),
        )
}

impl PaneFlowApp {
    pub(super) fn inline_rename_field(&self, ui: crate::theme::UiColors) -> gpui::Div {
        div()
            .flex_1()
            .min_w_0()
            .overflow_x_hidden()
            .text_color(ui.text)
            .text_size(px(14.))
            .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
            .bg(ui.overlay)
            .px_1()
            .rounded_sm()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(self.rename_input.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use gpui::{
        AvailableSpace, InteractiveElement, ParentElement, Styled, TestAppContext, div, point, px,
        size,
    };

    #[gpui::test]
    fn row_glyphs_share_the_row_vertical_center(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(SIDEBAR_WIDTH)),
                AvailableSpace::Definite(px(100.)),
            ),
            |_, _| {
                sidebar_row_shell()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .child(
                                div()
                                    .flex_none()
                                    .size(px(SIDEBAR_FOLDER_SLOT_WIDTH))
                                    .debug_selector(|| "folder".into()),
                            )
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
                                    .debug_selector(|| "title".into())
                                    .child("paneflow"),
                            )
                            .child(
                                sidebar_hover_actions("row".into()).visible().child(
                                    div()
                                        .size(px(SIDEBAR_ACTION_BUTTON_SIZE))
                                        .debug_selector(|| "action".into()),
                                ),
                            ),
                    )
                    .debug_selector(|| "row".into())
            },
        );

        let row = cx.debug_bounds("row").expect("row not painted");
        for selector in ["folder", "title", "action"] {
            let glyph = cx.debug_bounds(selector).expect("glyph not painted");
            assert_eq!(
                glyph.center().y,
                row.center().y,
                "{selector} is off the row's vertical center"
            );
        }
    }

    #[gpui::test]
    fn a_single_line_row_is_thirty_two_pixels_tall(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(SIDEBAR_WIDTH)),
                AvailableSpace::Definite(px(100.)),
            ),
            |_, _| {
                sidebar_row_shell()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .child(div().flex_none().size(px(SIDEBAR_FOLDER_SLOT_WIDTH)))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
                                    .child("paneflow"),
                            ),
                    )
                    .debug_selector(|| "row".into())
            },
        );

        let bounds = cx.debug_bounds("row").expect("row not painted");
        assert_eq!(
            bounds.size.height,
            px((SIDEBAR_ROW_LINE_HEIGHT + 2. * SIDEBAR_ROW_PADDING_Y).max(32.)),
            "a title line must not let the font's own line height set the row height"
        );
        assert_eq!(bounds.size.height, px(32.));
        assert!(
            ROW_RADIUS <= bounds.size.height / 2.,
            "row corner {ROW_RADIUS:?} exceeds half of a {:?} row",
            bounds.size.height
        );
    }

    #[gpui::test]
    fn sidebar_workspace_rows_keep_height_when_list_overflows(cx: &mut TestAppContext) {
        const ROWS: [&str; 8] = [
            "sidebar-row-0",
            "sidebar-row-1",
            "sidebar-row-2",
            "sidebar-row-3",
            "sidebar-row-4",
            "sidebar-row-5",
            "sidebar-row-6",
            "sidebar-row-7",
        ];

        let cx = cx.add_empty_window();
        cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(240.)),
                AvailableSpace::Definite(px(200.)),
            ),
            |_, _| {
                let mut list = div()
                    .w(px(240.))
                    .h(px(200.))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .gap(px(4.));

                for selector in ROWS {
                    list = list.child(
                        sidebar_row_shell()
                            .child(div().h(px(20.)).flex_none())
                            .child(div().h(px(14.)).flex_none())
                            .debug_selector(move || selector.into()),
                    );
                }
                list
            },
        );

        for selector in ROWS {
            let bounds = cx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("{selector} not painted"));
            assert_eq!(bounds.size.height, px(49.), "{selector}");
        }
    }
}
