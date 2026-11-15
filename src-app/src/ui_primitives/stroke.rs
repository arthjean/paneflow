use gpui::{LineCap, LineJoin, PathBuilder, PathStyle, Pixels, StrokeOptions};

pub(crate) fn round_stroke_options(width: Pixels) -> StrokeOptions {
    StrokeOptions {
        line_width: f32::from(width),
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..StrokeOptions::default()
    }
}

pub(crate) fn stroke_with(options: StrokeOptions) -> PathBuilder {
    PathBuilder::stroke(gpui::px(options.line_width)).with_style(PathStyle::Stroke(options))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, px};

    #[test]
    fn a_round_stroke_rounds_its_caps_and_joins_at_the_requested_width() {
        let options = round_stroke_options(px(2.5));
        assert_eq!(options.line_width, 2.5);
        assert_eq!(options.line_cap, LineCap::Round);
        assert_eq!(options.line_join, LineJoin::Round);
    }

    #[test]
    fn a_styled_stroke_builds_an_open_polyline() {
        let mut builder = stroke_with(round_stroke_options(px(2.)));
        builder.move_to(point(px(0.), px(0.)));
        builder.line_to(point(px(4.), px(4.)));
        builder.line_to(point(px(10.), px(-2.)));
        assert!(builder.build().is_ok());
    }
}
