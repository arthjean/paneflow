use std::ops::Range;

use gpui::{FontWeight, HighlightStyle, Pixels, StyledText, px};

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
