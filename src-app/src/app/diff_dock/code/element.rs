use std::cell::{Cell, RefCell};
use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, BorderStyle, Bounds, ContentMask, Corners, CursorStyle, Element, ElementId,
    ElementInputHandler, Entity, Focusable, Font, FontFeatures, FontStyle, FontWeight,
    GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId,
    Pixels, Point, ShapedLine, SharedString, Style, TextAlign, TextRun, UnderlineStyle, Window,
    fill, point, px, quad, relative, size,
};
use paneflow_textdiff::BlockKind;
use ropey::RopeSlice;

use super::cursor;
use super::document::CodeDocument;
use super::highlight::LineRuns;
use super::markers::{
    MARKER_BAR_RADIUS, MARKER_BAR_W, MARKER_COLUMN_W, MARKER_HOVER_GROW, marker_rects,
};
use super::minimap::MinimapPaint;
use super::navigation::{
    MINIMAP_FONT_SIZE, NavigationLayout, SCROLLBAR_SIZE, minimap_top, minimap_track, minimap_width,
    scrollbar_track,
};
use super::view::CodeView;
use crate::diff::{ROW_HEIGHT, RowPalette};
use crate::widgets::scrollbar::ScrollableHandle;

pub(crate) const CODE_ROW_HEIGHT: f32 = ROW_HEIGHT;
pub(crate) const CODE_FONT_SIZE: f32 = 12.0;
const NUM_GAP: f32 = 6.0;
const GUTTER_PAD_L: f32 = 8.0;
const GUTTER_MIN_W: f32 = 36.0;
const MARKER_INSET_L: f32 = 2.0;
const CODE_PAD_L: f32 = 6.0;
const CODE_PAD_R: f32 = 8.0;
const H_SCROLL_MARGIN: f32 = 12.0;
const CARET_WIDTH: f32 = 2.0;

const SCROLL_EPSILON: f32 = 0.5;
const REVEAL_MARGIN_ROWS: f32 = 2.0;

const OVERDRAW_ROWS: usize = 1;

pub(crate) fn visible_rows_at(
    scroll_rows: f64,
    viewport_h: f32,
    line_count: usize,
) -> Range<usize> {
    if line_count == 0 || viewport_h <= 0.0 {
        return 0..0;
    }
    let top = scroll_rows.max(0.0);
    let first = (top as usize).saturating_sub(OVERDRAW_ROWS);
    let bottom = top + f64::from(viewport_h) / f64::from(CODE_ROW_HEIGHT);
    let last = bottom as usize + 1 + OVERDRAW_ROWS;
    first.min(line_count)..last.min(line_count)
}

pub(crate) fn device_round(y: f64, scale_factor: f32) -> f32 {
    if scale_factor.is_nan() || scale_factor <= 0.0 {
        return y as f32;
    }
    let scale = f64::from(scale_factor);
    ((y * scale).round() / scale) as f32
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
    (GUTTER_PAD_L + MARKER_COLUMN_W + digits as f32 * digit_w + NUM_GAP)
        .max(GUTTER_MIN_W + MARKER_COLUMN_W)
}

pub(crate) fn code_font() -> Font {
    thread_local! {
        static MONO_FAMILY: SharedString =
            crate::terminal::element::resolve_font_family(None).into();
    }
    Font {
        family: MONO_FAMILY.with(|f| f.clone()),
        features: FontFeatures::disable_ligatures(),
        fallbacks: None,
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    }
}

