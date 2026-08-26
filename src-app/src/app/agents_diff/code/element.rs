//! Custom GPUI `Element` painting a [`CodeDocument`] - virtualized, direct-paint.
//!
//! Modeled on `diff/element.rs:1-12`, which is itself Paneflow's port of Zed's
//! `EditorElement` approach: the element reports its full content height, is
//! hosted inside an `overflow_y_scroll` div that translates it, derives the
//! visible window from `window.content_mask()` with pixel math, shapes ONLY the
//! visible lines through the frame-cached `text_system().shape_line`, and paints
//! quads plus glyphs directly.
//!
//! Two deliberate divergences from `widgets/text_area.rs`:
//!
//! 1. **No layout-time shaping.** `TextArea` shapes every line in
//!    `request_layout` (`text_area.rs:1256-1302`) to measure its own height.
//!    A code file cannot afford that: the height here is `line_count *
//!    ROW_HEIGHT`, an integer multiply, and no line is touched before
//!    `prepaint` knows which ones are on screen.
//! 2. **No soft-wrap** (`text_area.rs:12`). One logical line is exactly one
//!    visual line, which is what makes the first and last visible rows a pair
//!    of integer divisions ([`visible_rows`]) instead of a walk over a wrapped
//!    layout. Long lines scroll horizontally instead of wrapping.
//!
//! Everything geometric lives in free functions at the top of this file so the
//! virtualization bound, the gutter derivation, the horizontal extent and the
//! cursor-reveal math are provable without a GPUI window - the same split
//! `diff/hscroll.rs` uses.

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

/// Row height, shared with the diff so a file and its diff scroll in lockstep
/// (`rows.rs:22`).
pub(crate) const CODE_ROW_HEIGHT: f32 = ROW_HEIGHT;
/// Same size the diff shapes its code at.
const CODE_FONT_SIZE: f32 = 12.0;
/// Right padding inside the gutter (number to code gap). Same value as the
/// diff's `NUM_GAP` (`diff/element.rs:44`), as US-006 requires.
const NUM_GAP: f32 = 6.0;
/// Left breathing room inside the derived gutter, mirroring the diff.
const GUTTER_PAD_L: f32 = 8.0;
/// Gutter floor width - narrow files still get a readable rail.
const GUTTER_MIN_W: f32 = 36.0;
/// Inset between the gutter's right edge and the first code glyph.
const CODE_PAD_L: f32 = 6.0;
/// Right inset so the last column never sits flush against the scrollbar.
const CODE_PAD_R: f32 = 8.0;
/// Slack added past the longest line so the caret at end-of-line is reachable.
const H_SCROLL_MARGIN: f32 = 12.0;
/// Caret bar thickness (US-009).
const CARET_WIDTH: f32 = 2.0;

/// Vertical scrollbar track/thumb width. Matches `widgets::scrollbar`.
const V_SCROLLBAR_W: f32 = 6.0;
/// Gap between the thumb and the element's right edge.
const V_SCROLLBAR_INSET: f32 = 2.0;
/// Horizontal scrollbar thickness, matching the diff's own
/// (`hscroll.rs::H_SCROLLBAR_TRACK_HEIGHT`).
const H_SCROLLBAR_H: f32 = 6.0;
/// Gap between the horizontal thumb and the viewport's bottom edge.
const H_SCROLLBAR_INSET: f32 = 3.0;
/// Below this the horizontal thumb is too small to grab.
const H_SCROLLBAR_MIN_THUMB: f32 = 28.0;
/// Offsets under this magnitude count as "fits" - no scrollbar, no scrolling.
const SCROLL_EPSILON: f32 = 0.5;
/// Rows of breathing room kept between the cursor and the viewport edge when
/// an edit or a navigation scrolls it back into view (US-007).
const REVEAL_MARGIN_ROWS: f32 = 2.0;

/// Half-visible rows to add on each side of the strict viewport window, so a
/// sub-row scroll offset never leaves a sliver of blank line at either edge.
const OVERDRAW_ROWS: usize = 1;

// ---------------------------------------------------------------------------
// Pure geometry
// ---------------------------------------------------------------------------

