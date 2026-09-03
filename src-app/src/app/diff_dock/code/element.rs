use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, BorderStyle, Bounds, ContentMask, Corners, Element, ElementId, ElementInputHandler,
    Entity, Focusable, Font, FontFeatures, FontStyle, FontWeight, GlobalElementId, Hsla,
    InspectorElementId, IntoElement, LayoutId, Length, Pixels, Point, ShapedLine, SharedString,
    Style, TextAlign, TextRun, UnderlineStyle, Window, fill, point, px, quad, relative, size,
};

use super::cursor;
use super::document::CodeDocument;
use super::view::CodeView;
use crate::diff::{ROW_HEIGHT, RowPalette};

pub(crate) const CODE_ROW_HEIGHT: f32 = ROW_HEIGHT;
pub(crate) const CODE_FONT_SIZE: f32 = 12.0;
const NUM_GAP: f32 = 6.0;
const GUTTER_PAD_L: f32 = 8.0;
const GUTTER_MIN_W: f32 = 36.0;
const CODE_PAD_L: f32 = 6.0;
const CODE_PAD_R: f32 = 8.0;
const H_SCROLL_MARGIN: f32 = 12.0;
const CARET_WIDTH: f32 = 2.0;

const V_SCROLLBAR_W: f32 = 6.0;
const V_SCROLLBAR_INSET: f32 = 2.0;
const H_SCROLLBAR_H: f32 = 6.0;
const H_SCROLLBAR_INSET: f32 = 3.0;
const H_SCROLLBAR_MIN_THUMB: f32 = 28.0;
const SCROLL_EPSILON: f32 = 0.5;
const REVEAL_MARGIN_ROWS: f32 = 2.0;

const OVERDRAW_ROWS: usize = 1;

pub(crate) fn visible_rows(content_top: f32, viewport_h: f32, line_count: usize) -> Range<usize> {
    if line_count == 0 || viewport_h <= 0.0 {
        return 0..0;
    }
    let top = content_top.max(0.0);
    let first = ((top / CODE_ROW_HEIGHT) as usize).saturating_sub(OVERDRAW_ROWS);
    let bottom = top + viewport_h;
    let last = (bottom / CODE_ROW_HEIGHT) as usize + 1 + OVERDRAW_ROWS;
    first.min(line_count)..last.min(line_count)
}

pub(crate) fn digit_count(n: usize) -> usize {
    let mut n = n.max(1);
    let mut count = 0usize;
    while n > 0 {
        count += 1;
        n /= 10;
    }
    count
}

pub(crate) fn gutter_width(digits: usize, digit_w: f32) -> f32 {
    (GUTTER_PAD_L + digits as f32 * digit_w + NUM_GAP).max(GUTTER_MIN_W)
}

pub(crate) fn text_viewport_width(element_w: f32, gutter_w: f32) -> f32 {
    (element_w - gutter_w - CODE_PAD_L - CODE_PAD_R).max(0.0)
}

pub(crate) fn max_h_offset(longest_line_chars: usize, char_w: f32, text_viewport_w: f32) -> f32 {
    let content_w = longest_line_chars as f32 * char_w + H_SCROLL_MARGIN;
    (content_w - text_viewport_w).max(0.0)
}

pub(crate) fn h_thumb(offset: f32, max_offset: f32, track_w: f32) -> Option<(f32, f32)> {
    if track_w <= 0.0 || max_offset < SCROLL_EPSILON {
        return None;
    }
    let content_w = track_w + max_offset;
    let thumb_w = (track_w * track_w / content_w)
        .max(H_SCROLLBAR_MIN_THUMB)
        .min(track_w);
    let progress = (offset / max_offset).clamp(0.0, 1.0);
    Some((progress * (track_w - thumb_w), thumb_w))
}

pub(crate) fn reveal_offset(row: usize, viewport_h: f32, content_h: f32, offset_y: f32) -> f32 {
    if viewport_h <= 0.0 {
        return offset_y;
    }
    let max_off = (content_h - viewport_h).max(0.0);
    let margin = (REVEAL_MARGIN_ROWS * CODE_ROW_HEIGHT).min((viewport_h - CODE_ROW_HEIGHT) / 2.0);
    let margin = margin.max(0.0);
    let row_top = row as f32 * CODE_ROW_HEIGHT;
    let row_bottom = row_top + CODE_ROW_HEIGHT;
    let mut top = -offset_y;
    if row_top - margin < top {
        top = row_top - margin;
    } else if row_bottom + margin > top + viewport_h {
        top = row_bottom + margin - viewport_h;
    }
    -top.clamp(0.0, max_off)
}