pub(crate) fn syntax_text_runs(
    text: &str,
    syntax: &[(Range<usize>, Hsla)],
    font: &Font,
    default: Hsla,
) -> Vec<TextRun> {
    let run = |len: usize, color: Hsla| TextRun {
        len,
        font: font.clone(),
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

pub(crate) fn text_viewport_width(element_w: f32, gutter_w: f32) -> f32 {
    (element_w - gutter_w - CODE_PAD_L - CODE_PAD_R).max(0.0)
}

pub(crate) fn max_h_offset(longest_line_chars: usize, char_w: f32, text_viewport_w: f32) -> f32 {
    let content_w = longest_line_chars as f32 * char_w + H_SCROLL_MARGIN;
    (content_w - text_viewport_w).max(0.0)
}

pub(crate) fn reveal_rows(row: usize, viewport_h: f32, max_rows: f64, current: f64) -> f64 {
    if viewport_h <= 0.0 {
        return current;
    }
    let visible = f64::from(viewport_h) / f64::from(CODE_ROW_HEIGHT);
    let margin = f64::from(REVEAL_MARGIN_ROWS)
        .min((visible - 1.0) / 2.0)
        .max(0.0);
    let row_top = row as f64;
    let row_bottom = row_top + 1.0;
    let mut top = current;
    if row_top - margin < top {
        top = row_top - margin;
    } else if row_bottom + margin > top + visible {
        top = row_bottom + margin - visible;
    }
    top.clamp(0.0, max_rows)
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

pub(crate) fn line_content_hash(line: RopeSlice<'_>) -> u64 {
    let mut hasher = DefaultHasher::new();
    for chunk in line.chunks() {
        hasher.write(chunk.as_bytes());
    }
    hasher.finish()
}

pub(crate) fn line_number_hash(number: usize, color: Hsla) -> u64 {
    let mut hasher = DefaultHasher::new();
    hasher.write_usize(number);
    for channel in [color.h, color.s, color.l, color.a] {
        hasher.write_u32(channel.to_bits());
    }
    hasher.finish()
}

#[derive(Default)]
struct CodeScrollState {
    rows: Cell<f64>,
    viewport: Cell<Bounds<Pixels>>,
    line_count: Cell<usize>,
}

#[derive(Clone, Default)]
pub(crate) struct CodeScroll(Rc<CodeScrollState>);

impl CodeScroll {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn rows(&self) -> f64 {
        self.0.rows.get()
    }

    pub(crate) fn bounds(&self) -> Bounds<Pixels> {
        self.0.viewport.get()
    }

    pub(crate) fn viewport_height(&self) -> f32 {
        f32::from(self.0.viewport.get().size.height)
    }

    pub(crate) fn visible_rows(&self) -> f64 {
        let viewport_h = self.viewport_height();
        if viewport_h <= 0.0 {
            return 0.0;
        }
        f64::from(viewport_h) / f64::from(CODE_ROW_HEIGHT)
    }

    pub(crate) fn max_rows(&self) -> f64 {
        if self.viewport_height() <= 0.0 {
            return 0.0;
        }
        (self.0.line_count.get() as f64 - self.visible_rows()).max(0.0)
    }

    pub(crate) fn content_top(&self) -> f32 {
        (self.rows() * f64::from(CODE_ROW_HEIGHT)) as f32
    }

    pub(crate) fn set_rows(&self, rows: f64) -> bool {
        if self.viewport_height() <= 0.0 {
            return false;
        }
        let next = if rows.is_finite() { rows } else { 0.0 };
        let next = next.clamp(0.0, self.max_rows());
        if next == self.0.rows.get() {
            return false;
        }
        self.0.rows.set(next);
        true
    }

    pub(crate) fn reset_rows(&self) {
        self.0.rows.set(0.0);
    }

    pub(crate) fn scroll_by_pixels(&self, dy: f32) -> bool {
        self.set_rows(self.rows() + f64::from(dy) / f64::from(CODE_ROW_HEIGHT))
    }

    pub(crate) fn set_line_count(&self, line_count: usize) {
        self.0.line_count.set(line_count);
        self.set_rows(self.rows());
    }

    pub(crate) fn set_metrics(&self, viewport: Bounds<Pixels>, line_count: usize) {
        self.0.viewport.set(viewport);
        self.set_line_count(line_count);
    }
}

impl ScrollableHandle for CodeScroll {
    fn viewport(&self) -> Bounds<Pixels> {
        self.bounds()
    }

    fn max_offset(&self) -> Point<Pixels> {
        point(
            px(0.),
            px((self.max_rows() * f64::from(CODE_ROW_HEIGHT)) as f32),
        )
    }

    fn offset(&self) -> Point<Pixels> {
        point(px(0.), px(-self.content_top()))
    }

    fn set_offset(&self, offset: Point<Pixels>) {
        self.set_rows(f64::from(-f32::from(offset.y)) / f64::from(CODE_ROW_HEIGHT));
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
    pub(crate) marker_added: Hsla,
    pub(crate) marker_modified: Hsla,
    pub(crate) marker_deleted: Hsla,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MarkerHit {
    pub(crate) index: usize,
    pub(crate) y0: f32,
    pub(crate) y1: f32,
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
    pub(crate) marker_x: f32,
    pub(crate) markers: Vec<MarkerHit>,
    pub(crate) materialized_lines: usize,
    pub(crate) materialized_numbers: usize,
    pub(crate) lines: Vec<Option<ShapedLine>>,
}

impl CodeHitMap {
    fn row_at(&self, y: f32) -> isize {
        self.first_row as isize + ((y - self.top_y) / CODE_ROW_HEIGHT).floor() as isize
    }

    pub(crate) fn row_top(&self, row: usize) -> f32 {
        self.top_y + (row as f32 - self.first_row as f32) * CODE_ROW_HEIGHT
    }

    pub(crate) fn marker_at(&self, position: Point<Pixels>) -> Option<usize> {
        let x = f32::from(position.x);
        let y = f32::from(position.y);
        if self.markers.is_empty()
            || x < self.marker_x - MARKER_HOVER_GROW
            || x > self.marker_x + MARKER_COLUMN_W
        {
            return None;
        }
        self.markers
            .iter()
            .find(|hit| y >= hit.y0 && y < hit.y1)
            .map(|hit| hit.index)
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
    content_bounds: Bounds<Pixels>,
    quads: Vec<Quad>,
    markers: Vec<RoundedQuad>,
    glyphs: Vec<CodeGlyph>,
    navigation: NavigationLayout,
    navigation_hitbox: Hitbox,
    minimap: Option<MinimapPaint>,
    marker_hitbox: Hitbox,
    marker_pointer: bool,
}

const RUN_SCRATCH_CAPACITY: usize = 64;

pub(crate) struct CodeElement {
    view: Entity<CodeView>,
    palette: RowPalette,
    colors: CodeColors,
    scroll: CodeScroll,
    h_offset: f32,
    caret: CodeCaret,
    geometry: Rc<Cell<CodeGeometry>>,
    gutter_memo: Rc<Cell<GutterMemo>>,
    hits: Rc<RefCell<CodeHitMap>>,
    font: Font,
    font_size: Pixels,
    line_height: Pixels,
    runs: Vec<TextRun>,
    restyled: Vec<TextRun>,
    syntax: LineRuns,
}

impl CodeElement {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        view: Entity<CodeView>,
        palette: RowPalette,
        colors: CodeColors,
        scroll: CodeScroll,
        h_offset: f32,
        caret: CodeCaret,
        geometry: Rc<Cell<CodeGeometry>>,
        gutter_memo: Rc<Cell<GutterMemo>>,
        hits: Rc<RefCell<CodeHitMap>>,
    ) -> Self {
        Self {
            view,
            palette,
            colors,
            scroll,
            h_offset,
            caret,
            geometry,
            gutter_memo,
            hits,
            font: code_font(),
            font_size: px(CODE_FONT_SIZE),
            line_height: px(CODE_ROW_HEIGHT),
            runs: Vec::with_capacity(RUN_SCRATCH_CAPACITY),
            restyled: Vec::with_capacity(RUN_SCRATCH_CAPACITY),
            syntax: LineRuns::with_capacity(RUN_SCRATCH_CAPACITY),
        }
    }

    fn fill_text_runs(
        font: &Font,
        out: &mut Vec<TextRun>,
        len: usize,
        syntax: &[(Range<usize>, Hsla)],
        default: Hsla,
    ) {
        out.clear();
        let run = |len: usize, color: Hsla| TextRun {
            len,
            font: font.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        if syntax.is_empty() {
            out.push(run(len, default));
            return;
        }
        let mut ix = 0usize;
        for (r, color) in syntax {
            let start = r.start.min(len);
            let end = r.end.min(len);
            if start < ix || start >= end {
                continue;
            }
            if start > ix {
                out.push(run(start - ix, default));
            }
            out.push(run(end - start, *color));
            ix = end;
        }
        if ix < len {
            out.push(run(len - ix, default));
        }
    }

    fn restyle_in_place(
        runs: &mut Vec<TextRun>,
        scratch: &mut Vec<TextRun>,
        span: &Range<usize>,
        mut style: impl FnMut(&mut TextRun),
    ) {
        if span.start >= span.end {
            return;
        }
        scratch.clear();
        let mut ix = 0usize;
        for run in runs.iter() {
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
                scratch.push(piece);
            }
            ix = end;
        }
        std::mem::swap(runs, scratch);
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
        style.size.height = relative(1.).into();
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

        let memo = self.resolve_gutter(window, digit_count(line_count));
        let gutter_w = memo.gutter_w;
        let display = view.controls.read(cx).display;
        let scrollbar_w = if display.scrollbar {
            SCROLLBAR_SIZE
        } else {
            0.0
        };
        let minimap_w = minimap_width(
            text_viewport_width(f32::from(bounds.size.width) - scrollbar_w, gutter_w),
            memo.digit_w * MINIMAP_FONT_SIZE / CODE_FONT_SIZE,
            display.minimap,
        );
        let element_w = (f32::from(bounds.size.width) - scrollbar_w - minimap_w).max(0.0);
        let text_viewport_w = text_viewport_width(element_w, gutter_w);
        let h_max = max_h_offset(doc.longest_line_chars(), memo.digit_w, text_viewport_w);
        let h_offset = self.h_offset.clamp(0.0, h_max);
        let scrollbar_h = if display.scrollbar && h_max > SCROLL_EPSILON {
            SCROLLBAR_SIZE
        } else {
            0.0
        };
        let viewport_h = (f32::from(bounds.size.height) - scrollbar_h).max(0.0);
        self.scroll.set_metrics(
            Bounds::new(bounds.origin, size(bounds.size.width, px(viewport_h))),
            line_count,
        );
        let scroll_rows = self.scroll.rows();
        let scale_factor = window.scale_factor();
        let rows = visible_rows_at(scroll_rows, viewport_h, line_count);
        let vertical = display.scrollbar.then(|| {
            scrollbar_track(
                Bounds::new(
                    point(bounds.right() - px(SCROLLBAR_SIZE), bounds.origin.y),
                    size(px(SCROLLBAR_SIZE), px(viewport_h)),
                ),
                self.scroll.visible_rows(),
                line_count as f64,
                scroll_rows,
                false,
            )
        });
        let horizontal = (scrollbar_h > 0.0).then(|| {
            scrollbar_track(
                Bounds::new(
                    point(
                        bounds.origin.x + px(gutter_w + CODE_PAD_L),
                        bounds.bottom() - px(SCROLLBAR_SIZE),
                    ),
                    size(px(text_viewport_w), px(SCROLLBAR_SIZE)),
                ),
                f64::from(text_viewport_w),
                f64::from(text_viewport_w + h_max),
                f64::from(h_offset),
                true,
            )
        });
        let minimap_track = (minimap_w > 0.0).then(|| {
            minimap_track(
                Bounds::new(
                    point(bounds.origin.x + px(element_w), bounds.origin.y),
                    size(px(minimap_w), px(viewport_h)),
                ),
                line_count,
                &self.scroll,
            )
        });
        let navigation = NavigationLayout {
            vertical,
            horizontal,
            minimap: minimap_track,
            minimap_top: minimap_top(line_count, &self.scroll),
        };
        if minimap_track.is_some() && view.navigation.layout.get().minimap.is_none() {
            window.request_animation_frame();
        }
        view.navigation.layout.set(navigation);
        let navigation_hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let minimap = minimap_track.and_then(|track| {
            MinimapPaint::layout(
                view,
                track,
                &self.scroll,
                self.palette.context_bg,
                self.palette.text,
                window,
            )
        });
        let bounds = Bounds::new(bounds.origin, size(px(element_w), px(viewport_h)));
        self.geometry.set(CodeGeometry {
            gutter_w,
            char_w: memo.digit_w,
            text_viewport_w,
            max_h_offset: h_max,
        });

        let origin_y = f64::from(f32::from(bounds.origin.y));
        let row_y = |row: usize| -> Pixels {
            px(device_round(
                origin_y + (row as f64 - scroll_rows) * f64::from(CODE_ROW_HEIGHT),
                scale_factor,
            ))
        };

        let visible = rows.len();
        let mut quads = Vec::with_capacity(visible + 2);
        let mut glyphs = Vec::with_capacity(visible * 2);

        let left = bounds.origin.x;
        let gutter_px = px(gutter_w);
        let text_x = left + gutter_px + px(CODE_PAD_L);
        let text_clip = Bounds::new(
            point(left + gutter_px, bounds.origin.y),
            size(px(element_w - gutter_w).max(px(0.)), bounds.size.height),
        );

        if visible > 0 {
            let top = row_y(rows.start);
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
            let y = row_y(cursor_row);
            quads.push(Quad {
                bounds: Bounds::new(point(left, y), size(bounds.size.width, px(CODE_ROW_HEIGHT))),
                color: cursor_line_wash(self.palette.cursor_line_bg, self.caret.focused),
                clip: None,
            });
        }

        let column_x = left + px(MARKER_INSET_L);
        let mut hits = CodeHitMap {
            first_row: rows.start,
            top_y: f32::from(row_y(rows.start)),
            text_x: f32::from(text_x) - h_offset,
            marker_x: f32::from(column_x),
            markers: Vec::new(),
            materialized_lines: 0,
            materialized_numbers: 0,
            lines: Vec::with_capacity(visible),
        };

        let mut markers = Vec::new();
        let blocks = view.marker_blocks();
        let hovered = view.hovered_marker();
        if visible > 0 && !blocks.is_empty() {
            let window_top = hits.top_y;
            for rect in marker_rects(
                blocks,
                rows.start,
                CODE_ROW_HEIGHT,
                visible as f32 * CODE_ROW_HEIGHT,
            ) {
                let hover = hovered == Some(rect.index);
                let (color, x, w) = match rect.kind {
                    BlockKind::Added => {
                        (self.colors.marker_added, column_x + px(1.0), MARKER_BAR_W)
                    }
                    BlockKind::Modified => (
                        self.colors.marker_modified,
                        column_x + px(1.0),
                        MARKER_BAR_W,
                    ),
                    BlockKind::Deleted => (self.colors.marker_deleted, column_x, MARKER_COLUMN_W),
                };
                let (x, w) = if hover {
                    (x - px(MARKER_HOVER_GROW), w + MARKER_HOVER_GROW)
                } else {
                    (x, w)
                };
                markers.push(RoundedQuad {
                    bounds: Bounds::new(point(x, px(window_top + rect.y)), size(px(w), px(rect.h))),
                    corners: Corners::all(px(MARKER_BAR_RADIUS)),
                    color,
                });
                hits.markers.push(MarkerHit {
                    index: rect.index,
                    y0: window_top + rect.hit_y,
                    y1: window_top + rect.hit_y + rect.hit_h,
                });
            }
        }
        let marker_hitbox = window.insert_hitbox(
            Bounds::new(
                point(left, bounds.origin.y),
                size(px(MARKER_INSET_L + MARKER_COLUMN_W), bounds.size.height),
            ),
            HitboxBehavior::Normal,
        );
        let marker_pointer = hovered.is_some();

        let hl = view.highlighter();
        for row in rows.clone() {
            let y = row_y(row);

            let num_color = if row == cursor_row {
                self.palette.text
            } else {
                self.palette.muted
            };
            let number = row + 1;
            let digits = digit_count(number);
            self.runs.clear();
            self.runs.push(TextRun {
                len: digits,
                font: self.font.clone(),
                color: num_color,
                background_color: None,
                underline: None,
                strikethrough: None,
            });
            let mut number_materialized = false;
            let num_line = window.text_system().shape_line_by_hash(
                line_number_hash(number, num_color),
                digits,
                self.font_size,
                &self.runs,
                None,
                || {
                    number_materialized = true;
                    number.to_string().into()
                },
            );
            if number_materialized {
                hits.materialized_numbers += 1;
            }
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

            let len = range.end - range.start;
            let line = match doc.line(row).filter(|_| len > 0) {
                None => None,
                Some(slice) => {
                    self.syntax.clear();
                    if let Some(hl) = hl {
                        hl.runs_into(row, &mut self.syntax);
                    }
                    Self::fill_text_runs(
                        &self.font,
                        &mut self.runs,
                        len,
                        &self.syntax,
                        self.palette.text,
                    );
                    if let Some((local, _)) = &row_sel {
                        let fg = self.colors.selection_fg;
                        Self::restyle_in_place(&mut self.runs, &mut self.restyled, local, |run| {
                            run.color = fg
                        });
                    }
                    if let Some((local, _)) = row_selection(&marked, &range) {
                        let underline = UnderlineStyle {
                            color: Some(self.colors.cursor),
                            thickness: px(1.0),
                            wavy: false,
                        };
                        Self::restyle_in_place(&mut self.runs, &mut self.restyled, &local, |run| {
                            run.underline = Some(underline)
                        });
                    }
                    let mut materialized = false;
                    let shaped = window.text_system().shape_line_by_hash(
                        line_content_hash(slice),
                        len,
                        self.font_size,
                        &self.runs,
                        None,
                        || {
                            materialized = true;
                            slice.to_string().into()
                        },
                    );
                    if materialized {
                        hits.materialized_lines += 1;
                    }
                    Some(shaped)
                }
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

        Some(CodePrepaint {
            content_bounds: bounds,
            quads,
            markers,
            glyphs,
            navigation,
            navigation_hitbox,
            minimap,
            marker_hitbox,
            marker_pointer,
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
        window.with_content_mask(
            Some(ContentMask {
                bounds: layout.content_bounds,
            }),
            |window| {
                for q in &layout.quads {
                    match q.clip {
                        Some(clip) => {
                            window.with_content_mask(
                                Some(ContentMask { bounds: clip }),
                                |window| {
                                    window.paint_quad(fill(q.bounds, q.color));
                                },
                            );
                        }
                        None => window.paint_quad(fill(q.bounds, q.color)),
                    }
                }
                for q in &layout.markers {
                    window.paint_quad(quad(
                        q.bounds,
                        q.corners,
                        q.color,
                        px(0.),
                        q.color,
                        BorderStyle::Solid,
                    ));
                }
                if layout.marker_pointer {
                    window.set_cursor_style(CursorStyle::PointingHand, &layout.marker_hitbox);
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
            },
        );
        if let Some(minimap) = layout.minimap {
            minimap.paint(window, cx);
        }
        let view = self.view.read(cx);
        if view.navigation.drag.is_some() {
            super::navigation::bind_drag(&self.view, window);
        }
        if layout.navigation.part_at(window.mouse_position()).is_some() {
            window.set_cursor_style(CursorStyle::Arrow, &layout.navigation_hitbox);
        }
        super::navigation_paint::paint(
            layout.navigation,
            &view.navigation,
            view,
            self.colors.scrollbar_thumb,
            window,
        );
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

    fn visible_rows(content_top: f32, viewport_h: f32, line_count: usize) -> Range<usize> {
        visible_rows_at(
            f64::from(content_top.max(0.0)) / f64::from(CODE_ROW_HEIGHT),
            viewport_h,
            line_count,
        )
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
    fn gutter_never_goes_below_its_floor_and_always_reserves_the_marker_column() {
        assert_eq!(gutter_width(1, 1.0), GUTTER_MIN_W + MARKER_COLUMN_W);
        assert!(gutter_width(6, 7.5) > GUTTER_MIN_W + MARKER_COLUMN_W);
        assert_eq!(
            gutter_width(4, 7.0) - (GUTTER_PAD_L + 4.0 * 7.0 + NUM_GAP),
            MARKER_COLUMN_W,
            "the six pixel column sits left of the numbers whether or not a base is loaded"
        );
    }

    #[test]
    fn marker_hits_extend_three_pixels_left_and_stop_at_the_numbers() {
        let map = CodeHitMap {
            marker_x: 10.0,
            markers: vec![
                MarkerHit {
                    index: 0,
                    y0: 100.0,
                    y1: 136.0,
                },
                MarkerHit {
                    index: 3,
                    y0: 200.0,
                    y1: 220.0,
                },
            ],
            ..CodeHitMap::default()
        };
        assert_eq!(map.marker_at(point(px(12.), px(110.))), Some(0));
        assert_eq!(
            map.marker_at(point(px(7.), px(135.))),
            Some(0),
            "grown to the left"
        );
        assert_eq!(map.marker_at(point(px(6.), px(110.))), None);
        assert_eq!(
            map.marker_at(point(px(17.), px(110.))),
            None,
            "past the column"
        );
        assert_eq!(map.marker_at(point(px(12.), px(136.))), None);
        assert_eq!(map.marker_at(point(px(12.), px(205.))), Some(3));
        assert_eq!(CodeHitMap::default().marker_at(point(px(0.), px(0.))), None);
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

    fn max_rows_for(viewport_h: f32, line_count: usize) -> f64 {
        (line_count as f64 - f64::from(viewport_h) / f64::from(CODE_ROW_HEIGHT)).max(0.0)
    }

    #[test]
    fn reveal_keeps_the_cursor_visible_with_a_two_line_margin() {
        let viewport_h = 360.0;
        let visible = f64::from(viewport_h) / f64::from(CODE_ROW_HEIGHT);
        let max_rows = max_rows_for(viewport_h, 1_000);
        let margin = f64::from(REVEAL_MARGIN_ROWS);

        assert_eq!(reveal_rows(10, viewport_h, max_rows, 0.0), 0.0);

        let top = reveal_rows(40, viewport_h, max_rows, 0.0);
        assert!(41.0 + margin <= top + visible + 0.001);
        assert!(41.0 <= top + visible);

        let top = reveal_rows(5, viewport_h, max_rows, 600.0 / f64::from(CODE_ROW_HEIGHT));
        assert!(5.0 - margin >= top - 0.001);
        assert!(5.0 >= top);
    }

    #[test]
    fn reveal_is_clamped_to_the_document() {
        let viewport_h = 360.0;
        let max_rows = max_rows_for(viewport_h, 1_000);

        assert_eq!(reveal_rows(0, viewport_h, max_rows, 500.0), 0.0);

        let last = reveal_rows(999, viewport_h, max_rows, 0.0);
        assert!(last <= max_rows, "{last} overscrolled past {max_rows}");
        assert!(last >= 0.0);

        assert_eq!(reveal_rows(3, viewport_h, 0.0, 0.0), 0.0);
    }

    #[test]
    fn reveal_margin_degrades_on_a_tiny_viewport() {
        let viewport_h = 2.0 * CODE_ROW_HEIGHT;
        let max_rows = max_rows_for(viewport_h, 100);
        let top = reveal_rows(50, viewport_h, max_rows, 0.0);
        assert!(50.0 >= top - 0.001);
        assert!(51.0 <= top + 2.0 + 0.001);
    }

    fn scroll_for(viewport_h: f32, line_count: usize) -> CodeScroll {
        let scroll = CodeScroll::new();
        scroll.set_metrics(
            Bounds::new(point(px(0.), px(0.)), size(px(800.), px(viewport_h))),
            line_count,
        );
        scroll
    }

    #[test]
    fn a_scroll_position_stops_at_the_last_screenful() {
        let scroll = scroll_for(360.0, 1_000);
        assert_eq!(scroll.max_rows(), 1_000.0 - 20.0);
        assert!(scroll.set_rows(5_000.0));
        assert_eq!(scroll.rows(), 980.0);
        assert!(scroll.set_rows(-5.0));
        assert_eq!(scroll.rows(), 0.0);
    }

    #[test]
    fn a_document_shorter_than_the_viewport_never_scrolls() {
        let scroll = scroll_for(360.0, 3);
        assert_eq!(scroll.max_rows(), 0.0);
        assert!(!scroll.scroll_by_pixels(3.0 * CODE_ROW_HEIGHT));
        assert_eq!(scroll.rows(), 0.0);
    }

    #[test]
    fn a_collapsed_viewport_neither_scrolls_nor_divides_by_zero() {
        let scroll = scroll_for(0.0, 300_000);
        assert_eq!(scroll.visible_rows(), 0.0);
        assert_eq!(scroll.max_rows(), 0.0);
        assert!(!scroll.scroll_by_pixels(3.0 * CODE_ROW_HEIGHT));
        assert_eq!(scroll.rows(), 0.0);
        assert_eq!(scroll.content_top(), 0.0);
    }

    #[test]
    fn a_frame_laid_out_at_zero_height_keeps_the_position_it_had() {
        let scroll = scroll_for(360.0, 300_000);
        assert!(scroll.set_rows(5_000.0));

        scroll.set_metrics(
            Bounds::new(point(px(0.), px(0.)), size(px(800.), px(0.))),
            300_000,
        );
        assert_eq!(scroll.rows(), 5_000.0, "a collapsed frame must not rewind");

        scroll.set_metrics(
            Bounds::new(point(px(0.), px(0.)), size(px(800.), px(360.))),
            300_000,
        );
        assert_eq!(scroll.rows(), 5_000.0);

        scroll.reset_rows();
        assert_eq!(scroll.rows(), 0.0);
    }

    #[test]
    fn losing_lines_above_the_viewport_rebinds_the_position_to_the_document() {
        let scroll = scroll_for(360.0, 1_000);
        assert!(scroll.set_rows(980.0));
        scroll.set_line_count(40);
        assert_eq!(scroll.rows(), 20.0);
        scroll.set_line_count(10);
        assert_eq!(scroll.rows(), 0.0);
    }

    #[test]
    fn the_scrollbar_reads_the_position_through_the_scrollable_handle() {
        let scroll = scroll_for(360.0, 1_000);
        assert!(scroll.set_rows(100.0));
        assert_eq!(
            ScrollableHandle::offset(&scroll),
            point(px(0.), px(-100.0 * CODE_ROW_HEIGHT))
        );
        assert_eq!(
            ScrollableHandle::max_offset(&scroll),
            point(px(0.), px(980.0 * CODE_ROW_HEIGHT))
        );
        ScrollableHandle::set_offset(&scroll, point(px(0.), px(-42.0 * CODE_ROW_HEIGHT)));
        assert_eq!(scroll.rows(), 42.0);
        let metrics = crate::widgets::scrollbar::metrics(&scroll).expect("an overflowing document");
        assert!(metrics.thumb_top > 0.0);
    }

    #[test]
    fn a_pixel_delta_scrolls_a_fractional_row_without_rounding() {
        let scroll = scroll_for(360.0, 1_000);
        assert!(scroll.scroll_by_pixels(7.5));
        assert_eq!(scroll.content_top(), 7.5);
    }

    #[test]
    fn a_row_position_is_stable_across_two_identical_frames_of_a_huge_file() {
        let line_count = 300_000usize;
        let viewport_h = 720.0f32;
        let scroll_rows = line_count as f64 - f64::from(viewport_h) / f64::from(CODE_ROW_HEIGHT);
        let origin_y = 137.0f64;
        let row = line_count - 1;

        let position = |scroll: f64| {
            device_round(
                origin_y + (row as f64 - scroll) * f64::from(CODE_ROW_HEIGHT),
                2.0,
            )
        };

        assert_eq!(position(scroll_rows), position(scroll_rows));
        let rows = visible_rows_at(scroll_rows, viewport_h, line_count);
        assert!(rows.contains(&row), "{rows:?} must reach the last row");
        assert!(
            (position(scroll_rows) - position(scroll_rows + 1.0) - CODE_ROW_HEIGHT).abs() < 0.001,
            "one scrolled row must move the last row by exactly one row height"
        );
    }

    #[test]
    fn device_rounding_lands_on_whole_device_pixels() {
        assert_eq!(device_round(10.3, 2.0), 10.5);
        assert_eq!(device_round(10.3, 1.0), 10.0);
        assert_eq!(device_round(10.3, 0.0), 10.3);
        assert_eq!(device_round(-0.2, 2.0), 0.0);
    }

    #[test]
    fn identical_lines_at_different_rows_share_a_content_hash() {
        let rope = ropey::Rope::from_str("let a = 1;\nlet b = 2;\nlet a = 1;\n");
        let first = line_content_hash(rope.byte_slice(0..10));
        let third = line_content_hash(rope.byte_slice(22..32));
        let second = line_content_hash(rope.byte_slice(11..21));
        assert_eq!(first, third, "same content must reuse one layout");
        assert_ne!(first, second, "different content must not collide here");
    }

    #[test]
    fn no_corpus_line_collides_with_a_different_text() {
        use super::super::bench_corpus::{LARGE_RUST_BYTES, rust_source};
        use std::collections::HashMap;

        let rope = ropey::Rope::from_str(&rust_source(LARGE_RUST_BYTES));
        let mut seen: HashMap<(u64, usize), String> = HashMap::new();
        let mut hashed = 0usize;
        for row in 0..rope.len_lines() {
            let start = rope.line_to_byte(row);
            let mut end = if row + 1 < rope.len_lines() {
                rope.line_to_byte(row + 1)
            } else {
                rope.len_bytes()
            };
            while end > start && matches!(rope.byte(end - 1), b'\n' | b'\r') {
                end -= 1;
            }
            if end == start {
                continue;
            }
            let slice = rope.byte_slice(start..end);
            let key = (line_content_hash(slice), end - start);
            let text = slice.to_string();
            if let Some(previous) = seen.insert(key, text.clone()) {
                assert_eq!(previous, text, "row {row} shares a key with another text");
            }
            hashed += 1;
        }
        assert!(hashed >= 10_000, "{hashed} rows is too small a corpus");
    }

    #[test]
    fn a_split_rope_line_hashes_like_a_contiguous_one() {
        let mut rope = ropey::Rope::from_str("");
        for chunk in ["abcdef", "ghijkl", "mnopqr"] {
            let at = rope.len_bytes();
            rope.insert(rope.byte_to_char(at), chunk);
        }
        let contiguous = ropey::Rope::from_str("abcdefghijklmnopqr");
        assert_eq!(
            line_content_hash(rope.byte_slice(..)),
            line_content_hash(contiguous.byte_slice(..))
        );
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
            materialized_lines: 0,
            materialized_numbers: 0,
            lines: vec![None],
            ..CodeHitMap::default()
        };

        let below = map.offset_at(&doc, point(px(500.), px(10_000.)));
        assert_eq!(below, doc.len_bytes());
        assert_eq!(map.offset_at(&doc, point(px(0.), px(-10_000.))), 0);
        assert_eq!(map.offset_at(&doc, point(px(0.), px(105.))), 4);
    }
}