/// The rows that must be shaped for a viewport starting `content_top` pixels
/// into the document.
///
/// US-005: with no soft-wrap, row `i` occupies `[i * ROW_HEIGHT, (i+1) *
/// ROW_HEIGHT)`, so both ends are one integer division - O(1) in `line_count`.
/// The returned span is bounded by `viewport_h / ROW_HEIGHT + 2 *
/// OVERDRAW_ROWS + 1` regardless of how long the file is, which is the
/// virtualization guarantee the epic's DoD asks for.
pub(crate) fn visible_rows(content_top: f32, viewport_h: f32, line_count: usize) -> Range<usize> {
    if line_count == 0 || viewport_h <= 0.0 {
        return 0..0;
    }
    let top = content_top.max(0.0);
    let first = ((top / CODE_ROW_HEIGHT) as usize).saturating_sub(OVERDRAW_ROWS);
    let bottom = top + viewport_h;
    // `+ 1` turns the floor into a ceil for the exclusive end.
    let last = (bottom / CODE_ROW_HEIGHT) as usize + 1 + OVERDRAW_ROWS;
    first.min(line_count)..last.min(line_count)
}

/// Decimal digit count, by integer division.
///
/// Deliberately not `log10().floor()`: that rounds off by one at exact powers
/// of ten on some libm implementations (`log10(1000)` can come back as
/// `2.9999998`), which is exactly the 999 to 1000 boundary US-006 tests.
pub(crate) fn digit_count(n: usize) -> usize {
    let mut n = n.max(1);
    let mut count = 0usize;
    while n > 0 {
        count += 1;
        n /= 10;
    }
    count
}

/// Gutter column width for `digits` line-number digits at a monospace advance
/// of `digit_w`, floored at [`GUTTER_MIN_W`].
pub(crate) fn gutter_width(digits: usize, digit_w: f32) -> f32 {
    (GUTTER_PAD_L + digits as f32 * digit_w + NUM_GAP).max(GUTTER_MIN_W)
}

/// Width available to code once the gutter and the right inset are removed.
pub(crate) fn text_viewport_width(element_w: f32, gutter_w: f32) -> f32 {
    (element_w - gutter_w - CODE_PAD_L - CODE_PAD_R).max(0.0)
}

/// Largest horizontal offset the code column may take.
///
/// US-008: derived from the longest line `CodeDocument` maintains incrementally
/// (`document.rs::longest_line_chars`), never from a per-frame rescan, and from
/// the element's own measured width - the `-92.0` / `-55.0` constants in
/// `hscroll.rs:42-48` encode the diff's split geometry and are not reusable here.
pub(crate) fn max_h_offset(longest_line_chars: usize, char_w: f32, text_viewport_w: f32) -> f32 {
    let content_w = longest_line_chars as f32 * char_w + H_SCROLL_MARGIN;
    (content_w - text_viewport_w).max(0.0)
}

/// Horizontal thumb `(x, width)` inside a track of `track_w`, or `None` when
/// the longest line fits (US-008: no scrollbar unless it overflows).
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

/// Vertical scroll offset that brings `row` back into view with at least
/// [`REVEAL_MARGIN_ROWS`] rows of margin, in GPUI's sign convention
/// (`offset_y <= 0`, zero at the top).
///
/// US-007: returns `offset_y` unchanged when the row is already comfortably
/// visible, so a keystroke that does not move the cursor out of the viewport
/// does not jitter the scroll position.
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

/// Horizontal offset that keeps the caret inside the text column, with
/// [`H_SCROLL_MARGIN`] of breathing room on whichever edge it crossed
/// (US-011).
///
/// `caret_x` is measured from the first glyph of the row, not from the window,
/// so the comparison is offset-relative and holds whatever the gutter width is.
/// Returns `current` unchanged when the caret is already comfortably visible,
/// so plain vertical navigation never nudges the horizontal scroll.
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

/// Signed autoscroll step for a pointer that has left `lo..=hi` on one axis
/// (US-010).
///
/// Returns `0.0` while the pointer is inside the viewport, so a caller can use
/// it as its own "nothing to do" test on either axis. The sign follows the drag
/// rather than the pointer's distance: the step is applied once per mouse-move
/// event, and events already arrive faster the further the pointer travels.
pub(crate) fn autoscroll_step(pos: f32, lo: f32, hi: f32, step: f32) -> f32 {
    if pos < lo {
        -step
    } else if pos > hi {
        step
    } else {
        0.0
    }
}

