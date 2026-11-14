use std::ops::Range;

use gpui::{
    AnyElement, Div, FontWeight, HighlightStyle, Hsla, ParentElement, Pixels, Stateful, Styled,
    StyledText, px,
};

use crate::settings::components::{MENU_RADIUS, select_menu_surface, surface_edge, with_alpha};
use crate::theme::UiColors;
use crate::ui_primitives::interface_style::{CLASSIC_CONTROL, classic, pick};
use crate::ui_primitives::squircle::{squircle_border, squircle_fill};

pub(crate) const PANEL_RADIUS: Pixels = px(13.);
pub(crate) const PANEL_PADDING: f32 = 6.;
pub(crate) const FIELD_INSET: f32 = 2.;
pub(crate) const FIELD_GAP: f32 = 6.;
pub(crate) const ROW_HEIGHT: f32 = 29.;
pub(crate) const ROW_SPACING: f32 = 2.;
pub(crate) const ROW_RADIUS: Pixels = px(8.);
pub(crate) const ROW_TEXT_INSET: f32 = 11.;
pub(crate) const ROW_GAP: f32 = 8.;
pub(crate) const ICON_SIZE: f32 = 14.;
pub(crate) const ICON_GAP: f32 = 8.;
pub(crate) const LABEL_SIZE: f32 = 14.;

const HIGHLIGHT_WASH: f32 = 0.08;

const CLASSIC_ROW_HEIGHT: f32 = 36.;
const CLASSIC_ROW_SPACING: f32 = 0.;
const CLASSIC_HIGHLIGHT_WASH: f32 = 0.14;

pub(crate) struct RowColors {
    pub(crate) fill: Hsla,
    pub(crate) label: Hsla,
    pub(crate) muted: Hsla,
}

pub(crate) fn row_colors(selected: bool, ui: UiColors) -> RowColors {
    if selected && !classic() {
        return RowColors {
            fill: crate::theme::selection_color(),
            label: crate::theme::on_selection_color(),
            muted: crate::theme::on_selection_muted(),
        };
    }
    RowColors {
        fill: with_alpha(ui.text, pick(HIGHLIGHT_WASH, CLASSIC_HIGHLIGHT_WASH)),
        label: ui.text,
        muted: ui.muted,
    }
}

pub(crate) fn row_radius() -> Pixels {
    pick(ROW_RADIUS, CLASSIC_CONTROL)
}

pub(crate) fn row_height() -> f32 {
    pick(ROW_HEIGHT, CLASSIC_ROW_HEIGHT)
}

pub(crate) fn row_spacing() -> f32 {
    pick(ROW_SPACING, CLASSIC_ROW_SPACING)
}

pub(crate) fn panel_radius() -> Pixels {
    pick(PANEL_RADIUS, MENU_RADIUS)
}

pub(crate) fn shell(
    panel: Stateful<Div>,
    width: Pixels,
    ui: UiColors,
    field: AnyElement,
    list: AnyElement,
) -> Stateful<Div> {
    panel
        .relative()
        .child(squircle_fill(PANEL_RADIUS, select_menu_surface(ui)))
        .child(squircle_border(PANEL_RADIUS, px(1.), surface_edge(ui)))
        .flex()
        .flex_col()
        .p(px(PANEL_PADDING))
        .w(width)
        .child(field)
        .child(list)
}

pub(crate) fn highlighted_label(label: &str, highlights: &[Range<usize>]) -> StyledText {
    StyledText::new(label.to_string()).with_highlights(highlights.iter().map(|range| {
        (
            range.clone(),
            HighlightStyle {
                font_weight: Some(FontWeight::SEMIBOLD),
                ..Default::default()
            },
        )
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_primitives::interface_style::{Shape, scoped, with_paneflow_shape};

    #[test]
    fn classic_rows_swap_the_selection_fill_for_a_neutral_wash() {
        let ui = crate::theme::ui_colors();
        assert_eq!(row_colors(true, ui).fill, crate::theme::selection_color());
        assert_eq!(row_radius(), ROW_RADIUS);
        assert_eq!((row_height(), row_spacing()), (ROW_HEIGHT, ROW_SPACING));
        assert_eq!(panel_radius(), PANEL_RADIUS);
        scoped(Shape::Classic, || {
            let colors = row_colors(true, ui);
            assert_eq!(colors.fill, with_alpha(ui.text, CLASSIC_HIGHLIGHT_WASH));
            assert_eq!(colors.label, ui.text);
            assert_eq!(row_radius(), CLASSIC_CONTROL);
            assert_eq!((row_height(), row_spacing()), (36., 0.));
            assert_eq!(panel_radius(), MENU_RADIUS);
        });
    }

    #[test]
    fn the_command_palette_scope_keeps_the_paneflow_rows_in_the_classic_style() {
        let ui = crate::theme::ui_colors();
        scoped(Shape::Classic, || {
            with_paneflow_shape(|| {
                assert_eq!(row_colors(true, ui).fill, crate::theme::selection_color());
            });
        });
    }
}
