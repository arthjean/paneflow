use std::sync::Arc;

use gpui::{
    App, Bounds, Hsla, LineLayout, Pixels, Point, ShapedLine, SharedString, TextAlign, TextRun,
    UnderlineStyle, Window, WindowTextSystem, black, point, px, underline_y_offset,
};
use unicode_width::UnicodeWidthChar;

pub(crate) const TAB_SIZE: usize = 4;

pub(crate) fn tab_len(column: usize) -> usize {
    TAB_SIZE - column % TAB_SIZE
}

pub(crate) fn char_columns(ch: char) -> usize {
    ch.width().unwrap_or(0)
}

pub(crate) fn advance(column: usize, ch: char) -> usize {
    if ch == '\t' {
        column + tab_len(column)
    } else {
        column + char_columns(ch)
    }
}

pub(crate) fn display_columns(text: &str) -> usize {
    advance_str(0, text)
}

pub(crate) fn advance_str(column: usize, text: &str) -> usize {
    if !text.is_ascii() {
        return text.chars().fold(column, advance);
    }
    text.bytes().fold(column, |column, byte| match byte {
        b'\t' => column + tab_len(column),
        byte if is_one_column_ascii(byte) => column + 1,
        _ => column,
    })
}

pub(crate) fn is_one_column_ascii(byte: u8) -> bool {
    (0x20..=0x7e).contains(&byte)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TabStop {
    buffer: usize,
    display: usize,
    len: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TabStops(Option<Arc<[TabStop]>>);

impl TabStops {
    pub(crate) fn expand(line: &SharedString) -> (SharedString, Self) {
        if !line.as_bytes().contains(&b'\t') {
            return (line.clone(), Self(None));
        }
        let mut expanded = String::with_capacity(line.len() + line.len() / 2);
        let mut stops = Vec::new();
        let mut column = 0usize;
        for (buffer, ch) in line.char_indices() {
            if ch == '\t' {
                let len = tab_len(column);
                stops.push(TabStop {
                    buffer,
                    display: expanded.len(),
                    len,
                });
                expanded.extend(std::iter::repeat_n(' ', len));
                column += len;
            } else {
                expanded.push(ch);
                column += char_columns(ch);
            }
        }
        (expanded.into(), Self(Some(stops.into())))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    pub(crate) fn to_display(&self, buffer: usize) -> usize {
        let Some(stops) = &self.0 else {
            return buffer;
        };
        let passed = stops.partition_point(|stop| stop.buffer < buffer);
        let Some(last) = passed.checked_sub(1).map(|index| stops[index]) else {
            return buffer;
        };
        last.display + last.len + (buffer - last.buffer - 1)
    }

    pub(crate) fn to_buffer(&self, display: usize) -> usize {
        let Some(stops) = &self.0 else {
            return display;
        };
        let passed = stops.partition_point(|stop| stop.display < display);
        let Some(last) = passed.checked_sub(1).map(|index| stops[index]) else {
            return display;
        };
        let into = display - last.display;
        if into < last.len {
            if into * 2 <= last.len {
                last.buffer
            } else {
                last.buffer + 1
            }
        } else {
            last.buffer + 1 + (into - last.len)
        }
    }

    pub(crate) fn remap_runs(&self, runs: &[TextRun], out: &mut Vec<TextRun>) {
        out.clear();
        let mut start = 0usize;
        for run in runs {
            let end = start + run.len;
            out.push(TextRun {
                len: self.to_display(end) - self.to_display(start),
                ..run.clone()
            });
            start = end;
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct PaintSpan {
    end: usize,
    color: Hsla,
    underline: Option<UnderlineStyle>,
}

#[derive(Clone, Debug)]
pub(crate) struct PaintedLine {
    shaped: ShapedLine,
    spans: Option<Arc<[PaintSpan]>>,
}

impl From<ShapedLine> for PaintedLine {
    fn from(shaped: ShapedLine) -> Self {
        Self {
            shaped,
            spans: None,
        }
    }
}

impl PaintedLine {
    pub(crate) fn shape(
        text: SharedString,
        font_size: Pixels,
        runs: &[TextRun],
        text_system: &WindowTextSystem,
    ) -> Self {
        if runs
            .iter()
            .any(|run| run.background_color.is_some() || run.strikethrough.is_some())
        {
            return text_system.shape_line(text, font_size, runs, None).into();
        }
        let shaped = text_system.shape_line(text, font_size, &layout_runs(runs), None);
        Self {
            shaped,
            spans: Some(paint_spans(runs)),
        }
    }

    #[cfg(test)]
    pub(crate) fn shaped(&self) -> &ShapedLine {
        &self.shaped
    }

    pub(crate) fn color_at(&self, index: usize) -> Option<Hsla> {
        let spans = self.spans.as_ref()?;
        span_at(spans, index).map(|span| span.color)
    }

    pub(crate) fn paint(
        &self,
        origin: Point<Pixels>,
        line_height: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) -> gpui::Result<()> {
        let Some(spans) = &self.spans else {
            return self
                .shaped
                .paint(origin, line_height, TextAlign::Left, None, window, cx);
        };
        let layout: &LineLayout = &self.shaped;
        let text_system = cx.text_system().clone();
        window.paint_layer(|window| {
            let padding_top = (line_height - layout.ascent - layout.descent) / 2.;
            let baseline = point(px(0.), padding_top + layout.ascent);
            let mask = window.content_mask().bounds;
            for run in &layout.runs {
                let glyph_size = text_system.bounding_box(run.font_id, layout.font_size).size;
                for glyph in &run.glyphs {
                    let glyph_origin = point(origin.x + glyph.position.x, origin.y);
                    let reach = Bounds {
                        origin: glyph_origin,
                        size: glyph_size,
                    };
                    if !reach.intersects(&mask) {
                        continue;
                    }
                    let at = glyph_origin + baseline + point(px(0.), glyph.position.y);
                    if glyph.is_emoji {
                        window.paint_emoji(at, run.font_id, glyph.id, layout.font_size)?;
                    } else {
                        let color = self.color_at(glyph.index).unwrap_or_else(black);
                        window.paint_glyph(at, run.font_id, glyph.id, layout.font_size, color)?;
                    }
                }
            }
            let underline_y =
                origin.y + underline_y_offset(line_height, layout.ascent, layout.descent);
            let mut start = 0usize;
            for span in spans.iter() {
                if let Some(style) = &span.underline {
                    let x0 = self.shaped.x_for_index(start);
                    let x1 = self.shaped.x_for_index(span.end.min(layout.len));
                    if x1 > x0 {
                        window.paint_underline(
                            point(origin.x + x0, underline_y),
                            x1 - x0,
                            &UnderlineStyle {
                                color: Some(style.color.unwrap_or(span.color)),
                                ..*style
                            },
                        );
                    }
                }
                start = span.end;
            }
            Ok(())
        })
    }
}

fn layout_runs(runs: &[TextRun]) -> Vec<TextRun> {
    let mut out: Vec<TextRun> = Vec::new();
    for run in runs {
        if let Some(last) = out.last_mut()
            && last.font == run.font
        {
            last.len += run.len;
            continue;
        }
        out.push(TextRun {
            len: run.len,
            font: run.font.clone(),
            color: black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    }
    out
}

fn paint_spans(runs: &[TextRun]) -> Arc<[PaintSpan]> {
    let mut spans: Vec<PaintSpan> = Vec::with_capacity(runs.len());
    let mut end = 0usize;
    for run in runs {
        end += run.len;
        if let Some(last) = spans.last_mut()
            && last.color == run.color
            && last.underline == run.underline
        {
            last.end = end;
            continue;
        }
        spans.push(PaintSpan {
            end,
            color: run.color,
            underline: run.underline,
        });
    }
    spans.into()
}

fn span_at(spans: &[PaintSpan], index: usize) -> Option<&PaintSpan> {
    spans.get(spans.partition_point(|span| span.end <= index))
}

#[derive(Clone, Debug)]
pub(crate) struct TabExpandedLine {
    line: PaintedLine,
    stops: TabStops,
}

impl TabExpandedLine {
    pub(crate) fn shape(
        text: &SharedString,
        font_size: Pixels,
        runs: &[TextRun],
        text_system: &WindowTextSystem,
    ) -> Self {
        let (display, stops) = TabStops::expand(text);
        let line = if stops.is_empty() {
            PaintedLine::shape(display, font_size, runs, text_system)
        } else {
            let mut display_runs = Vec::with_capacity(runs.len());
            stops.remap_runs(runs, &mut display_runs);
            PaintedLine::shape(display, font_size, &display_runs, text_system)
        };
        Self { line, stops }
    }

    pub(crate) fn x_for_index(&self, buffer: usize) -> Pixels {
        self.line.shaped.x_for_index(self.stops.to_display(buffer))
    }

    pub(crate) fn closest_index_for_x(&self, x: Pixels) -> usize {
        self.stops
            .to_buffer(self.line.shaped.closest_index_for_x(x))
    }

    pub(crate) fn width(&self) -> Pixels {
        self.line.shaped.width()
    }

    #[cfg(test)]
    pub(crate) fn shaped(&self) -> &ShapedLine {
        &self.line.shaped
    }

    pub(crate) fn painted(&self) -> &PaintedLine {
        &self.line
    }

    pub(crate) fn into_painted(self) -> PaintedLine {
        self.line
    }
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, font, hsla, px};

    use super::*;

    fn expand(text: &str) -> (String, TabStops) {
        let (display, stops) = TabStops::expand(&SharedString::from(text.to_string()));
        (display.to_string(), stops)
    }

    fn zed_tab_map_expansion(line: &str) -> String {
        let mut out = String::new();
        let mut expanded_chars = 0u32;
        for ch in line.chars() {
            if ch == '\t' {
                let tab_size = TAB_SIZE as u32;
                let tab_len = tab_size - (expanded_chars % tab_size);
                out.extend(std::iter::repeat_n(' ', tab_len as usize));
                expanded_chars += tab_len;
            } else {
                out.push(ch);
                expanded_chars += 1;
            }
        }
        out
    }

    fn run(len: usize, alpha: f32) -> TextRun {
        TextRun {
            len,
            font: font("Lilex"),
            color: hsla(0., 0., 0., alpha),
            background_color: None,
            underline: None,
            strikethrough: None,
        }
    }

    #[test]
    fn each_tab_reaches_the_next_multiple_of_four_columns() {
        assert_eq!(expand("\tx").0, "    x");
        assert_eq!(expand("a\tb").0, "a   b");
        assert_eq!(expand("abc\td").0, "abc d");
        assert_eq!(expand("abcd\te").0, "abcd    e");
        assert_eq!(expand("\t\tx").0, "        x");
        assert_eq!(expand("ab\t\tc").0, "ab      c");
    }

    #[test]
    fn ascii_expansion_matches_the_zed_tab_map_rule() {
        for line in [
            "\t",
            "a\t",
            "ab\tc\td",
            "abcd\t\t",
            "fn x() {\n",
            "\t\t\tlet x = 1;\t// x",
        ] {
            assert_eq!(expand(line).0, zed_tab_map_expansion(line), "{line:?}");
        }
    }

    #[test]
    fn a_line_without_tabs_is_shared_untouched() {
        let line = SharedString::from("let x = 1;".to_string());
        let (display, stops) = TabStops::expand(&line);
        assert!(stops.is_empty());
        assert_eq!(display, line);
        assert_eq!(stops.to_display(4), 4);
        assert_eq!(stops.to_buffer(4), 4);
    }

    #[test]
    fn a_double_width_character_counts_two_columns_before_a_tab() {
        assert_eq!(expand("中\tx").0, "中  x");
        assert_eq!(expand("a中\tx").0, "a中 x");
        assert_eq!(expand("👍\tx").0, "👍  x");
        assert_eq!(expand("中中\tx").0, "中中    x");
        assert_eq!(display_columns("中\tx"), 5);
    }

    #[test]
    fn a_combining_mark_takes_no_column_before_a_tab() {
        assert_eq!(expand("e\u{301}\tx").0, "e\u{301}   x");
    }

    #[test]
    fn buffer_offsets_map_to_expanded_columns_and_back() {
        let (display, stops) = expand("a\tbc\td");
        assert_eq!(display, "a   bc  d");
        let pairs = [(0, 0), (1, 1), (2, 4), (3, 5), (4, 6), (5, 8), (6, 9)];
        for (buffer, column) in pairs {
            assert_eq!(stops.to_display(buffer), column, "buffer {buffer}");
            assert_eq!(stops.to_buffer(column), buffer, "column {column}");
        }
    }

    #[test]
    fn a_column_inside_a_tab_span_lands_on_the_nearer_side() {
        let (_, stops) = expand("a\tb");
        assert_eq!(stops.to_buffer(1), 1);
        assert_eq!(stops.to_buffer(2), 1);
        assert_eq!(stops.to_buffer(3), 2);
        assert_eq!(stops.to_buffer(4), 2);
        let (_, stops) = expand("\tb");
        assert_eq!(stops.to_buffer(1), 0);
        assert_eq!(stops.to_buffer(2), 0);
        assert_eq!(stops.to_buffer(3), 1);
    }

    #[test]
    fn expansion_does_not_depend_on_the_platform_text_system() {
        let (display, stops) = expand("\tif x {\n");
        assert_eq!(display, "    if x {\n");
        assert_eq!(stops.to_display(1) - stops.to_display(0), TAB_SIZE);
    }

    #[test]
    fn runs_stretch_over_the_tabs_they_cover() {
        let (display, stops) = expand("a\tbc\td");
        let mut out = Vec::new();
        stops.remap_runs(&[run(2, 0.1), run(3, 0.2), run(1, 0.3)], &mut out);
        let lens: Vec<usize> = out.iter().map(|run| run.len).collect();
        assert_eq!(lens, vec![4, 4, 1]);
        assert_eq!(lens.iter().sum::<usize>(), display.len());
        assert_eq!(out[1].color, hsla(0., 0., 0., 0.2));
    }

    #[test]
    fn a_word_range_that_spans_a_tab_covers_the_expanded_width() {
        let (_, stops) = expand("x\ty");
        assert_eq!(stops.to_display(0)..stops.to_display(3), 0..5);
        assert_eq!(stops.to_display(1)..stops.to_display(2), 1..4);
    }

    #[test]
    fn ten_thousand_tabs_expand_in_one_linear_pass() {
        let line = "\t".repeat(10_000);
        let (display, stops) = expand(&line);
        assert_eq!(display.len(), 40_000);
        assert!(display.bytes().all(|byte| byte == b' '));
        assert_eq!(stops.to_display(10_000), 40_000);
        assert_eq!(stops.to_display(5_000), 20_000);
        assert_eq!(stops.to_buffer(40_000), 10_000);
        assert_eq!(stops.to_buffer(20_001), 5_000);
    }

    #[gpui::test]
    fn a_shaped_line_maps_carets_through_the_expansion(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, _cx| {
            let text = SharedString::from("a\tb".to_string());
            let line = TabExpandedLine::shape(&text, px(14.), &[run(3, 1.)], window.text_system());
            assert_eq!(line.shaped().text.as_ref(), "a   b");
            let one = line.shaped().x_for_index(1);
            assert_eq!(line.x_for_index(1), one);
            assert_eq!(line.x_for_index(2), line.shaped().x_for_index(4));
            assert!(line.x_for_index(2) > one);
            let near_left = line.shaped().x_for_index(2);
            assert_eq!(line.closest_index_for_x(near_left), 1);
            let near_right = line.shaped().x_for_index(4);
            assert_eq!(line.closest_index_for_x(near_right), 2);
        });
    }

    #[test]
    fn advancing_by_a_whole_string_matches_advancing_one_character_at_a_time() {
        let every_ascii_byte: String = (0u8..=0x7f).map(char::from).collect();
        let texts = [
            "",
            "abc",
            "\t",
            "\tx",
            "a\tb\t\tc",
            "\t\t\t",
            "\x1b[31m\tred\x7f\t",
            "é\tñ",
            "日本\ta",
            "x\n\ty",
            every_ascii_byte.as_str(),
        ];
        for text in texts {
            for start in 0..2 * TAB_SIZE {
                assert_eq!(
                    advance_str(start, text),
                    text.chars().fold(start, advance),
                    "{text:?} from column {start}"
                );
            }
        }
    }

    fn colored(len: usize, hue: f32) -> TextRun {
        TextRun {
            len,
            font: font("Lilex"),
            color: hsla(hue, 0.8, 0.6, 1.),
            background_color: None,
            underline: None,
            strikethrough: None,
        }
    }

    fn token_runs(text: &str) -> Vec<TextRun> {
        let mut runs = Vec::new();
        let mut start = 0usize;
        for (index, word) in text.split_inclusive(' ').enumerate() {
            let hue = (index % 7) as f32 / 7.;
            runs.push(colored(word.len(), hue));
            start += word.len();
        }
        assert_eq!(start, text.len());
        runs
    }

    #[gpui::test]
    fn recoloring_a_line_with_new_token_boundaries_reuses_its_glyph_layout(
        cx: &mut TestAppContext,
    ) {
        let window = cx.add_empty_window();
        window.update(|window, _cx| {
            let text = SharedString::from("const answer = 42;".to_string());
            let lexical = [colored(5, 0.1), colored(13, 0.5)];
            let syntax = [
                colored(5, 0.1),
                colored(1, 0.5),
                colored(6, 0.3),
                colored(6, 0.5),
            ];
            let first = TabExpandedLine::shape(&text, px(14.), &lexical, window.text_system());
            let second = TabExpandedLine::shape(&text, px(14.), &syntax, window.text_system());
            let first_layout: &LineLayout = first.painted().shaped();
            let second_layout: &LineLayout = second.painted().shaped();
            assert!(std::ptr::eq(first_layout, second_layout));
            assert_eq!(second.painted().color_at(6), Some(hsla(0.3, 0.8, 0.6, 1.)));
        });
    }

    #[gpui::test]
    fn every_glyph_of_a_ten_thousand_byte_line_takes_its_run_color(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, _cx| {
            let mut text = String::from("const ");
            while text.len() < 10_001 {
                text.push_str("v = f(x) + y; ");
            }
            text.truncate(10_001);
            let runs = token_runs(&text);
            assert!(runs.len() > 2_000);
            let line =
                TabExpandedLine::shape(&text.clone().into(), px(14.), &runs, window.text_system());
            let painted = line.painted();
            let mut expected = Vec::with_capacity(text.len());
            for run in &runs {
                expected.extend(std::iter::repeat_n(run.color, run.len));
            }
            let mut glyphs = 0usize;
            for shaped_run in &painted.shaped().runs {
                for glyph in &shaped_run.glyphs {
                    assert_eq!(
                        painted.color_at(glyph.index),
                        Some(expected[glyph.index]),
                        "glyph at byte {}",
                        glyph.index
                    );
                    glyphs += 1;
                }
            }
            assert!(glyphs > 9_000);
            assert_eq!(painted.color_at(0), Some(runs[0].color));
        });
    }

    #[gpui::test]
    fn a_glyph_after_a_tab_takes_the_color_of_its_buffer_run(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, _cx| {
            let text = SharedString::from("	if x".to_string());
            let runs = [colored(1, 0.0), colored(2, 0.2), colored(2, 0.6)];
            let line = TabExpandedLine::shape(&text, px(14.), &runs, window.text_system());
            assert_eq!(line.painted().color_at(4), Some(hsla(0.2, 0.8, 0.6, 1.)));
            assert_eq!(line.painted().color_at(7), Some(hsla(0.6, 0.8, 0.6, 1.)));
        });
    }

    #[gpui::test]
    fn a_run_with_a_background_keeps_gpui_decoration_painting(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, _cx| {
            let text = SharedString::from("abc".to_string());
            let mut run = colored(3, 0.4);
            run.background_color = Some(hsla(0., 0., 0.2, 1.));
            let line = TabExpandedLine::shape(&text, px(14.), &[run], window.text_system());
            assert_eq!(line.painted().color_at(0), None);
        });
    }
}