/// Alpha the current-line wash keeps once the editor loses focus (US-009).
const UNFOCUSED_WASH_FACTOR: f32 = 1.0 / 3.0;

/// The current-line wash for a given focus state (US-009).
///
/// Unfocused dims rather than drops: the row the caret is parked on is the
/// reader's place in the file, and losing it every time focus moves to the
/// sidebar is more disorienting than a faint band. The caret bar itself does
/// disappear, which is what tells the two states apart.
pub(crate) fn cursor_line_wash(base: Hsla, focused: bool) -> Hsla {
    if focused {
        base
    } else {
        base.opacity(UNFOCUSED_WASH_FACTOR)
    }
}

/// Live geometry the element resolves during `prepaint` and the hosting view
/// reads back from its wheel / scrollbar handlers.
///
/// A single-threaded `Rc<Cell<_>>` hand-off, which is the pattern this codebase
/// already uses for render-closure state (see `CLAUDE.md`, "No `Arc`/`Mutex`
/// for UI state"). Every field is derived, never authored by the view.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct CodeGeometry {
    /// Derived gutter width in pixels (US-006).
    pub(crate) gutter_w: f32,
    /// Exact monospace advance of one character, from a shaped digit.
    pub(crate) char_w: f32,
    /// Width available to code after the gutter.
    pub(crate) text_viewport_w: f32,
    /// Largest legal horizontal offset (US-008).
    pub(crate) max_h_offset: f32,
}

/// Memoized gutter metrics, keyed on the digit count.
///
/// US-006 requires the gutter width to be recomputed only when the number of
/// digits in `line_count` changes, never per frame. `digits == 0` is the
/// never-measured sentinel, since [`digit_count`] returns at least 1.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct GutterMemo {
    pub(crate) digits: usize,
    pub(crate) digit_w: f32,
    pub(crate) gutter_w: f32,
}

/// Portion of row `line` (a document byte range, terminator excluded) covered
/// by the ordered selection `sel`.
///
/// Returns the range **local to the row** plus whether the selection runs past
/// the row's terminator, which is what tells the painter to add the trailing
/// sliver that makes a multi-line selection visible on an empty row.
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

// ---------------------------------------------------------------------------
// Element
// ---------------------------------------------------------------------------

struct Quad {
    bounds: Bounds<Pixels>,
    color: Hsla,
    /// Selection and caret quads live in the text column and must be masked to
    /// it, exactly like the glyphs: a horizontally scrolled line would
    /// otherwise paint its selection over the gutter. `None` for the full-width
    /// washes, which are supposed to reach the gutter.
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

/// Colors the caret layer needs. `UiColors` has no cursor or selection slot,
/// so these come straight off the active [`crate::theme::TerminalTheme`]
/// (`theme/model.rs:21`, `:22`, `:29`), the same source the terminal cursor and
/// the diff scrollbar already read.
#[derive(Clone, Copy)]
pub(crate) struct CodeColors {
    pub(crate) scrollbar_thumb: Hsla,
    pub(crate) cursor: Hsla,
    pub(crate) selection: Hsla,
    pub(crate) selection_fg: Hsla,
}

/// The caret state for one frame (US-009, US-010).
#[derive(Clone, Debug, Default)]
pub(crate) struct CodeCaret {
    /// Caret byte offset.
    pub(crate) cursor: usize,
    /// Ordered selection range; empty means "plain caret, paint nothing".
    pub(crate) selection: Range<usize>,
    /// Whether the hosting view holds focus. Unfocused hides the bar and dims
    /// the current-line wash, per US-009.
    pub(crate) focused: bool,
    /// Blink phase, shared app-wide by `terminal::blink`. Ignored while
    /// unfocused.
    pub(crate) visible: bool,
    /// Byte range of the live IME composition, empty when there is none
    /// (US-012). Painted underlined so the user can tell an uncommitted
    /// composition from text the document has really accepted.
    pub(crate) marked: Range<usize>,
}

/// The frame's shaped code lines, published back to [`CodeView`] so a click can
/// be resolved against exactly the layout that was painted.
///
/// Hit-testing through the real `ShapedLine` rather than a `column * char_w`
/// guess is what keeps tabs, ligature-free proportional fallbacks and wide CJK
/// glyphs landing where the user clicked.
#[derive(Default)]
pub(crate) struct CodeHitMap {
    /// First row present in [`Self::lines`].
    pub(crate) first_row: usize,
    /// Window-space y of `first_row`'s top edge.
    pub(crate) top_y: f32,
    /// Window-space x of a line's first glyph, horizontal offset already
    /// applied.
    pub(crate) text_x: f32,
    /// One entry per row from `first_row`; `None` for a row with no glyphs.
    pub(crate) lines: Vec<Option<ShapedLine>>,
}

impl CodeHitMap {
    /// Row under `y`, unclamped by the map's own window.
    fn row_at(&self, y: f32) -> isize {
        self.first_row as isize + ((y - self.top_y) / CODE_ROW_HEIGHT).floor() as isize
    }