pub(crate) fn reveal_h_offset(
    caret_x: f32,
    text_viewport_w: f32,
    max_offset: f32,
    current: f32,
) -> f32 {
    if text_viewport_w <= 0.0 {
        return current;
    }
    let margin = H_SCROLL_MARGIN.min(text_viewport_w / 2.0);
    let mut next = current;
    if caret_x - margin < next {
        next = caret_x - margin;
    } else if caret_x + margin > next + text_viewport_w {
        next = caret_x + margin - text_viewport_w;
    }
    next.clamp(0.0, max_offset)
}

pub(crate) fn autoscroll_step(pos: f32, lo: f32, hi: f32, step: f32) -> f32 {
    if pos < lo {
        -step
    } else if pos > hi {
        step
    } else {
        0.0
    }
}

const UNFOCUSED_WASH_FACTOR: f32 = 1.0 / 3.0;

pub(crate) fn cursor_line_wash(base: Hsla, focused: bool) -> Hsla {
    if focused {
        base
    } else {
        base.opacity(UNFOCUSED_WASH_FACTOR)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct CodeGeometry {
    pub(crate) gutter_w: f32,
    pub(crate) char_w: f32,
    pub(crate) text_viewport_w: f32,
    pub(crate) max_h_offset: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct GutterMemo {
    pub(crate) digits: usize,
    pub(crate) digit_w: f32,
    pub(crate) gutter_w: f32,
}

pub(crate) fn row_selection(
    sel: &Range<usize>,
    line: &Range<usize>,
) -> Option<(Range<usize>, bool)> {
    if sel.start >= sel.end || sel.end <= line.start || sel.start > line.end {
        return None;
    }
    let start = sel.start.max(line.start) - line.start;
    let end = sel.end.min(line.end) - line.start;
    Some((start..end, sel.end > line.end))
}

struct Quad {
    bounds: Bounds<Pixels>,
    color: Hsla,
    clip: Option<Bounds<Pixels>>,
}

struct RoundedQuad {
    bounds: Bounds<Pixels>,
    corners: Corners<Pixels>,
    color: Hsla,
}

struct CodeGlyph {
    origin: Point<Pixels>,
    line: ShapedLine,
    clip: Option<Bounds<Pixels>>,
}

#[derive(Clone, Copy)]
pub(crate) struct CodeColors {
    pub(crate) scrollbar_thumb: Hsla,
    pub(crate) cursor: Hsla,
    pub(crate) selection: Hsla,
    pub(crate) selection_fg: Hsla,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CodeCaret {
    pub(crate) cursor: usize,
    pub(crate) selection: Range<usize>,
    pub(crate) focused: bool,
    pub(crate) visible: bool,
    pub(crate) marked: Range<usize>,
}

#[derive(Default)]
pub(crate) struct CodeHitMap {
    pub(crate) first_row: usize,
    pub(crate) top_y: f32,
    pub(crate) text_x: f32,
    pub(crate) lines: Vec<Option<ShapedLine>>,
}

impl CodeHitMap {
    fn row_at(&self, y: f32) -> isize {
        self.first_row as isize + ((y - self.top_y) / CODE_ROW_HEIGHT).floor() as isize
    }

    pub(crate) fn offset_at(&self, doc: &CodeDocument, position: Point<Pixels>) -> usize {
        let last = doc.line_count().saturating_sub(1) as isize;
        let raw = self.row_at(f32::from(position.y));
        let row = raw.clamp(0, last) as usize;
        let range = doc
            .line_byte_range(row)
            .unwrap_or_else(|| doc.len_bytes()..doc.len_bytes());
        let index = raw - self.first_row as isize;
        if index < 0 {
            return range.start;
        }
        let Some(slot) = self.lines.get(index as usize) else {
            return range.end;
        };
        let Some(line) = slot else {
            return range.start;
        };
        let local = line.closest_index_for_x(position.x - px(self.text_x));
        cursor::clamp(doc, range.start + local.min(range.end - range.start))
    }
}

pub(crate) struct CodePrepaint {
    quads: Vec<Quad>,
    glyphs: Vec<CodeGlyph>,
    scrollbars: Vec<RoundedQuad>,
}

pub(crate) struct CodeElement {
    view: Entity<CodeView>,
    palette: RowPalette,
    colors: CodeColors,
    scroll: gpui::ScrollHandle,
    h_offset: f32,
    caret: CodeCaret,
    line_count: usize,
    geometry: Rc<Cell<CodeGeometry>>,
    gutter_memo: Rc<Cell<GutterMemo>>,
    hits: Rc<RefCell<CodeHitMap>>,
    font: Font,
    font_size: Pixels,
    line_height: Pixels,
}

impl CodeElement {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        view: Entity<CodeView>,
        palette: RowPalette,
        colors: CodeColors,
        scroll: gpui::ScrollHandle,
        h_offset: f32,
        caret: CodeCaret,
        line_count: usize,
        geometry: Rc<Cell<CodeGeometry>>,
        gutter_memo: Rc<Cell<GutterMemo>>,
        hits: Rc<RefCell<CodeHitMap>>,
    ) -> Self {
        thread_local! {
            static MONO_FAMILY: SharedString =
                crate::terminal::element::resolve_font_family(None).into();
        }
        let family = MONO_FAMILY.with(|f| f.clone());
        Self {
            view,
            palette,
            colors,
            scroll,
            h_offset,
            caret,
            line_count,
            geometry,
            gutter_memo,
            hits,
            font: Font {
                family,
                features: FontFeatures::disable_ligatures(),
                fallbacks: None,
                weight: FontWeight::NORMAL,
                style: FontStyle::Normal,
            },
            font_size: px(CODE_FONT_SIZE),
            line_height: px(CODE_ROW_HEIGHT),
        }
    }

    fn text_runs(
        &self,
        text: &str,
        syntax: &[(Range<usize>, Hsla)],
        default: Hsla,
    ) -> Vec<TextRun> {
        let run = |len: usize, color: Hsla| TextRun {
            len,
            font: self.font.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        if syntax.is_empty() {
            return vec![run(text.len(), default)];
        }
        let len = text.len();
        let mut runs = Vec::new();
        let mut ix = 0usize;
        for (r, color) in syntax {
            let start = r.start.min(len);
            let end = r.end.min(len);
            if start < ix || start >= end {
                continue;
            }
            if start > ix {
                runs.push(run(start - ix, default));
            }
            runs.push(run(end - start, *color));
            ix = end;
        }
        if ix < len {
            runs.push(run(len - ix, default));
        }
        runs
    }

    fn restyle(
        runs: Vec<TextRun>,
        span: &Range<usize>,
        mut style: impl FnMut(&mut TextRun),
    ) -> Vec<TextRun> {
        if span.start >= span.end {
            return runs;
        }
        let mut out = Vec::with_capacity(runs.len() + 2);
        let mut ix = 0usize;
        for run in runs {
            let end = ix + run.len;
            for (from, to, inside) in [
                (ix, end.min(span.start), false),
                (ix.max(span.start), end.min(span.end), true),
                (ix.max(span.end), end, false),
            ] {
                if to <= from {
                    continue;
                }
                let mut piece = run.clone();
                piece.len = to - from;
                if inside {
                    style(&mut piece);
                }
                out.push(piece);
            }
            ix = end;
        }
        out
    }

    fn shape_plain(&self, window: &mut Window, text: SharedString, color: Hsla) -> ShapedLine {
        let runs = [TextRun {
            len: text.len(),
            font: self.font.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }];
        window
            .text_system()
            .shape_line(text, self.font_size, &runs, None)
    }

    fn resolve_gutter(&self, window: &mut Window, digits: usize) -> GutterMemo {
        let memo = self.gutter_memo.get();
        if memo.digits == digits && memo.digit_w > 0.0 {
            return memo;
        }
        let digit_w = f32::from(
            self.shape_plain(window, "0".into(), self.palette.muted)
                .width(),
        );
        let fresh = GutterMemo {
            digits,
            digit_w,
            gutter_w: gutter_width(digits, digit_w),
        };
        self.gutter_memo.set(fresh);
        fresh
    }
}

impl Element for CodeElement {
    type RequestLayoutState = ();
    type PrepaintState = Option<CodePrepaint>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = Length::Definite(px(self.line_count as f32 * CODE_ROW_HEIGHT).into());
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let view = self.view.read(cx);
        let doc = view.document()?;
        let line_count = doc.line_count();

        let mask = window.content_mask();
        let viewport_h = f32::from(mask.bounds.size.height);
        let content_top = f32::from(mask.bounds.origin.y - bounds.origin.y).max(0.0);
        let rows = visible_rows(content_top, viewport_h, line_count);

        let memo = self.resolve_gutter(window, digit_count(line_count));
        let gutter_w = memo.gutter_w;
        let element_w = f32::from(bounds.size.width);
        let text_viewport_w = text_viewport_width(element_w, gutter_w);
        let h_max = max_h_offset(doc.longest_line_chars(), memo.digit_w, text_viewport_w);
        let h_offset = self.h_offset.clamp(0.0, h_max);
        self.geometry.set(CodeGeometry {
            gutter_w,
            char_w: memo.digit_w,
            text_viewport_w,
            max_h_offset: h_max,
        });

        let visible = rows.len();
        let mut quads = Vec::with_capacity(visible + 2);
        let mut glyphs = Vec::with_capacity(visible * 2);
        let mut scrollbars = Vec::with_capacity(2);

        let left = bounds.origin.x;
        let gutter_px = px(gutter_w);
        let text_x = left + gutter_px + px(CODE_PAD_L);
        let text_clip = Bounds::new(
            point(left + gutter_px, mask.bounds.origin.y),
            size(
                px(element_w - gutter_w).max(px(0.)),
                mask.bounds.size.height,
            ),
        );

        if visible > 0 {
            let top = bounds.origin.y + px(rows.start as f32 * CODE_ROW_HEIGHT);
            let span_h = px(visible as f32 * CODE_ROW_HEIGHT);
            quads.push(Quad {
                bounds: Bounds::new(point(left, top), size(bounds.size.width, span_h)),
                color: self.palette.context_bg,
                clip: None,
            });
            quads.push(Quad {
                bounds: Bounds::new(point(left, top), size(gutter_px, span_h)),
                color: self.palette.gutter_bg,
                clip: None,
            });
        }

        let cursor = self.caret.cursor.min(doc.len_bytes());
        let cursor_row = doc.byte_to_line(cursor);
        let sel = self.caret.selection.clone();
        let marked = self.caret.marked.clone();
        if sel.start >= sel.end && rows.contains(&cursor_row) {
            let y = bounds.origin.y + px(cursor_row as f32 * CODE_ROW_HEIGHT);
            quads.push(Quad {
                bounds: Bounds::new(point(left, y), size(bounds.size.width, px(CODE_ROW_HEIGHT))),
                color: cursor_line_wash(self.palette.cursor_line_bg, self.caret.focused),
                clip: None,
            });
        }

        let mut hits = CodeHitMap {
            first_row: rows.start,
            top_y: f32::from(bounds.origin.y) + rows.start as f32 * CODE_ROW_HEIGHT,
            text_x: f32::from(text_x) - h_offset,
            lines: Vec::with_capacity(visible),
        };

        let hl = view.highlighter();
        for row in rows.clone() {
            let y = bounds.origin.y + px(row as f32 * CODE_ROW_HEIGHT);

            let number: SharedString = (row + 1).to_string().into();
            let num_color = if row == cursor_row {
                self.palette.text
            } else {
                self.palette.muted
            };
            let num_line = self.shape_plain(window, number, num_color);
            let num_x = (left + gutter_px - px(NUM_GAP) - num_line.width()).max(left);
            glyphs.push(CodeGlyph {
                origin: point(num_x, y),
                line: num_line,
                clip: None,
            });

            let range = doc
                .line_byte_range(row)
                .unwrap_or_else(|| doc.len_bytes()..doc.len_bytes());
            let row_sel = row_selection(&sel, &range);
            let origin = point(text_x - px(h_offset), y);

            let text = doc.line_string(row).unwrap_or_default();
            let line = if text.is_empty() {
                None
            } else {
                let text: SharedString = text.into();
                let syntax = hl.map(|hl| hl.runs(row)).unwrap_or_default();
                let runs = self.text_runs(&text, &syntax, self.palette.text);
                let runs = match &row_sel {
                    Some((local, _)) => {
                        let fg = self.colors.selection_fg;
                        Self::restyle(runs, local, |run| run.color = fg)
                    }
                    None => runs,
                };
                let runs = match row_selection(&marked, &range) {
                    Some((local, _)) => {
                        let underline = UnderlineStyle {
                            color: Some(self.colors.cursor),
                            thickness: px(1.0),
                            wavy: false,
                        };
                        Self::restyle(runs, &local, |run| run.underline = Some(underline))
                    }
                    None => runs,
                };
                Some(
                    window
                        .text_system()
                        .shape_line(text, self.font_size, &runs, None),
                )
            };

            if let Some((local, wraps)) = row_sel {
                let x0 = line
                    .as_ref()
                    .map(|l| l.x_for_index(local.start))
                    .unwrap_or(px(0.));
                let mut x1 = line
                    .as_ref()
                    .map(|l| l.x_for_index(local.end))
                    .unwrap_or(px(0.));
                if wraps {
                    x1 += px(memo.digit_w.max(1.0));
                }
                if x1 > x0 {
                    quads.push(Quad {
                        bounds: Bounds::new(
                            point(origin.x + x0, y),
                            size(x1 - x0, px(CODE_ROW_HEIGHT)),
                        ),
                        color: self.colors.selection,
                        clip: Some(text_clip),
                    });
                }
            }

            if row == cursor_row && self.caret.focused && self.caret.visible {
                let local = cursor.saturating_sub(range.start);
                let x = line
                    .as_ref()
                    .map(|l| l.x_for_index(local))
                    .unwrap_or(px(0.));
                quads.push(Quad {
                    bounds: Bounds::new(
                        point(origin.x + x, y),
                        size(px(CARET_WIDTH), px(CODE_ROW_HEIGHT)),
                    ),
                    color: self.colors.cursor,
                    clip: Some(text_clip),
                });
            }

            if let Some(line) = line {
                glyphs.push(CodeGlyph {
                    origin,
                    line: line.clone(),
                    clip: Some(text_clip),
                });
                hits.lines.push(Some(line));
            } else {
                hits.lines.push(None);
            }
        }
        *self.hits.borrow_mut() = hits;

        let corners = Corners::all(px(3.));
        if let Some(m) = crate::widgets::scrollbar::metrics(&self.scroll) {
            let x = mask.bounds.right() - px(V_SCROLLBAR_INSET + V_SCROLLBAR_W);
            scrollbars.push(RoundedQuad {
                bounds: Bounds::new(
                    point(x, mask.bounds.origin.y + px(m.thumb_top)),
                    size(px(V_SCROLLBAR_W), px(m.thumb_h)),
                ),
                corners,
                color: self.colors.scrollbar_thumb,
            });
        }

        if let Some((thumb_x, thumb_w)) = h_thumb(h_offset, h_max, text_viewport_w) {
            let y = mask.bounds.bottom() - px(H_SCROLLBAR_INSET + H_SCROLLBAR_H);
            scrollbars.push(RoundedQuad {
                bounds: Bounds::new(
                    point(text_x + px(thumb_x), y),
                    size(px(thumb_w), px(H_SCROLLBAR_H)),
                ),
                corners,
                color: self.colors.scrollbar_thumb,
            });
        }

        Some(CodePrepaint {
            quads,
            glyphs,
            scrollbars,
        })
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(layout) = prepaint.take() else {
            return;
        };
        let focus = self.view.read(cx).focus_handle(cx);
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );
        let lh = self.line_height;
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for q in &layout.quads {
                match q.clip {
                    Some(clip) => {
                        window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                            window.paint_quad(fill(q.bounds, q.color));
                        });
                    }
                    None => window.paint_quad(fill(q.bounds, q.color)),
                }
            }
            for g in layout.glyphs {
                if let Some(clip) = g.clip {
                    window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                        let _ = g
                            .line
                            .paint(g.origin, lh, TextAlign::Left, None, window, cx);
                    });
                } else {
                    let _ = g
                        .line
                        .paint(g.origin, lh, TextAlign::Left, None, window, cx);
                }
            }
        });
        if !layout.scrollbars.is_empty() {
            let viewport = window.content_mask().bounds;
            window.paint_layer(viewport, |window| {
                for q in &layout.scrollbars {
                    window.paint_quad(quad(
                        q.bounds,
                        q.corners,
                        q.color,
                        px(0.),
                        q.color,
                        BorderStyle::Solid,
                    ));
                }
            });
        }
    }
}

