use std::ops::Range;

use gpui::{
    App, Bounds, ContentMask, FontWeight, Hsla, Pixels, Point, ShapedLine, TextAlign, Window, fill,
    point, px,
};

use super::element::{CodeScroll, code_font, syntax_text_runs};
use super::navigation::{MINIMAP_FONT_SIZE, MINIMAP_LINE_HEIGHT, Track, minimap_top};
use super::view::CodeView;

pub(crate) fn visible_rows(line_count: usize, scroll: &CodeScroll) -> Range<usize> {
    let first = minimap_top(line_count, scroll).floor() as usize;
    let count = (scroll.viewport_height() / MINIMAP_LINE_HEIGHT).ceil() as usize + 1;
    first.min(line_count)..first.saturating_add(count).min(line_count)
}

pub(crate) struct MinimapPaint {
    bounds: Bounds<Pixels>,
    background: Hsla,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
}

impl MinimapPaint {
    pub(crate) fn layout(
        view: &CodeView,
        track: Track,
        scroll: &CodeScroll,
        background: Hsla,
        foreground: Hsla,
        window: &mut Window,
    ) -> Option<Self> {
        let doc = view.document()?;
        let rows = visible_rows(doc.line_count(), scroll);
        let top = minimap_top(doc.line_count(), scroll);
        let mut font = code_font();
        font.family = ".ZedMono".into();
        font.weight = FontWeight::BLACK;
        let mut lines = Vec::with_capacity(rows.len());
        let mut syntax = Vec::new();
        for row in rows {
            let Some(slice) = doc.line(row) else {
                continue;
            };
            let text: String = slice.chars().take(160).collect();
            if text.is_empty() {
                continue;
            }
            syntax.clear();
            if let Some(highlighter) = view.highlighter() {
                highlighter.runs_into(row, &mut syntax);
            }
            let runs = syntax_text_runs(&text, &syntax, &font, foreground);
            let line =
                window
                    .text_system()
                    .shape_line(text.into(), px(MINIMAP_FONT_SIZE), &runs, None);
            lines.push((
                point(
                    track.bounds.origin.x,
                    track.bounds.origin.y
                        + px(((row as f64 - top) * f64::from(MINIMAP_LINE_HEIGHT)) as f32),
                ),
                line,
            ));
        }
        Some(Self {
            bounds: track.bounds,
            background,
            lines,
        })
    }

    pub(crate) fn paint(self, window: &mut Window, cx: &mut App) {
        window.with_content_mask(
            Some(ContentMask {
                bounds: self.bounds,
            }),
            |window| {
                window.paint_quad(fill(self.bounds, self.background.opacity(0.7)));
                for (origin, line) in self.lines {
                    let _ = line.paint(
                        origin,
                        px(MINIMAP_LINE_HEIGHT),
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }
            },
        );
    }
}