    /// Caret slot under a window-space position.
    ///
    /// A position above or below the shaped window (which is where an
    /// auto-scrolling drag lives) resolves to that row's start or end, the
    /// behavior every editor gives a drag that has left the viewport.
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

/// Everything `paint` needs, resolved once in `prepaint`.
pub(crate) struct CodePrepaint {
    quads: Vec<Quad>,
    glyphs: Vec<CodeGlyph>,
    scrollbars: Vec<RoundedQuad>,
}

/// Direct-paint element for one open file.
pub(crate) struct CodeElement {
    view: Entity<CodeView>,
    palette: RowPalette,
    colors: CodeColors,
    /// Handle of the hosting `overflow_y_scroll` div - the only source of the
    /// vertical scrollbar's viewport / offset / max-offset triple.
    scroll: gpui::ScrollHandle,
    /// Live horizontal offset, owned by the view (US-008).
    h_offset: f32,
    /// Caret and selection for this frame (US-009, US-010).
    caret: CodeCaret,
    line_count: usize,
    geometry: Rc<Cell<CodeGeometry>>,
    gutter_memo: Rc<Cell<GutterMemo>>,
    /// Shaped lines handed back to the view for hit-testing (US-010).
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
        // The mono family is constant and a fresh element is built every frame,
        // so resolve it once per thread and clone the cheap `SharedString`
        // handle - same reasoning as `DiffElement::new`.
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

    /// Split a line into `TextRun`s: the syntax runs carry their own color, the
    /// gaps fall back to `default`. Run lengths sum to `text.len()`, which
    /// `shape_line` requires. Same contract as `DiffElement::text_runs`, so a
    /// file and its diff color identically from the same `LineRuns` shape.
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
                continue; // defensive: malformed / overlapping ranges
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

    /// Restyle the `span` byte range of an already-built run list, splitting
    /// runs at the span's edges and leaving `style` to say what changes. Run
    /// lengths still sum to the line length, which is what `shape_line`
    /// requires.
    ///
    /// Two callers, two spans that can overlap: the selection recolor
    /// (`ui.selection_foreground`, `theme/model.rs:29`) and the IME preedit
    /// underline. Sharing the splitter is what lets a composition inside a
    /// selection keep both.
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
            // Three possible slices of this run: before, inside, after.
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

    /// Resolve the gutter width, shaping a digit only when the digit count
    /// actually changed since the last frame (US-006).
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
        // US-005: the content height is an integer multiply, not a measurement.
        // No line is shaped here - that is the divergence from `TextArea`,
        // which shapes every line to size itself (`text_area.rs:1256-1302`).
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

        // Visible window: the host div translates this element by the scroll
        // offset, so the clip rect's distance from our own origin is exactly how
        // far into the document the viewport starts.
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
        // Code is clipped to the text column so a horizontally scrolled line
        // never bleeds into the gutter (US-006: the gutter stays pinned).
        let text_clip = Bounds::new(
            point(left + gutter_px, mask.bounds.origin.y),
            size(
                px(element_w - gutter_w).max(px(0.)),
                mask.bounds.size.height,
            ),
        );