impl IntoElement for CodeElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn bound_for(viewport_h: f32) -> usize {
        (viewport_h / CODE_ROW_HEIGHT) as usize + 1 + 2 * OVERDRAW_ROWS
    }

    #[test]
    fn visible_rows_are_bounded_by_the_viewport_not_the_file() {
        let line_count = 100_000;
        let viewport_h = 720.0;
        let bound = bound_for(viewport_h);
        let content_h = line_count as f32 * CODE_ROW_HEIGHT;

        let mut top = 0.0;
        while top <= content_h {
            let rows = visible_rows(top, viewport_h, line_count);
            assert!(
                rows.len() <= bound,
                "top {top}: {} rows shaped, bound is {bound}",
                rows.len()
            );
            top += CODE_ROW_HEIGHT * 137.0 + 0.5;
        }
        assert_eq!(
            visible_rows(0.0, viewport_h, line_count).len(),
            visible_rows(0.0, viewport_h, 1_000_000).len()
        );
    }

    #[test]
    fn visible_rows_cover_every_row_touching_the_viewport() {
        let line_count = 5_000;
        let viewport_h = 400.0;
        for step in 0..200 {
            let top = step as f32 * 13.7;
            let rows = visible_rows(top, viewport_h, line_count);
            let first_touched = (top / CODE_ROW_HEIGHT) as usize;
            let last_touched = (((top + viewport_h - 0.001) / CODE_ROW_HEIGHT) as usize)
                .min(line_count.saturating_sub(1));
            assert!(rows.start <= first_touched, "top {top}: {rows:?}");
            assert!(rows.end > last_touched, "top {top}: {rows:?}");
        }
    }

    #[test]
    fn visible_rows_clamps_at_both_ends() {
        assert_eq!(visible_rows(0.0, 100.0, 0), 0..0);
        assert_eq!(visible_rows(0.0, 0.0, 500), 0..0);
        let end = visible_rows(10_000.0, 200.0, 12);
        assert_eq!(end, 12..12);
        let start = visible_rows(0.0, 200.0, 500);
        assert_eq!(start.start, 0);
    }

    #[test]
    fn crossing_a_power_of_ten_widens_the_gutter_without_moving_rows() {
        assert_eq!(digit_count(999), 3);
        assert_eq!(digit_count(1_000), 4);
        let digit_w = 7.5;
        let narrow = gutter_width(digit_count(999), digit_w);
        let wide = gutter_width(digit_count(1_000), digit_w);
        assert!(wide > narrow, "{wide} should exceed {narrow}");
        assert_eq!(wide - narrow, digit_w);
        for row in [0usize, 998, 999] {
            assert_eq!(
                row as f32 * CODE_ROW_HEIGHT,
                row as f32 * CODE_ROW_HEIGHT,
                "row {row} must not depend on the gutter width"
            );
        }
    }

    #[test]
    fn digit_count_is_exact_at_powers_of_ten() {
        assert_eq!(digit_count(0), 1);
        assert_eq!(digit_count(1), 1);
        assert_eq!(digit_count(9), 1);
        assert_eq!(digit_count(10), 2);
        assert_eq!(digit_count(99_999), 5);
        assert_eq!(digit_count(100_000), 6);
    }

    #[test]
    fn gutter_never_goes_below_its_floor() {
        assert_eq!(gutter_width(1, 1.0), GUTTER_MIN_W);
        assert!(gutter_width(6, 7.5) > GUTTER_MIN_W);
    }

    #[test]
    fn horizontal_extent_derives_from_the_longest_line() {
        let char_w = 7.5;
        assert_eq!(max_h_offset(40, char_w, 600.0), 0.0);
        assert_eq!(max_h_offset(200, char_w, 600.0), 1500.0 + 12.0 - 600.0);
        let a = max_h_offset(200, char_w, 600.0);
        let b = max_h_offset(201, char_w, 600.0);
        assert!(b > a);
    }

    #[test]
    fn text_viewport_excludes_the_gutter() {
        let w = text_viewport_width(800.0, 60.0);
        assert_eq!(w, 800.0 - 60.0 - CODE_PAD_L - CODE_PAD_R);
        assert_eq!(text_viewport_width(20.0, 60.0), 0.0);
    }

    #[test]
    fn horizontal_thumb_only_exists_on_overflow() {
        assert!(h_thumb(0.0, 0.0, 600.0).is_none());
        assert!(h_thumb(0.0, 0.2, 600.0).is_none());
        let (x, w) = h_thumb(0.0, 600.0, 600.0).expect("overflowing line has a thumb");
        assert_eq!(x, 0.0);
        assert!((H_SCROLLBAR_MIN_THUMB..600.0).contains(&w));
        let (x_end, w_end) = h_thumb(600.0, 600.0, 600.0).expect("thumb");
        assert!((x_end + w_end - 600.0).abs() < 0.001);
    }

    #[test]
    fn reveal_keeps_the_cursor_visible_with_a_two_line_margin() {
        let viewport_h = 360.0;
        let content_h = 1_000.0 * CODE_ROW_HEIGHT;
        let margin = REVEAL_MARGIN_ROWS * CODE_ROW_HEIGHT;

        assert_eq!(reveal_offset(10, viewport_h, content_h, 0.0), 0.0);

        let off = reveal_offset(40, viewport_h, content_h, 0.0);
        let top = -off;
        let row_bottom = 41.0 * CODE_ROW_HEIGHT;
        assert!(row_bottom + margin <= top + viewport_h + 0.001);
        assert!(row_bottom <= top + viewport_h);

        let off = reveal_offset(5, viewport_h, content_h, -600.0);
        let top = -off;
        assert!(5.0 * CODE_ROW_HEIGHT - margin >= top - 0.001);
        assert!(5.0 * CODE_ROW_HEIGHT >= top);
    }

    #[test]
    fn reveal_is_clamped_to_the_document() {
        let viewport_h = 360.0;
        let line_count = 1_000.0;
        let content_h = line_count * CODE_ROW_HEIGHT;
        let max_off = content_h - viewport_h;

        let first = reveal_offset(0, viewport_h, content_h, -500.0);
        assert_eq!(first, 0.0);

        let last = reveal_offset(999, viewport_h, content_h, 0.0);
        assert!(last >= -max_off, "{last} overscrolled past {max_off}");
        assert!(last <= 0.0);

        assert_eq!(reveal_offset(3, viewport_h, 90.0, 0.0), 0.0);
    }

    #[test]
    fn reveal_margin_degrades_on_a_tiny_viewport() {
        let viewport_h = 2.0 * CODE_ROW_HEIGHT;
        let content_h = 100.0 * CODE_ROW_HEIGHT;
        let off = reveal_offset(50, viewport_h, content_h, 0.0);
        let top = -off;
        assert!(50.0 * CODE_ROW_HEIGHT >= top - 0.001);
        assert!(51.0 * CODE_ROW_HEIGHT <= top + viewport_h + 0.001);
    }

    #[test]
    fn a_row_paints_only_its_own_slice_of_the_selection() {
        let row = 4..7;

        assert_eq!(
            row_selection(&(0..0), &row),
            None,
            "an empty selection paints nothing"
        );
        assert_eq!(
            row_selection(&(0..4), &row),
            None,
            "a selection ending at the row start"
        );
        assert_eq!(
            row_selection(&(8..11), &row),
            None,
            "a selection after the row"
        );

        assert_eq!(row_selection(&(5..6), &row), Some((1..2, false)));
        assert_eq!(
            row_selection(&(0..6), &row),
            Some((0..2, false)),
            "clipped on the left"
        );
        assert_eq!(
            row_selection(&(5..11), &row),
            Some((1..3, true)),
            "crossing the row end selects the terminator too"
        );
        assert_eq!(
            row_selection(&(0..11), &row),
            Some((0..3, true)),
            "a row fully inside the selection"
        );
    }

    #[test]
    fn an_empty_row_inside_a_selection_still_paints_its_terminator() {
        let row = 4..4;
        assert_eq!(row_selection(&(0..9), &row), Some((0..0, true)));
        assert_eq!(row_selection(&(4..4), &row), None);
    }

    #[test]
    fn the_horizontal_reveal_only_moves_when_the_caret_leaves_the_column() {
        let viewport_w = 400.0;
        let max = 1_000.0;

        assert_eq!(
            reveal_h_offset(200.0, viewport_w, max, 0.0),
            0.0,
            "already visible, no nudge"
        );
        let right = reveal_h_offset(500.0, viewport_w, max, 0.0);
        assert!(right > 0.0);
        assert!(500.0 - right <= viewport_w);
        assert_eq!(
            reveal_h_offset(100.0, viewport_w, max, 300.0),
            100.0 - H_SCROLL_MARGIN
        );
        assert_eq!(reveal_h_offset(0.0, viewport_w, max, 0.0), 0.0);
        assert_eq!(reveal_h_offset(10_000.0, viewport_w, max, 0.0), max);
        assert_eq!(reveal_h_offset(500.0, 0.0, max, 42.0), 42.0);
    }

    #[test]
    fn an_out_of_viewport_drag_steps_toward_the_pointer() {
        assert_eq!(autoscroll_step(50.0, 10.0, 90.0, 4.0), 0.0, "inside");
        assert_eq!(autoscroll_step(10.0, 10.0, 90.0, 4.0), 0.0, "on the edge");
        assert_eq!(autoscroll_step(90.0, 10.0, 90.0, 4.0), 0.0, "on the edge");
        assert_eq!(autoscroll_step(9.0, 10.0, 90.0, 4.0), -4.0, "past the top");
        assert_eq!(
            autoscroll_step(91.0, 10.0, 90.0, 4.0),
            4.0,
            "past the bottom"
        );
        assert_eq!(autoscroll_step(9_000.0, 10.0, 90.0, 4.0), 4.0);
        assert_eq!(autoscroll_step(9_000.0, 10.0, 90.0, 0.0), 0.0);
    }

    #[test]
    fn the_current_line_wash_is_derived_from_the_theme_foreground() {
        for (name, build) in crate::theme::THEMES {
            let ui = crate::theme::ui_colors_with(&build());
            let p = crate::diff::palette(ui);
            assert_eq!(p.cursor_line_bg.h, ui.text.h);
            assert_eq!(p.cursor_line_bg.s, ui.text.s);
            assert_eq!(p.cursor_line_bg.l, ui.text.l);
            assert!(
                p.cursor_line_bg.a > 0.0 && p.cursor_line_bg.a < 0.1,
                "{name}: a wash at {} would fight the diff colors",
                p.cursor_line_bg.a
            );
        }
    }

    #[test]
    fn the_current_line_wash_dims_when_focus_leaves() {
        let base = crate::theme::ui_colors_with(&crate::theme::paneflow_dark())
            .text
            .opacity(0.05);
        assert_eq!(cursor_line_wash(base, true), base);
        let dim = cursor_line_wash(base, false);
        assert!(dim.a < base.a, "unfocused must be fainter");
        assert!(dim.a > 0.0, "but still visible, not dropped");
        assert_eq!(cursor_line_wash(dim, true), dim, "focused is the identity");
    }

    #[test]
    fn a_click_outside_the_shaped_window_still_lands_on_a_legal_slot() {
        let doc = CodeDocument::new(PathBuf::from("/nonexistent/a.txt"), "one\ntwo\nthree");
        let map = CodeHitMap {
            first_row: 1,
            top_y: 100.0,
            text_x: 40.0,
            lines: vec![None],
        };

        let below = map.offset_at(&doc, point(px(500.), px(10_000.)));
        assert_eq!(below, doc.len_bytes());
        assert_eq!(map.offset_at(&doc, point(px(0.), px(-10_000.))), 0);
        assert_eq!(map.offset_at(&doc, point(px(0.), px(105.))), 4);
    }
}