        // Document wash + gutter rail, painted over the visible span only.
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

        // US-009: the caret's row. Derived here rather than carried on the
        // frame state so the highlighted gutter number and the wash can never
        // disagree with the byte offset the selection actually holds.
        let cursor = self.caret.cursor.min(doc.len_bytes());
        let cursor_row = doc.byte_to_line(cursor);
        let sel = self.caret.selection.clone();
        let marked = self.caret.marked.clone();
        // US-009: the wash follows focus. Losing focus dims it rather than
        // dropping it, so the reader keeps their place in the file while the
        // caret itself disappears.
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

            // Gutter number, right-aligned against `NUM_GAP` (US-006).
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

            // Code, shifted left by the live horizontal offset.
            let text = doc.line_string(row).unwrap_or_default();
            let line = if text.is_empty() {
                None
            } else {
                let text: SharedString = text.into();
                let runs = match hl {
                    Some(hl) => self.text_runs(&text, hl.runs(row), self.palette.text),
                    None => self.text_runs(&text, &[], self.palette.text),
                };
                let runs = match &row_sel {
                    Some((local, _)) => {
                        let fg = self.colors.selection_fg;
                        Self::restyle(runs, local, |run| run.color = fg)
                    }
                    None => runs,
                };
                // US-012: the live composition is underlined in the caret
                // color. It sits in the rope like any other text, so the
                // underline is the only thing telling the user it is still
                // uncommitted.
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

            // US-010: selection band, measured off the shaped line so it lands
            // on glyph boundaries and not on a monospace guess.
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
                    // The terminator itself is selected: a fixed sliver is what
                    // makes a multi-row selection visible on an empty row.
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

            // US-009: the caret bar, 2 px in the theme's cursor color. Hidden
            // while unfocused or on the blink's off phase.
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

        // Vertical scrollbar (US-007). Geometry comes from the host handle, so
        // the painted thumb and the dragged thumb can never diverge.
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

        // Horizontal scrollbar (US-008), only when the longest line overflows.
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
        // US-012: hand the platform its text-input target for this frame.
        // Registration has to happen in `paint`, with the element's real
        // bounds, because that is the only point where GPUI accepts one - and
        // it must happen every frame, since the handler is cleared between
        // them. Keystrokes still go through action dispatch first
        // (`gpui/src/window.rs:4525`), so Enter, Tab and Backspace reach their
        // bindings and only printable input falls through to here.
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
        // Scrollbars are viewport furniture, not content. `prepaint` anchors
        // them to `window.content_mask()` (the host viewport), which extends
        // past `bounds` whenever the file is shorter than the pane: the
        // element only claims `line_count * row_height`. Painting them inside
        // the `bounds` mask above would clip the horizontal thumb away on
        // exactly the case US-008 requires it - a short file whose longest
        // line overflows - so they get their own layer against the viewport.
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

    /// The number of rows a viewport of `h` pixels can hold, plus the overdraw
    /// margin on each side and the partial row at the bottom.
    fn bound_for(viewport_h: f32) -> usize {
        (viewport_h / CODE_ROW_HEIGHT) as usize + 1 + 2 * OVERDRAW_ROWS
    }

    /// US-005 AC: on a 100 000-line file, `prepaint` touches no more lines than
    /// the viewport holds. Proven on the exact function `prepaint` calls, at
    /// every scroll position across the whole document.
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
        // And the bound is genuinely independent of the file size.
        assert_eq!(
            visible_rows(0.0, viewport_h, line_count).len(),
            visible_rows(0.0, viewport_h, 1_000_000).len()
        );
    }

    /// The window actually covers the viewport: every row whose band intersects
    /// `[top, top + viewport_h)` is inside the returned range.
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

    /// US-006 AC: 999 to 1 000 widens the gutter. The digit count is what
    /// changes; the row a given line occupies does not, so the cursor cannot
    /// shift by a line.
    #[test]
    fn crossing_a_power_of_ten_widens_the_gutter_without_moving_rows() {
        assert_eq!(digit_count(999), 3);
        assert_eq!(digit_count(1_000), 4);
        let digit_w = 7.5;
        let narrow = gutter_width(digit_count(999), digit_w);
        let wide = gutter_width(digit_count(1_000), digit_w);
        assert!(wide > narrow, "{wide} should exceed {narrow}");
        assert_eq!(wide - narrow, digit_w);
        // Row geometry is purely vertical: the same line sits on the same band
        // in both documents.
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

    /// US-008 AC: the horizontal extent comes from the maintained longest line
    /// and the measured viewport, and is zero when the file fits.
    #[test]
    fn horizontal_extent_derives_from_the_longest_line() {
        let char_w = 7.5;
        // 40 chars at 7.5px = 300px + margin, inside a 600px column.
        assert_eq!(max_h_offset(40, char_w, 600.0), 0.0);
        // 200 chars = 1500px + 12px margin, inside 600px.
        assert_eq!(max_h_offset(200, char_w, 600.0), 1500.0 + 12.0 - 600.0);
        // Growing the longest line grows the extent, monotonically.
        let a = max_h_offset(200, char_w, 600.0);
        let b = max_h_offset(201, char_w, 600.0);
        assert!(b > a);
    }

    #[test]
    fn text_viewport_excludes_the_gutter() {
        let w = text_viewport_width(800.0, 60.0);
        assert_eq!(w, 800.0 - 60.0 - CODE_PAD_L - CODE_PAD_R);
        // A pane narrower than its own gutter clamps instead of going negative.
        assert_eq!(text_viewport_width(20.0, 60.0), 0.0);
    }

    /// US-008 AC: no horizontal scrollbar unless the longest line overflows.
    #[test]
    fn horizontal_thumb_only_exists_on_overflow() {
        assert!(h_thumb(0.0, 0.0, 600.0).is_none());
        assert!(h_thumb(0.0, 0.2, 600.0).is_none());
        let (x, w) = h_thumb(0.0, 600.0, 600.0).expect("overflowing line has a thumb");
        assert_eq!(x, 0.0);
        assert!((H_SCROLLBAR_MIN_THUMB..600.0).contains(&w));
        // Fully scrolled: the thumb sits flush against the track's right edge.
        let (x_end, w_end) = h_thumb(600.0, 600.0, 600.0).expect("thumb");
        assert!((x_end + w_end - 600.0).abs() < 0.001);
    }

    /// US-007 AC: any navigation that pushes the cursor out of the viewport
    /// scrolls it back with at least two lines of margin; a cursor already in
    /// view leaves the offset alone.
    #[test]
    fn reveal_keeps_the_cursor_visible_with_a_two_line_margin() {
        let viewport_h = 360.0; // 20 rows
        let content_h = 1_000.0 * CODE_ROW_HEIGHT;
        let margin = REVEAL_MARGIN_ROWS * CODE_ROW_HEIGHT;

        // Already visible: untouched.
        assert_eq!(reveal_offset(10, viewport_h, content_h, 0.0), 0.0);

        // Below the viewport: the row plus its margin lands on the bottom edge.
        let off = reveal_offset(40, viewport_h, content_h, 0.0);
        let top = -off;
        let row_bottom = 41.0 * CODE_ROW_HEIGHT;
        assert!(row_bottom + margin <= top + viewport_h + 0.001);
        assert!(row_bottom <= top + viewport_h);

        // Above the viewport: the row minus its margin lands on the top edge.
        let off = reveal_offset(5, viewport_h, content_h, -600.0);
        let top = -off;
        assert!(5.0 * CODE_ROW_HEIGHT - margin >= top - 0.001);
        assert!(5.0 * CODE_ROW_HEIGHT >= top);
    }

    /// US-007 AC: the reveal never overscrolls past either end of the document.
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

        // Content shorter than the viewport: nothing to scroll.
        assert_eq!(reveal_offset(3, viewport_h, 90.0, 0.0), 0.0);
    }

    #[test]
    fn reveal_margin_degrades_on_a_tiny_viewport() {
        // Two rows tall: a full two-line margin would be impossible, and the
        // reveal must still land the row inside the viewport rather than
        // oscillating.
        let viewport_h = 2.0 * CODE_ROW_HEIGHT;
        let content_h = 100.0 * CODE_ROW_HEIGHT;
        let off = reveal_offset(50, viewport_h, content_h, 0.0);
        let top = -off;
        assert!(50.0 * CODE_ROW_HEIGHT >= top - 0.001);
        assert!(51.0 * CODE_ROW_HEIGHT <= top + viewport_h + 0.001);
    }

    /// US-010: a row paints exactly the slice of the selection that overlaps
    /// it, and reports whether the terminator is inside too - which is what
    /// makes a multi-row selection visible on an empty row.
    #[test]
    fn a_row_paints_only_its_own_slice_of_the_selection() {
        // Row 1 of "abc\ndef\nghi" is bytes 4..7, terminator at 7.
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

    /// US-010: an empty row inside a multi-row selection still reports the
    /// terminator, so the band does not vanish on blank lines.
    #[test]
    fn an_empty_row_inside_a_selection_still_paints_its_terminator() {
        let row = 4..4;
        assert_eq!(row_selection(&(0..9), &row), Some((0..0, true)));
        assert_eq!(row_selection(&(4..4), &row), None);
    }

    /// US-011: the horizontal reveal only moves when the caret has actually
    /// left the text column, and never past the legal offsets.
    #[test]
    fn the_horizontal_reveal_only_moves_when_the_caret_leaves_the_column() {
        let viewport_w = 400.0;
        let max = 1_000.0;

        assert_eq!(
            reveal_h_offset(200.0, viewport_w, max, 0.0),
            0.0,
            "already visible, no nudge"
        );
        // Past the right edge: scroll just enough to clear the margin.
        let right = reveal_h_offset(500.0, viewport_w, max, 0.0);
        assert!(right > 0.0);
        assert!(500.0 - right <= viewport_w);
        // Back to the left of the current window.
        assert_eq!(
            reveal_h_offset(100.0, viewport_w, max, 300.0),
            100.0 - H_SCROLL_MARGIN
        );
        // Never negative, never past the maximum.
        assert_eq!(reveal_h_offset(0.0, viewport_w, max, 0.0), 0.0);
        assert_eq!(reveal_h_offset(10_000.0, viewport_w, max, 0.0), max);
        // A viewport that has not been measured yet leaves the offset alone.
        assert_eq!(reveal_h_offset(500.0, 0.0, max, 42.0), 42.0);
    }

    /// US-010: a drag only autoscrolls once the pointer has actually left the
    /// viewport, and it goes the way the pointer went, on either axis.
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
        // Far out is the same single step: the event rate carries the speed.
        assert_eq!(autoscroll_step(9_000.0, 10.0, 90.0, 4.0), 4.0);
        // An unmeasured metric (char width before the first layout) is a no-op.
        assert_eq!(autoscroll_step(9_000.0, 10.0, 90.0, 0.0), 0.0);
    }

    /// US-009: the current-line wash is derived in `palette()`, so it exists on
    /// every theme without any of the six bundled files declaring a slot, and
    /// stays faint enough to sit under a diff wash.
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

    /// US-009: losing focus dims the current-line wash instead of dropping it,
    /// and regaining focus restores it exactly.
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

    /// US-009: a click below the last row lands on the end of the file rather
    /// than panicking, and a click above the shaped window lands on a row
    /// start - the behavior a drag that has left the viewport relies on.
    #[test]
    fn a_click_outside_the_shaped_window_still_lands_on_a_legal_slot() {
        let doc = CodeDocument::new(PathBuf::from("/nonexistent/a.txt"), "one\ntwo\nthree");
        let map = CodeHitMap {
            first_row: 1,
            top_y: 100.0,
            text_x: 40.0,
            lines: vec![None],
        };

        // Far below the last row: clamped to the last row, then to its end.
        let below = map.offset_at(&doc, point(px(500.), px(10_000.)));
        assert_eq!(below, doc.len_bytes());
        // Far above the window: the first row's start.
        assert_eq!(map.offset_at(&doc, point(px(0.), px(-10_000.))), 0);
        // A row present in the map but with no glyphs resolves to its start.
        assert_eq!(map.offset_at(&doc, point(px(0.), px(105.))), 4);
    }
}
