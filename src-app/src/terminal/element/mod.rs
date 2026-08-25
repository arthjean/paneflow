//! Terminal cell renderer using GPUI's Element trait.
//!
//! Renders terminal cells from a backend-neutral snapshot as batched text runs
//! full ANSI color support, cell attributes, and background quads.

use std::sync::{Arc, Mutex};

use gpui::{
    App, Bounds, ContentMask, Element, ElementId, Font, FontStyle, FontWeight, GlobalElementId,
    Hsla, InspectorElementId, IntoElement, LayoutId, Pixels, Point, SharedString,
    StrikethroughStyle, Style, UnderlineStyle, Window, px, relative,
};

use crate::terminal::TerminalSessionBackend;
use crate::terminal::types::{
    Cell, CellFlags, Color, Content, CopyModeCursorState, CursorShape, NamedColor,
    Point as GridPoint, RenderableCursor, SearchHighlight, SelectionRange, TerminalWindowSize,
    terminal_metric_to_u16,
};

pub(super) mod color;
mod font;
mod geometry;
mod hyperlink;
mod paint;
#[cfg(debug_assertions)]
pub(super) mod pixel_probe;

use color::{convert_color, rgb_to_hsla};
pub(crate) use font::{
    DEFAULT_CELL_WIDTH, DEFAULT_FONT_SIZE, DEFAULT_LINE_HEIGHT, normalize_font_weight_key,
};
pub use font::{
    MAX_FONT_SIZE, MIN_FONT_SIZE, global_font_size, resolve_font_family, resolve_frame_metrics,
    sanitize_font_override,
};
use geometry::CellGeometry;
pub use hyperlink::{
    detect_code_paths_on_line_mapped, detect_file_paths_on_line_mapped, detect_urls_on_line_mapped,
    is_url_scheme_openable,
};

// US-007: re-export APCA primitives so theme code (and theme tests) can
// derive and verify a contrast-validated `selection_foreground` color
// without duplicating the algorithm. `ensure_minimum_contrast` is also
// used locally by `build_layout` (cell-vs-bg pass); `apca_contrast` is
// referenced only by theme tests but must be re-exported through this
// module to honour `pub(crate)` visibility.
#[allow(unused_imports)] // re-exported for theme tests; not used inside this module
pub(crate) use color::apca_contrast;
pub(crate) use color::ensure_minimum_contrast;
// US-015: re-export the scrollbar geometry so the view's mouse handlers
// (`crate::terminal::input`) can hit-test against the painted strip. `paint`
// is a private module, so the type must surface through `element`.
pub(crate) use paint::scrollbar::ScrollbarMetrics;

/// APCA minimum Lc (lightness contrast) threshold.
/// Lc 45 is "minimum for large fluent text" per ARC Bronze Simple Mode - matches Zed's default.
/// APCA is more accurate than WCAG 2.0 on dark backgrounds (polarity-aware, perceptually uniform).
pub(crate) const MIN_APCA_CONTRAST: f32 = 45.0;

/// Returns `true` for characters whose colors should be preserved exactly
/// (no contrast adjustment). Covers box-drawing, block elements, geometric
/// shapes, and Powerline separator symbols.
fn is_decorative_character(ch: char) -> bool {
    matches!(
        ch as u32,
        0x2500..=0x257F   // Box Drawing (─ │ ┌ ┐ └ ┘ etc.)
        | 0x2580..=0x259F // Block Elements (▀ ▄ █ ░ ▒ ▓ etc.)
        | 0x25A0..=0x25FF // Geometric Shapes (■ ▶ ● etc.)
        | 0xE0B0..=0xE0B7 // Powerline: right/left arrows
        | 0xE0B8..=0xE0BF // Powerline: bottom/top triangles
        | 0xE0C0..=0xE0CA // Powerline: flame, pixel separators
        | 0xE0CC..=0xE0D1 // Powerline: waveform, hex (excludes 0xE0CB)
        | 0xE0D2..=0xE0D7 // Powerline: trapezoids, inverted triangles
    )
}

/// US-007: returns `true` if a cell at `point` (viewport coordinates) lies
/// inside the active `SelectionRange` (whose `start`/`end` are in scrollback
/// coordinates and require `display_offset` correction). Mirrors the
/// `selection_rects` generation block below - first/last/middle line ranges
/// for linear selections, axis-aligned rectangle for block selections.
///
/// Used inside the cell loop to override the cell's `fg` with the theme's
/// `selection_foreground`, guaranteeing readable text under the selection
/// quad on themes whose `selection` background is close in luminance to
/// common ANSI colors.
fn is_cell_in_selection(point: GridPoint, sel: &SelectionRange, display_offset: usize) -> bool {
    let start_line = sel.start.line.0 + display_offset as i32;
    let end_line = sel.end.line.0 + display_offset as i32;
    let start_col = sel.start.column.0;
    let end_col = sel.end.column.0;

    let cell_line = point.line.0;
    let cell_col = point.column.0;

    if sel.is_block {
        let (l_min, l_max) = if start_line <= end_line {
            (start_line, end_line)
        } else {
            (end_line, start_line)
        };
        let (c_min, c_max) = if start_col <= end_col {
            (start_col, end_col)
        } else {
            (end_col, start_col)
        };
        return cell_line >= l_min && cell_line <= l_max && cell_col >= c_min && cell_col <= c_max;
    }

    // Linear selection: normalize so (s_line, s_col) is reading-order start.
    let ((s_line, s_col), (e_line, e_col)) =
        if start_line < end_line || (start_line == end_line && start_col <= end_col) {
            ((start_line, start_col), (end_line, end_col))
        } else {
            ((end_line, end_col), (start_line, start_col))
        };
    if cell_line < s_line || cell_line > e_line {
        false
    } else if s_line == e_line {
        cell_col >= s_col && cell_col <= e_col
    } else if cell_line == s_line {
        cell_col >= s_col
    } else if cell_line == e_line {
        cell_col <= e_col
    } else {
        true
    }
}

/// Merge vertically adjacent background rects that share the same column span
/// and color, reducing the number of paint_quad() calls. The input rects are
/// already horizontally merged (same-row, same-color, contiguous columns).
fn merge_background_regions(mut rects: Vec<LayoutRect>) -> Vec<LayoutRect> {
    if rects.len() <= 1 {
        return rects;
    }
    // Sort by (col, num_cols, color bits, line) so vertically adjacent candidates
    // are consecutive in the list.
    rects.sort_unstable_by(|a, b| {
        a.col
            .cmp(&b.col)
            .then(a.num_cols.cmp(&b.num_cols))
            .then(a.color.h.total_cmp(&b.color.h))
            .then(a.color.s.total_cmp(&b.color.s))
            .then(a.color.l.total_cmp(&b.color.l))
            .then(a.color.a.total_cmp(&b.color.a))
            .then(a.line.cmp(&b.line))
    });

    let mut merged: Vec<LayoutRect> = Vec::with_capacity(rects.len());
    let mut iter = rects.into_iter();
    let mut current = iter.next().expect(
        "merge_background_regions: rects.len() >= 2 guaranteed by the len() <= 1 early return",
    );

    for next in iter {
        if next.col == current.col
            && next.num_cols == current.num_cols
            && next.color == current.color
            && next.line == current.line + current.num_lines as i32
        {
            current.num_lines += next.num_lines;
        } else {
            merged.push(current);
            current = next;
        }
    }
    merged.push(current);
    merged
}

fn codex_panel_background_for_terminal(theme: &crate::theme::TerminalTheme) -> Hsla {
    if theme.background.l > 0.5 {
        crate::theme::ui_colors_with(theme).subtle
    } else {
        Hsla::from(gpui::rgb(0x383838))
    }
}

fn terminal_panel_background(
    raw_bg: Color,
    resolved_bg: Hsla,
    theme: &crate::theme::TerminalTheme,
) -> Hsla {
    let is_codex_surface_gray = match raw_bg {
        Color::Named(NamedColor::BrightBlack) | Color::Indexed(8 | 236 | 237) => true,
        Color::Spec(rgb) => rgb.r == rgb.g && rgb.g == rgb.b && (40..=56).contains(&rgb.r),
        _ => false,
    };

    if is_codex_surface_gray {
        codex_panel_background_for_terminal(theme)
    } else {
        resolved_bg
    }
}

fn resolved_cell_background(
    cell_fg: Color,
    cell_bg: Color,
    flags: CellFlags,
    theme: &crate::theme::TerminalTheme,
) -> Hsla {
    let raw_bg = if flags.contains(CellFlags::INVERSE) {
        cell_fg
    } else {
        cell_bg
    };

    if matches!(raw_bg, Color::Named(NamedColor::Background)) {
        // Default-background cells paint nothing: the pane card behind the
        // element owns the fill, and only it is clipped to the card radius.
        gpui::transparent_black()
    } else {
        terminal_panel_background(raw_bg, convert_color(raw_bg, theme), theme)
    }
}

fn selection_marker_color() -> Hsla {
    Hsla {
        h: 0.5,
        s: 0.8,
        l: 0.65,
        a: 0.9,
    }
}

// ---------------------------------------------------------------------------
// Layout types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct CellDimensions {
    pub cell_width: Pixels,
    pub line_height: Pixels,
}

#[derive(Clone)]
pub struct TerminalFrameMetrics {
    pub dimensions: CellDimensions,
    pub base_font: Font,
    pub font_size: Pixels,
}

struct BatchedTextRun {
    /// US-047: `SharedString` (not `String`) so the per-frame paint pass
    /// (`shape_line`) refcount-bumps the text instead of deep-copying it +
    /// re-wrapping into an `Arc<str>` every frame. Built once per flush.
    text: SharedString,
    font: Font,
    color: Hsla,
    underline: Option<UnderlineStyle>,
    strikethrough: Option<StrikethroughStyle>,
    line: i32,
    col_start: usize,
}

struct LayoutRect {
    line: i32,
    num_lines: usize,
    col: usize,
    num_cols: usize,
    color: Hsla,
}

/// A block/half-block character rendered as a filled quad instead of a font glyph.
/// This eliminates subpixel gaps between adjacent block elements in pixel art (logos, etc.).
struct BlockQuad {
    line: i32,
    col: usize,
    num_cols: usize, // 2 for wide chars
    color: Hsla,
    /// Fractional coverage of the cell: (x_start, y_start, width, height) in 0.0..1.0
    coverage: (f32, f32, f32, f32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BoxDrawingShape {
    left: bool,
    right: bool,
    up: bool,
    down: bool,
    rounded: bool,
}

struct BoxDrawingGlyph {
    line: i32,
    col: usize,
    color: Hsla,
    shape: BoxDrawingShape,
}

/// Map the single-stroke box glyphs used by terminal TUIs to geometry. These
/// glyphs must meet at exact cell boundaries, which font advances cannot
/// guarantee across font fallback and fractional scaling.
fn box_drawing_shape(c: char) -> Option<BoxDrawingShape> {
    let shape = match c {
        '─' => (true, true, false, false, false),
        '│' => (false, false, true, true, false),
        '┌' => (false, true, false, true, false),
        '┐' => (true, false, false, true, false),
        '└' => (false, true, true, false, false),
        '┘' => (true, false, true, false, false),
        '├' => (false, true, true, true, false),
        '┤' => (true, false, true, true, false),
        '┬' => (true, true, false, true, false),
        '┴' => (true, true, true, false, false),
        '┼' => (true, true, true, true, false),
        '╭' => (false, true, false, true, true),
        '╮' => (true, false, false, true, true),
        '╯' => (true, false, true, false, true),
        '╰' => (false, true, true, false, true),
        _ => return None,
    };
    Some(BoxDrawingShape {
        left: shape.0,
        right: shape.1,
        up: shape.2,
        down: shape.3,
        rounded: shape.4,
    })
}

/// If `c` is a Unicode block element, return its fractional cell coverage as
/// a slice of `(x, y, w, h)` rects (origin at the cell's top-left, in 0..1).
/// Returns `None` for characters that should be rendered as normal glyphs.
///
/// Most block-element codepoints are a single rectangle, but the multi-quadrant
/// chars (`▙ ▚ ▛ ▜ ▞ ▟`) need 2 rects each - that's why this returns a slice.
/// Each emitted rect becomes one [`BlockQuad`] at the call site, all sharing
/// the same outer cell boundaries through [`geometry::cell_x_boundaries`].
///
/// US-005 fallback: extension beyond the original `U+2580..U+2590` range, after
/// the pixel probe revealed Claude Code's banner robot uses single + multi
/// quadrant blocks (`U+2596..U+259F`) and the upper one-eighth block (`U+2594`).
/// Without this extension these codepoints fall back to font glyphs that don't
/// fully fill the cell, producing the visible vertical gaps documented in the
/// `debug_block_char_rendering.md` memory.
fn block_char_coverages(c: char) -> Option<&'static [(f32, f32, f32, f32)]> {
    match c {
        // U+2580..U+2590 - half / eighth blocks (one rect each)
        '▀' => Some(&[(0.0, 0.0, 1.0, 0.5)]), // U+2580 Upper half
        '▁' => Some(&[(0.0, 7.0 / 8.0, 1.0, 1.0 / 8.0)]), // U+2581 Lower 1/8
        '▂' => Some(&[(0.0, 6.0 / 8.0, 1.0, 2.0 / 8.0)]), // U+2582 Lower 1/4
        '▃' => Some(&[(0.0, 5.0 / 8.0, 1.0, 3.0 / 8.0)]), // U+2583 Lower 3/8
        '▄' => Some(&[(0.0, 0.5, 1.0, 0.5)]), // U+2584 Lower half
        '▅' => Some(&[(0.0, 3.0 / 8.0, 1.0, 5.0 / 8.0)]), // U+2585 Lower 5/8
        '▆' => Some(&[(0.0, 2.0 / 8.0, 1.0, 6.0 / 8.0)]), // U+2586 Lower 3/4
        '▇' => Some(&[(0.0, 1.0 / 8.0, 1.0, 7.0 / 8.0)]), // U+2587 Lower 7/8
        '█' => Some(&[(0.0, 0.0, 1.0, 1.0)]), // U+2588 Full block
        '▉' => Some(&[(0.0, 0.0, 7.0 / 8.0, 1.0)]), // U+2589 Left 7/8
        '▊' => Some(&[(0.0, 0.0, 6.0 / 8.0, 1.0)]), // U+258A Left 3/4
        '▋' => Some(&[(0.0, 0.0, 5.0 / 8.0, 1.0)]), // U+258B Left 5/8
        '▌' => Some(&[(0.0, 0.0, 0.5, 1.0)]), // U+258C Left half
        '▍' => Some(&[(0.0, 0.0, 3.0 / 8.0, 1.0)]), // U+258D Left 3/8
        '▎' => Some(&[(0.0, 0.0, 2.0 / 8.0, 1.0)]), // U+258E Left 1/4
        '▏' => Some(&[(0.0, 0.0, 1.0 / 8.0, 1.0)]), // U+258F Left 1/8
        '▐' => Some(&[(0.5, 0.0, 0.5, 1.0)]), // U+2590 Right half

        // ─── US-005 fallback extension ────────────────────────────────────
        // U+2594 - Upper 1/8 (the lone "upper edge" block, complement of ▁)
        '▔' => Some(&[(0.0, 0.0, 1.0, 1.0 / 8.0)]),

        // U+2596..U+259D - single quadrants
        '▖' => Some(&[(0.0, 0.5, 0.5, 0.5)]), // U+2596 Quadrant lower left
        '▗' => Some(&[(0.5, 0.5, 0.5, 0.5)]), // U+2597 Quadrant lower right
        '▘' => Some(&[(0.0, 0.0, 0.5, 0.5)]), // U+2598 Quadrant upper left
        '▝' => Some(&[(0.5, 0.0, 0.5, 0.5)]), // U+259D Quadrant upper right

        // U+2599..U+259F - multi-quadrants (2 rects each, each rect already
        // shares its outer edges with the surrounding cell's boundary array
        // via `paint_block_quads` → no inter-rect gaps possible).
        '▙' => Some(&[
            // U+2599 Quadrant upper-left + entire lower half
            (0.0, 0.0, 0.5, 0.5),
            (0.0, 0.5, 1.0, 0.5),
        ]),
        '▚' => Some(&[
            // U+259A Diagonal upper-left + lower-right
            (0.0, 0.0, 0.5, 0.5),
            (0.5, 0.5, 0.5, 0.5),
        ]),
        '▛' => Some(&[
            // U+259B Entire upper half + lower-left
            (0.0, 0.0, 1.0, 0.5),
            (0.0, 0.5, 0.5, 0.5),
        ]),
        '▜' => Some(&[
            // U+259C Entire upper half + lower-right
            (0.0, 0.0, 1.0, 0.5),
            (0.5, 0.5, 0.5, 0.5),
        ]),
        '▞' => Some(&[
            // U+259E Diagonal upper-right + lower-left
            (0.5, 0.0, 0.5, 0.5),
            (0.0, 0.5, 0.5, 0.5),
        ]),
        '▟' => Some(&[
            // U+259F Quadrant upper-right + entire lower half
            (0.5, 0.0, 0.5, 0.5),
            (0.0, 0.5, 1.0, 0.5),
        ]),
        _ => None,
    }
}

pub(crate) struct CursorInfo {
    line: i32,
    col: usize,
    shape: CursorShape,
    color: Hsla,
    cell_bg: Hsla,
    wide: bool,
    /// Character under the cursor (None for whitespace or non-Block shapes).
    text: Option<char>,
    bold: bool,
    italic: bool,
}

#[derive(Clone, Copy)]
struct CursorCellContext<'a> {
    desired_cols: usize,
    desired_rows: usize,
    theme: &'a crate::theme::TerminalTheme,
}

fn selection_marker_cursor(
    cells: &[Cell],
    line: i32,
    col: usize,
    color: Hsla,
    ctx: CursorCellContext<'_>,
) -> Option<CursorInfo> {
    if line < 0 || line >= ctx.desired_rows as i32 || col >= ctx.desired_cols {
        return None;
    }

    let cell = cells
        .iter()
        .find(|cell| cell.point.line.0 == line && cell.point.column.0 == col);

    let (wide, text, bold, italic, cell_bg) = cell
        .map(|cell| {
            let is_spacer = cell.flags.contains(CellFlags::WIDE_CHAR_SPACER);
            (
                cell.flags.contains(CellFlags::WIDE_CHAR),
                (!is_spacer && cell.c != '\0').then_some(cell.c),
                cell.flags.contains(CellFlags::BOLD) || cell.flags.contains(CellFlags::BOLD_ITALIC),
                cell.flags.contains(CellFlags::ITALIC)
                    || cell.flags.contains(CellFlags::BOLD_ITALIC),
                resolved_cell_background(cell.fg, cell.bg, cell.flags, ctx.theme),
            )
        })
        .unwrap_or((
            false,
            None,
            false,
            false,
            resolved_cell_background(
                Color::Named(NamedColor::Foreground),
                Color::Named(NamedColor::Background),
                CellFlags::empty(),
                ctx.theme,
            ),
        ));

    Some(CursorInfo {
        line,
        col,
        shape: CursorShape::Block,
        color,
        cell_bg,
        wide,
        text,
        bold,
        italic,
    })
}

fn cursor_from_content(
    cursor: RenderableCursor,
    cursor_visible: bool,
    focused: bool,
    cursor_color: Hsla,
    default_cursor_shape: CursorShape,
    theme: &crate::theme::TerminalTheme,
) -> Option<CursorInfo> {
    if matches!(cursor.shape, CursorShape::Hidden) || !cursor_visible || !focused {
        return None;
    }

    let shape = match (default_cursor_shape, cursor.shape) {
        (CursorShape::Vintage, CursorShape::Block) => CursorShape::Vintage,
        (CursorShape::DoubleUnderline, CursorShape::Underline) => CursorShape::DoubleUnderline,
        _ => cursor.shape,
    };

    let text = if matches!(shape, CursorShape::Block) && cursor.text != ' ' && cursor.text != '\0' {
        Some(cursor.text)
    } else {
        None
    };

    Some(CursorInfo {
        line: cursor.point.line.0,
        col: cursor.point.column.0,
        shape,
        color: cursor_color,
        cell_bg: resolved_cell_background(cursor.fg, cursor.bg, cursor.flags, theme),
        wide: cursor.wide,
        text,
        bold: cursor.bold,
        italic: cursor.italic,
    })
}

fn focused_copy_mode_cursor(
    copy_mode_cursor: Option<&CopyModeCursorState>,
    focused: bool,
) -> Option<&CopyModeCursorState> {
    focused.then_some(copy_mode_cursor).flatten()
}

/// Window-free inputs to [`layout_from_snapshot`]. Everything the layout pass
/// needs that would otherwise be read from `&mut Window` / `&App` / the `Term`
/// lock / `&self`, captured as plain values. `build_layout` fills this from a
/// neutral [`Content`] snapshot ([`content_from_term`]) plus the content mask;
/// the golden-frame net fills it from a fixed fixture so the entire layout is
/// reproducible with no display. The cells are the backend-neutral
/// [`crate::terminal::types::Cell`] (EP-003) - no alacritty types reach here.
pub(crate) struct LayoutInputs<'a> {
    pub cells: Arc<[Cell]>,
    /// Cursor as snapshotted from the grid (before the copy-mode / selection
    /// anchor override, which `layout_from_snapshot` applies internally).
    pub cursor: Option<CursorInfo>,
    pub selection_range: Option<SelectionRange>,
    pub copy_mode_cursor: Option<&'a CopyModeCursorState>,
    pub search_highlights: &'a [SearchHighlight],
    pub display_offset: usize,
    pub history_size: usize,
    pub desired_cols: usize,
    pub desired_rows: usize,
    /// Viewport cull range (rows `[first, last)`), derived from the content
    /// mask in `build_layout`. Tests pass `0..desired_rows` to render all rows.
    pub first_visible_row: i32,
    pub last_visible_row: i32,
    pub dims: CellDimensions,
    /// Base font, resolved once by the caller (config-dependent). Bold/italic
    /// variants are derived per-cell. Passed in so the layout pass never reads
    /// the font config and stays deterministic.
    pub base_font: Font,
    pub theme: &'a crate::theme::TerminalTheme,
    pub exited: Option<i32>,
    pub exit_signal: Option<String>,
    pub integrated_glyphs_enabled: bool,
    pub color_emoji_enabled: bool,
}

pub struct LayoutState {
    batched_runs: Vec<BatchedTextRun>,
    rects: Vec<LayoutRect>,
    block_quads: Vec<BlockQuad>,
    box_drawing_glyphs: Vec<BoxDrawingGlyph>,
    selection_rects: Vec<LayoutRect>,
    search_rects: Vec<LayoutRect>,
    cursor: Option<CursorInfo>,
    /// Secondary marker for keyboard copy mode selection.
    anchor_cursor: Option<CursorInfo>,
    dimensions: CellDimensions,
    background_color: Hsla,
    scrollbar_thumb: Hsla,
    exited: Option<i32>,
    /// US-004: signal name if the child was killed by a signal; the exit
    /// overlay renders this instead of the exit code to flag a crash.
    exit_signal: Option<String>,
    /// Scroll position for scrollbar indicator (0 = at bottom)
    display_offset: usize,
    /// Total scrollback history size
    history_size: usize,
    /// Number of columns in the terminal grid
    desired_cols: usize,
    /// Number of rows in the terminal grid
    desired_rows: usize,
    /// Theme color for hyperlink underline.
    link_text_color: Hsla,
    /// Cursor position bounds for IME popup positioning (pixel coordinates).
    ime_cursor_bounds: Option<Bounds<Pixels>>,
    /// Whether emoji glyphs should use GPUI's platform color-emoji path.
    color_emoji_enabled: bool,
}

// ---------------------------------------------------------------------------
// Cell style - used for batching comparison
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq)]
struct CellStyle {
    font: Font,
    fg: Hsla,
    bg: Hsla,
    underline: bool,
    undercurl: bool,
    strikethrough: bool,
}

// ---------------------------------------------------------------------------
// TerminalElement
// ---------------------------------------------------------------------------

pub struct TerminalElement {
    backend: TerminalSessionBackend,
    cursor_visible: bool,
    focused: bool,
    exited: Option<i32>,
    /// US-004: signal name if the child was killed by a signal; the exit
    /// overlay renders this instead of the exit code to flag a crash.
    exit_signal: Option<String>,
    /// Shared origin - updated in paint() so mouse handlers know the element position.
    element_origin: Arc<Mutex<Point<Pixels>>>,
    /// Search match highlights to paint
    search_highlights: Vec<SearchHighlight>,
    /// Copy mode cursor position (grid coordinates), if copy mode is active
    copy_mode_cursor: Option<CopyModeCursorState>,
    /// Ctrl+hovered hyperlink range for underline rendering (line, start_col, end_col).
    hovered_link_range: Option<(i32, usize, usize)>,
    /// IME preedit text to render at cursor position.
    ime_marked_text: String,
    /// Focus handle for IME input handler registration.
    focus_handle: gpui::FocusHandle,
    /// Terminal view entity for IME callbacks.
    terminal_view: gpui::Entity<crate::terminal::TerminalView>,
    /// User-configured fallback cursor shape before applications override it.
    default_cursor_shape: CursorShape,
    /// User-configured cursor color override; falls back to theme cursor.
    cursor_color_override: Option<Hsla>,
    /// Gate for clearing pre-resize shell startup content on first render.
    needs_initial_clear: Arc<std::sync::atomic::AtomicBool>,
    /// Last terminal window size measured by layout and sent to the PTY.
    terminal_window_size: Arc<Mutex<Option<TerminalWindowSize>>>,
    /// US-015: shared sink for the painted scrollbar geometry. `paint()` writes
    /// the current frame's [`ScrollbarMetrics`] (or `None`) here so the view's
    /// mouse handlers can hit-test interactive scroll against the exact strip
    /// that was drawn. Same single-thread sharing as [`element_origin`].
    scrollbar_metrics: Arc<Mutex<Option<ScrollbarMetrics>>>,
    /// EP-006 US-017: search-match positions as lines-from-grid-bottom,
    /// snapshotted by the view at render time (empty when no search).
    /// Painted as decimated ticks on the scrollbar track.
    search_rail_lines: Vec<usize>,
    /// When active, default terminal backgrounds are painted transparent so
    /// the parent surface/window material can show through.
    /// When enabled, block-element glyphs are rendered as built-in quads.
    integrated_glyphs_enabled: bool,
    /// When enabled, emoji glyphs are rendered through GPUI's color path.
    color_emoji_enabled: bool,
    /// Font and cell metrics resolved once by the view for this frame.
    frame_metrics: TerminalFrameMetrics,
    /// True while the terminal is in DEC alternate screen.
    alt_screen: bool,
    /// Timestamp of the keystroke that triggered this render, for latency measurement.
    #[cfg(debug_assertions)]
    last_keystroke_at: Option<std::time::Instant>,
}

impl TerminalElement {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        backend: TerminalSessionBackend,
        cursor_visible: bool,
        focused: bool,
        exited: Option<i32>,
        exit_signal: Option<String>,
        element_origin: Arc<Mutex<Point<Pixels>>>,
        search_highlights: Vec<SearchHighlight>,
        copy_mode_cursor: Option<CopyModeCursorState>,
        hovered_link_range: Option<(i32, usize, usize)>,
        ime_marked_text: String,
        focus_handle: gpui::FocusHandle,
        terminal_view: gpui::Entity<crate::terminal::TerminalView>,
        needs_initial_clear: Arc<std::sync::atomic::AtomicBool>,
        terminal_window_size: Arc<Mutex<Option<TerminalWindowSize>>>,
        scrollbar_metrics: Arc<Mutex<Option<ScrollbarMetrics>>>,
        search_rail_lines: Vec<usize>,
        default_cursor_shape: CursorShape,
        cursor_color_override: Option<Hsla>,
        integrated_glyphs_enabled: bool,
        color_emoji_enabled: bool,
        frame_metrics: TerminalFrameMetrics,
        alt_screen: bool,
        #[cfg(debug_assertions)] last_keystroke_at: Option<std::time::Instant>,
    ) -> Self {
        Self {
            backend,
            cursor_visible,
            focused,
            exited,
            exit_signal,
            element_origin,
            search_highlights,
            copy_mode_cursor,
            hovered_link_range,
            ime_marked_text,
            focus_handle,
            terminal_view,
            default_cursor_shape,
            needs_initial_clear,
            terminal_window_size,
            scrollbar_metrics,
            search_rail_lines,
            cursor_color_override,
            integrated_glyphs_enabled,
            color_emoji_enabled,
            frame_metrics,
            alt_screen,
            #[cfg(debug_assertions)]
            last_keystroke_at,
        }
    }

    fn build_layout(
        &self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut App,
    ) -> LayoutState {
        let dims = self.frame_metrics.dimensions;
        let theme = crate::theme::active_theme();

        // Ghostty's window padding model (`src/renderer/size.zig`): the inset
        // is subtracted from the viewport on BOTH edges of each axis before the
        // grid is sized, so the cells sit the same distance from every side of
        // the pane card instead of hugging the right and bottom edges.
        let inset_x = px(crate::app::constants::PANE_CONTENT_INSET_X);
        let inset_y = px(crate::app::constants::PANE_CONTENT_INSET_Y);
        let available_width = (bounds.size.width - inset_x * 2.).max(px(0.0));
        let available_height = (bounds.size.height - inset_y * 2.).max(px(0.0));
        // `next_up().floor()` guards against f32 rounding error: when pixel
        // bounds are an exact multiple of the cell metric (24 lines × 16 px),
        // direct `.floor()` can drop one cell because the division yields
        // `23.99999…` instead of `24.0`. Stepping to the next representable
        // float before flooring matches Zed's `TerminalBounds::num_lines`.
        let desired_cols = (available_width / dims.cell_width)
            .next_up()
            .floor()
            .max(1.0) as usize;
        // `.max(1.0)` mirrors `desired_cols` above (U-046): on a zero/near-zero
        // -height pane this keeps the row count ≥ 1 so no downstream consumer
        // can underflow a `desired_rows - 1` or index a 0-len boundary array.
        let desired_rows = (available_height / dims.line_height)
            .next_up()
            .floor()
            .max(1.0) as usize;

        // Viewport culling range from the content mask - the only remaining
        // Window dependency. Computing it before the terminal snapshot lets the
        // seam skip offscreen scrollback rows instead of allocating them and
        // dropping them later.
        let content_mask = window.content_mask();
        let visible_top = content_mask.bounds.origin.y;
        let visible_bottom = visible_top + content_mask.bounds.size.height;
        // Row 0 starts one vertical inset below the element's own top edge, so
        // the culling range is measured from the grid origin, not from `bounds`.
        let grid_top = bounds.origin.y + inset_y;
        let first_visible_row = ((visible_top - grid_top) / dims.line_height)
            .floor()
            .max(0.0) as i32;
        let last_visible_row = ((visible_bottom - grid_top) / dims.line_height)
            .ceil()
            .max(0.0) as i32;

        // Snapshot the grid into neutral owned data. Alacritty resizes under
        // the same lock and therefore returns the requested dimensions here.
        // Ghostty applies the resize on its runtime thread, so it can return the
        // previous complete grid for one frame before its wakeup publishes the
        // resized snapshot. The layout below must use the snapshot dimensions,
        // never combine old cells with the newly requested GPUI dimensions.
        let cursor_color = self.cursor_color_override.unwrap_or(theme.cursor);
        let window_size = TerminalWindowSize::new(
            desired_cols,
            desired_rows,
            terminal_metric_to_u16(dims.cell_width.as_f32()),
            terminal_metric_to_u16(dims.line_height.as_f32()),
        );

        // A provisional layout can still match the 120x40 bootstrap grid. Keep
        // the one-shot armed until a backend actually resizes and clears, or the
        // next real layout would preserve startup bytes in Ghostty's scrollback.
        let clear_on_resize = self
            .needs_initial_clear
            .load(std::sync::atomic::Ordering::Relaxed);
        let (content, initial_clear_consumed): (Content, bool) = self.backend.render_content(
            window_size,
            first_visible_row,
            last_visible_row,
            clear_on_resize,
        );
        if initial_clear_consumed {
            self.needs_initial_clear
                .store(false, std::sync::atomic::Ordering::Relaxed);
        }
        let notify_resize = {
            let mut last = self
                .terminal_window_size
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if *last == Some(window_size) {
                false
            } else {
                *last = Some(window_size);
                true
            }
        };
        if notify_resize {
            self.backend.notify_window_size(window_size);
        }

        let render_cols = content.cols.max(1);
        let render_rows = content.rows.max(1);
        let display_offset = content.display_offset;
        let history_size = content.history_size;
        let selection_range = content.selection;

        let cursor_snapshot = cursor_from_content(
            content.cursor,
            self.cursor_visible,
            self.focused,
            cursor_color,
            self.default_cursor_shape,
            &theme,
        );
        let copy_mode_cursor =
            focused_copy_mode_cursor(self.copy_mode_cursor.as_ref(), self.focused);

        let cells = content.cells;

        layout_from_snapshot(LayoutInputs {
            cells,
            cursor: cursor_snapshot,
            selection_range,
            copy_mode_cursor,
            search_highlights: &self.search_highlights,
            display_offset,
            history_size,
            desired_cols: render_cols,
            desired_rows: render_rows,
            first_visible_row,
            last_visible_row,
            dims,
            base_font: self.frame_metrics.base_font.clone(),
            theme: &theme,
            exited: self.exited,
            exit_signal: self.exit_signal.clone(),
            integrated_glyphs_enabled: self.integrated_glyphs_enabled,
            color_emoji_enabled: self.color_emoji_enabled,
        })
    }
}

/// Window-free rendering layout pass (US-002 golden-frame net).
///
/// Produces the complete [`LayoutState`] from a pure snapshot of the grid,
/// theme, and cell dimensions - no `Window`/`App` access and no `Term` lock.
/// [`TerminalElement::build_layout`] is the thin Window-coupled wrapper that
/// snapshots the grid under lock, measures the cell, and derives the viewport
/// cull range from the content mask, then delegates here. Keeping this seam
/// pure lets the golden-frame net assert total layout state over a fixed
/// corpus with no GPU/display.
pub(crate) fn layout_from_snapshot(inputs: LayoutInputs<'_>) -> LayoutState {
    let LayoutInputs {
        cells,
        cursor: cursor_snapshot,
        selection_range,
        copy_mode_cursor,
        search_highlights,
        display_offset,
        history_size,
        desired_cols,
        desired_rows,
        first_visible_row,
        last_visible_row,
        dims,
        base_font,
        theme,
        exited,
        exit_signal,
        integrated_glyphs_enabled,
        color_emoji_enabled,
    } = inputs;

    // The terminal never fills its own bounds. Its host pane card carries the
    // background and the rounded corners; a base fill here would repaint the
    // arcs square (GPUI does not clip children to a parent radius).
    let background_color = gpui::transparent_black();
    let selection_color = theme.selection;

    let cursor_snapshot = cursor_snapshot.and_then(|mut cursor| {
        cursor.line += display_offset as i32;
        (cursor.line >= 0 && cursor.line < desired_rows as i32).then_some(cursor)
    });

    // Override cursor with copy mode cursor when active, and surface the
    // selection anchor as a distinct secondary marker (tmux-style).
    let (cursor_snapshot, anchor_cursor) = if let Some(cm) = copy_mode_cursor {
        let display_line = cm.grid_line + display_offset as i32;
        let marker_color = selection_marker_color();
        let cursor_ctx = CursorCellContext {
            desired_cols,
            desired_rows,
            theme,
        };

        let main = selection_marker_cursor(
            cells.as_ref(),
            display_line,
            cm.col,
            marker_color,
            cursor_ctx,
        );

        let anchor = cm.anchor_grid_line.and_then(|anchor_line| {
            let display_anchor = anchor_line + display_offset as i32;
            selection_marker_cursor(
                cells.as_ref(),
                display_anchor,
                cm.anchor_col,
                marker_color,
                cursor_ctx,
            )
        });

        (main, anchor)
    } else if selection_range.is_some() {
        // Mouse selection is cleaner as highlight-only: the range itself is the
        // affordance, and auto-copy clears it on mouse-up.
        (None, None)
    } else {
        (cursor_snapshot, None)
    };

    let mut batch = BatchAccumulator::new(base_font.clone());
    let mut rects: Vec<LayoutRect> = Vec::new();
    let mut block_quads: Vec<BlockQuad> = Vec::new();
    let mut box_drawing_glyphs: Vec<BoxDrawingGlyph> = Vec::new();
    let mut current_rect: Option<LayoutRect> = None;
    let mut last_line: i32 = i32::MIN;
    let mut previous_cell_had_extras = false;

    for cell in cells.iter() {
        let Cell {
            point,
            c,
            fg: cell_fg,
            bg: cell_bg,
            flags,
            zerowidth: zw,
            hyperlink,
        } = cell;
        let point = *point;
        let flags = *flags;

        // Viewport culling: skip rendering for rows outside the visible content mask.
        if point.line.0 < first_visible_row || point.line.0 >= last_visible_row {
            continue;
        }

        // Skip wide char spacers (trailing cell of CJK chars)
        if flags.contains(CellFlags::WIDE_CHAR_SPACER) {
            continue;
        }

        // Line change → flush batch and rect
        if point.line.0 != last_line {
            batch.flush();
            if let Some(rect) = current_rect.take() {
                rects.push(rect);
            }
            last_line = point.line.0;
        }

        // Compute colors - INVERSE swap on raw ANSI tags, then tag-based
        // default-background skip (Zed parity: structural check, not HSLA compare).
        let (raw_fg, raw_bg) = if flags.contains(CellFlags::INVERSE) {
            (*cell_bg, *cell_fg)
        } else {
            (*cell_fg, *cell_bg)
        };
        let mut fg = convert_color(raw_fg, theme);
        let bg = terminal_panel_background(raw_bg, convert_color(raw_bg, theme), theme);

        // DIM/faint (SGR 2): reduce foreground opacity (applied after INVERSE)
        if flags.contains(CellFlags::DIM) {
            fg.a *= 0.7;
        }

        // Enforce minimum foreground/background contrast.
        // Skip when:
        //  - the character is decorative (box-drawing, Powerline, blocks),
        //    where APCA adjustment would destroy the intended visual shape.
        //  - the app explicitly chose the fg color via truecolor SGR
        //    (`Color::Spec`) or the xterm-256 palette indices 16-255
        //    (the 6×6×6 RGB cube at 16..=231 and the 24-step grayscale ramp
        //    at 232..=255). Apps that pick a specific color there (bat,
        //    delta, lazygit, Neovim themes) expect it to render exactly;
        //    APCA washing the foreground breaks their palettes.
        //    Indices 0..=15 still go through contrast correction (US-018):
        //    they map to theme-defined ANSI slots and can clash with the
        //    theme background (e.g. `\e[38;5;0m` on a dark theme).
        //    Mirrors Zed `terminal::is_app_chosen_exact_color` (PR #54565).
        let skip_contrast = matches!(raw_fg, Color::Spec(_) | Color::Indexed(16..=255));
        if !is_decorative_character(*c) && !skip_contrast {
            fg = ensure_minimum_contrast(fg, bg, MIN_APCA_CONTRAST);
        }

        // US-007: cells inside the selection rect get the precomputed
        // contrast-validated `selection_foreground` (computed at theme-
        // load time against `selection`). This replaces the cell-vs-
        // background contrast we just enforced - selected text needs
        // contrast against the selection quad painted ON TOP of the
        // cell background, not against the cell background itself.
        // Because `fg` is part of `CellStyle` and `BatchAccumulator::
        // can_append` compares CellStyle by equality, this override
        // also breaks batched runs at selection boundaries with no
        // explicit accumulator change.
        //
        // Decorative characters (box-drawing, Powerline separators,
        // block elements) are skipped: their color encodes visual
        // shape (e.g. Powerline arrows transitioning between segment
        // colors), and overriding `fg` to `selection_foreground`
        // would destroy that meaning. Same exclusion as the
        // cell-vs-bg `ensure_minimum_contrast` pass above.
        if let Some(sel) = &selection_range
            && !is_decorative_character(*c)
            && is_cell_in_selection(point, sel, display_offset)
        {
            fg = theme.selection_foreground;
        }

        // Background rect - paint for ALL cells. Default-bg cells normally use
        // ansi_background (the theme's actual background) to contrast with the
        // slightly darker widget fill, creating visible depth for TUI content.
        // With terminal material enabled, only those default backgrounds become
        // transparent; explicit ANSI/app backgrounds stay opaque.
        let cell_cols = if flags.contains(CellFlags::WIDE_CHAR) {
            2
        } else {
            1
        };
        let cell_bg_color = resolved_cell_background(*cell_fg, *cell_bg, flags, theme);
        match &mut current_rect {
            Some(rect)
                if rect.line == point.line.0
                    && rect.color == cell_bg_color
                    && rect.col + rect.num_cols == point.column.0 =>
            {
                rect.num_cols += cell_cols;
            }
            _ => {
                if let Some(rect) = current_rect.take() {
                    rects.push(rect);
                }
                current_rect = Some(LayoutRect {
                    line: point.line.0,
                    num_lines: 1,
                    col: point.column.0,
                    num_cols: cell_cols,
                    color: cell_bg_color,
                });
            }
        }

        // Skip space fillers following cells with zero-width extras (emoji sequences)
        let c = *c;
        if c == ' ' && previous_cell_had_extras {
            previous_cell_had_extras = false;
            continue;
        }

        // Track whether this cell has combining/zero-width characters
        let has_extras = matches!(zw, Some(chars) if !chars.is_empty());

        // Skip empty cells for text runs (space or NUL)
        if c == ' ' || c == '\0' {
            previous_cell_had_extras = has_extras;
            batch.flush();
            continue;
        }

        // Render common single-stroke box drawing as connected paths. Every
        // segment reaches the shared cell boundary, so adjacent `─` and `│`
        // cells cannot expose font-side-bearing gaps.
        if integrated_glyphs_enabled && let Some(shape) = box_drawing_shape(c) {
            batch.flush();
            box_drawing_glyphs.push(BoxDrawingGlyph {
                line: point.line.0,
                col: point.column.0,
                color: fg,
                shape,
            });
            previous_cell_had_extras = false;
            continue;
        }

        // Render block elements as filled quads instead of font glyphs
        // to eliminate subpixel gaps between adjacent cells (pixel art,
        // Claude Code's banner robot, neofetch ASCII).
        //
        // Multi-quadrant chars (`▙ ▚ ▛ ▜ ▞ ▟`) emit two BlockQuad records
        // per cell - both share the cell's outer boundary array so adjacent
        // cells stay seamless regardless of how many sub-rects they each
        // produce.
        if integrated_glyphs_enabled && let Some(coverages) = block_char_coverages(c) {
            batch.flush();
            for &coverage in coverages {
                block_quads.push(BlockQuad {
                    line: point.line.0,
                    col: point.column.0,
                    num_cols: cell_cols,
                    color: fg,
                    coverage,
                });
            }
            previous_cell_had_extras = false;
            continue;
        }

        // Build cell style for batching comparison
        let mut font = base_font.clone();
        // OSC 8 hyperlinks must render with an underline even when the cell
        // flags don't carry `UNDERLINE` - alacritty 0.26 does not auto-set
        // the flag on OSC 8 cells, so without this we'd lose the visual
        // affordance until Ctrl/Cmd is held. Matches Zed
        // `terminal_element.rs:580`.
        let is_underline = flags.contains(CellFlags::UNDERLINE)
            || flags.contains(CellFlags::DOUBLE_UNDERLINE)
            || flags.contains(CellFlags::UNDERCURL)
            || flags.contains(CellFlags::DOTTED_UNDERLINE)
            || flags.contains(CellFlags::DASHED_UNDERLINE)
            || *hyperlink;
        let is_undercurl = flags.contains(CellFlags::UNDERCURL);
        let is_strikethrough = flags.contains(CellFlags::STRIKEOUT);

        if flags.contains(CellFlags::BOLD) || flags.contains(CellFlags::BOLD_ITALIC) {
            font.weight = FontWeight::BOLD;
        }
        if flags.contains(CellFlags::ITALIC) || flags.contains(CellFlags::BOLD_ITALIC) {
            font.style = FontStyle::Italic;
        }

        let style = CellStyle {
            font: font.clone(),
            fg,
            bg,
            underline: is_underline,
            undercurl: is_undercurl,
            strikethrough: is_strikethrough,
        };

        // Check if we can append to current batch
        if batch.can_append(&style, point.line.0, point.column.0) {
            batch.append(c, cell_cols);
        } else {
            batch.flush();
            batch.start(
                c,
                cell_cols,
                style,
                font,
                fg,
                is_underline,
                is_undercurl,
                is_strikethrough,
                point.line.0,
                point.column.0,
            );
        }

        // Append zero-width combining characters (diacriticals, ZWJ, variation selectors)
        if let Some(chars) = zw {
            batch.append_zerowidth(chars);
        }
        previous_cell_had_extras = has_extras;
    }

    // Flush remaining
    batch.flush();
    if let Some(rect) = current_rect {
        rects.push(rect);
    }
    // Vertical merge: coalesce same-column-span, same-color, adjacent-line rects
    let rects = merge_background_regions(rects);

    // Build selection highlight rects from the SelectionRange.
    // SelectionRange carries alacritty grid-line coords (scrollback = negative);
    // convert to viewport-line coords to match the cell coordinate system.
    let mut selection_rects = Vec::new();
    if let Some(sel) = &selection_range {
        let start_line = sel.start.line.0 + display_offset as i32;
        let end_line = sel.end.line.0 + display_offset as i32;
        let start_col = sel.start.column.0;
        let end_col = sel.end.column.0;
        let num_cols = desired_cols.max(1);
        let visible_start = first_visible_row.max(0).min(desired_rows as i32);
        let visible_end = last_visible_row.max(0).min(desired_rows as i32);

        let push_selection_rect =
            |rects: &mut Vec<LayoutRect>, line: i32, col: usize, rect_cols: usize| {
                if line < visible_start || line >= visible_end || col >= num_cols || rect_cols == 0
                {
                    return;
                }
                rects.push(LayoutRect {
                    line,
                    num_lines: 1,
                    col,
                    num_cols: rect_cols.min(num_cols - col),
                    color: selection_color,
                });
            };

        if sel.is_block {
            // US-007: block (rectangular) selection - emit one rect per
            // visible line covering only the columns inside the block,
            // matching the rectangular semantics of `is_cell_in_selection`
            // so the bg quad and the fg override agree on which cells
            // are "in" the selection.
            let (l_min, l_max) = if start_line <= end_line {
                (start_line, end_line)
            } else {
                (end_line, start_line)
            };
            let (c_min, c_max) = if start_col <= end_col {
                (start_col, end_col)
            } else {
                (end_col, start_col)
            };
            let block_cols = c_max.saturating_sub(c_min).saturating_add(1);
            let line_start = l_min.max(visible_start);
            let line_end = l_max.saturating_add(1).min(visible_end);
            for line in line_start..line_end {
                push_selection_rect(&mut selection_rects, line, c_min, block_cols);
            }
        } else {
            let ((s_line, s_col), (e_line, e_col)) =
                if start_line < end_line || (start_line == end_line && start_col <= end_col) {
                    ((start_line, start_col), (end_line, end_col))
                } else {
                    ((end_line, end_col), (start_line, start_col))
                };
            if s_line == e_line {
                push_selection_rect(
                    &mut selection_rects,
                    s_line,
                    s_col,
                    e_col.saturating_sub(s_col).saturating_add(1),
                );
            } else {
                // Multi-line linear: first line from start.col to end of line
                push_selection_rect(
                    &mut selection_rects,
                    s_line,
                    s_col,
                    num_cols.saturating_sub(s_col),
                );
                // Middle full lines
                let middle_start = s_line.saturating_add(1).max(visible_start);
                let middle_end = e_line.min(visible_end);
                for line in middle_start..middle_end {
                    push_selection_rect(&mut selection_rects, line, 0, num_cols);
                }
                // Last line from col 0 to end.col. `saturating_add` matches the
                // defensive arithmetic of the sibling rects (U-047): a stale
                // `end_col` from a pre-resize selection can't overflow the count.
                push_selection_rect(&mut selection_rects, e_line, 0, e_col.saturating_add(1));
            }
        }
    }

    // Build search match highlight rects
    let search_match_color = Hsla {
        h: 0.11,
        s: 0.9,
        l: 0.55,
        a: 0.45,
    }; // Amber for inactive matches
    let search_active_color = Hsla {
        h: 0.08,
        s: 1.0,
        l: 0.6,
        a: 0.7,
    }; // Brighter orange for active match

    let mut search_rects = Vec::new();
    for highlight in search_highlights {
        // Convert grid coordinates to display-relative line numbers
        // display_offset is the number of scrollback lines visible above the viewport
        // Visible lines are: -(display_offset as i32) .. (screen_lines - 1 - display_offset as i32)
        // A match at grid line L maps to display line: L.0 + display_offset as i32
        let display_line = highlight.start.line.0 + display_offset as i32;

        // Only paint if the match is in the visible area
        if display_line >= 0 && display_line < desired_rows as i32 {
            let color = if highlight.is_active {
                search_active_color
            } else {
                search_match_color
            };

            // Single-line match (search matches are always single-line)
            let col_start = highlight.start.column.0;
            let col_end = highlight.end.column.0;
            search_rects.push(LayoutRect {
                line: display_line,
                num_lines: 1,
                col: col_start,
                num_cols: col_end.saturating_sub(col_start) + 1,
                color,
            });
        }
    }

    // Compute IME cursor bounds for popup positioning
    let ime_cursor_bounds = cursor_snapshot.as_ref().map(|c| {
        let x = dims.cell_width * c.col as f32;
        let y = dims.line_height * c.line as f32;
        Bounds::new(
            Point { x, y },
            gpui::Size {
                width: dims.cell_width,
                height: dims.line_height,
            },
        )
    });

    LayoutState {
        batched_runs: batch.runs,
        rects,
        block_quads,
        box_drawing_glyphs,
        selection_rects,
        search_rects,
        cursor: cursor_snapshot,
        anchor_cursor,
        dimensions: dims,
        background_color,
        scrollbar_thumb: theme.scrollbar_thumb,
        exited,
        exit_signal,
        display_offset,
        history_size,
        desired_cols,
        desired_rows,
        link_text_color: theme.link_text,
        ime_cursor_bounds,
        color_emoji_enabled,
    }
}

struct BatchAccumulator {
    runs: Vec<BatchedTextRun>,
    text: String,
    style: Option<CellStyle>,
    font: Font,
    fg: Hsla,
    underline: bool,
    undercurl: bool,
    strikethrough: bool,
    line: i32,
    col_start: usize,
    col_end: usize, // next expected column (tracks wide chars correctly)
}

impl BatchAccumulator {
    fn new(font: Font) -> Self {
        Self {
            runs: Vec::new(),
            text: String::new(),
            style: None,
            font,
            fg: Hsla::default(),
            underline: false,
            undercurl: false,
            strikethrough: false,
            line: 0,
            col_start: 0,
            col_end: 0,
        }
    }

    fn can_append(&self, style: &CellStyle, line: i32, col: usize) -> bool {
        match &self.style {
            Some(cs) => *cs == *style && self.line == line && col == self.col_end,
            None => false,
        }
    }

    fn append(&mut self, c: char, cell_cols: usize) {
        self.text.push(c);
        self.col_end += cell_cols;
    }

    fn append_zerowidth(&mut self, chars: &[char]) {
        // If alacritty hands us combining marks before any base char has been
        // appended (rare, but the grid layout could change in future versions),
        // silently drop them rather than panicking in debug. The previous
        // `debug_assert!` could trip during legitimate render flows that the
        // user has no control over.
        if self.text.is_empty() {
            return;
        }
        for &c in chars {
            self.text.push(c);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn start(
        &mut self,
        c: char,
        cell_cols: usize,
        style: CellStyle,
        font: Font,
        fg: Hsla,
        underline: bool,
        undercurl: bool,
        strikethrough: bool,
        line: i32,
        col_start: usize,
    ) {
        self.text.push(c);
        self.style = Some(style);
        self.font = font;
        self.fg = fg;
        self.underline = underline;
        self.undercurl = undercurl;
        self.strikethrough = strikethrough;
        self.line = line;
        self.col_start = col_start;
        self.col_end = col_start + cell_cols;
    }

    fn flush(&mut self) {
        if self.text.is_empty() {
            return;
        }
        self.runs.push(BatchedTextRun {
            text: SharedString::from(std::mem::take(&mut self.text)),
            font: self.font.clone(),
            color: self.fg,
            underline: if self.underline {
                Some(UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(self.fg),
                    wavy: self.undercurl,
                })
            } else {
                None
            },
            strikethrough: if self.strikethrough {
                Some(StrikethroughStyle {
                    thickness: px(1.0),
                    color: Some(self.fg),
                })
            } else {
                None
            },
            line: self.line,
            col_start: self.col_start,
        });
        self.style = None;
    }
}

// ---------------------------------------------------------------------------
// Element trait implementation
// ---------------------------------------------------------------------------

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Option<LayoutState>;

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
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        let layout_id = window.request_layout(style, [], cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        Some(self.build_layout(bounds, window, cx))
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        #[cfg(debug_assertions)]
        let _paint_start = if crate::terminal::probe_enabled() {
            Some(std::time::Instant::now())
        } else {
            None
        };

        let Some(layout) = prepaint.take() else {
            return;
        };

        let cell_width = layout.dimensions.cell_width;
        // Offset the grid origin by the same fixed insets reserved in layout,
        // on both axes (Ghostty's `padding.left` / `padding.top`).
        let mut origin = Point {
            x: bounds.origin.x + px(crate::app::constants::PANE_CONTENT_INSET_X),
            y: bounds.origin.y + px(crate::app::constants::PANE_CONTENT_INSET_Y),
        };
        // US-017: snap the origin to physical-pixel boundaries so the grid
        // doesn't shiver between sub-pixel positions while resizing the window
        // or a pane divider on a HiDPI display. Snap the ORIGIN ONLY - never
        // cell_width / line_height (Zed reverted metric-snapping in #54836; it
        // breaks scroll math when rows × snapped_line_height ≠ viewport height).
        // At scale 1.0 this floors the inset-adjusted origin to whole pixels,
        // which is also the right thing (no regression). Mirrors Zed
        // terminal_element.rs:1062-1070 (PR #47195). `.max(1.0)` guards against
        // a 0.0 scale on headless/test windows (would divide by zero).
        let scale_factor = window.scale_factor().max(1.0);
        let snap_px = |v: Pixels| px((f32::from(v) * scale_factor).floor() / scale_factor);
        origin.x = snap_px(origin.x);
        origin.y = snap_px(origin.y);
        // Store the inset-adjusted, SNAPPED origin for mouse → grid coordinate
        // conversion so hit-testing stays coherent with what was painted.
        // Poison-safe: a prior panic inside paint() could have poisoned the
        // Mutex. The inner Point is still a valid value; recover and continue.
        *self
            .element_origin
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = origin;
        let line_height = layout.dimensions.line_height;
        let font_size = self.frame_metrics.font_size;

        let geom = CellGeometry {
            origin,
            cell_width,
            line_height,
        };

        // PANEFLOW_PIXEL_PROBE: log the per-frame origin once, before any
        // glyph/background record carries it implicitly. Pairs with the
        // `cell_dims` record emitted from `resolve_frame_metrics()`.
        #[cfg(debug_assertions)]
        pixel_probe::record_origin(origin);

        let base_font = &self.frame_metrics.base_font;

        // US-047: the shared integer pixel boundary arrays are derived purely
        // from `geom` + the viewport size, so compute them ONCE here and lend
        // them to both background passes instead of each pass rebuilding two
        // `Vec<Pixels>` per frame. Empty viewport → empty slices (both passes
        // early-return before indexing).
        let (cell_x_bounds, cell_y_bounds) = if layout.desired_cols == 0 || layout.desired_rows == 0
        {
            (Vec::new(), Vec::new())
        } else {
            (
                geom.x_boundaries(layout.desired_cols),
                geom.y_boundaries(layout.desired_rows),
            )
        };

        // Clip to element bounds
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            // 1. Terminal background fill
            paint::background::paint_base_fill(&layout, bounds, window);

            // 2. Per-cell background rects with Ghostty-style edge extension.
            paint::background::paint_cell_backgrounds(
                &layout,
                bounds,
                &cell_x_bounds,
                &cell_y_bounds,
                window,
            );

            // 2b. Selection highlight
            paint::selection::paint_selection(&layout, &geom, window);

            // 2c. Search match highlight
            paint::overlay::paint_search_highlights(&layout, &geom, window);

            // 2d. Block element quads (pixel-perfect, no font glyph gaps)
            paint::background::paint_block_quads(&layout, &cell_x_bounds, &cell_y_bounds, window);

            // 2e. Single-stroke box drawing with shared cell-edge endpoints.
            paint::box_drawing::paint_box_drawing_glyphs(
                &layout,
                &cell_x_bounds,
                &cell_y_bounds,
                window,
            );

            // 3. Batched text runs
            paint::text::paint_text_runs(&layout, &geom, base_font, font_size, window, cx);

            // 3a. PANEFLOW_PIXEL_PROBE_OVERLAY: draw thin red cell borders
            // above the text. Independent of `PANEFLOW_PIXEL_PROBE`; opt-in
            // only. Compiled out in release builds.
            #[cfg(debug_assertions)]
            if pixel_probe::overlay_enabled() {
                paint::overlay::paint_pixel_probe_overlay(&layout, &geom, window);
            }

            // 3b. Hyperlink underline (Ctrl+hover)
            paint::overlay::paint_hyperlink_underline(self, &layout, &geom, window);

            // 4. Primary cursor
            paint::cursor::paint_cursor(&layout, &geom, base_font, font_size, window, cx);

            // 4b. Copy-mode / mouse-selection secondary marker
            paint::cursor::paint_anchor_cursor(&layout, &geom, base_font, font_size, window, cx);

            // 5. Scrollbar thumb
            paint::scrollbar::paint_scrollbar(&layout, &geom, bounds, window);

            // 5b. EP-006 US-017: search match rail - decimated ticks on the
            // same strip. Click-to-jump rides the existing proportional
            // track click (US-015 hit-test below); the rail disappears with
            // the search at the same repaint (empty snapshot → no paint).
            paint::scrollbar::paint_match_ticks(
                &self.search_rail_lines,
                crate::theme::ui_colors().vc_modified,
                &layout,
                &geom,
                bounds,
                window,
            );

            // US-015: publish the painted scrollbar geometry so the view's
            // mouse handlers can hit-test click-to-jump / drag against the same
            // strip. Computed even when the thumb is hidden (display_offset==0)
            // so the track stays clickable to scroll back. Poison-safe like
            // `element_origin`.
            let metrics = paint::scrollbar::scrollbar_metrics(
                layout.history_size,
                layout.display_offset,
                &geom,
                bounds,
            );
            *self
                .scrollbar_metrics
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = metrics;

            // 6. IME handler registration + preedit overlay
            let view_for_ime = self.terminal_view.clone();
            paint::overlay::paint_ime_preedit(
                self,
                &layout,
                &geom,
                font_size,
                base_font,
                window,
                cx,
                |cursor_bounds| TerminalInputHandler {
                    terminal_view: view_for_ime,
                    cursor_bounds,
                    alt_screen: self.alt_screen,
                },
            );

            // 7. Exit overlay
            let exit_fg = rgb_to_hsla(0x6c, 0x70, 0x86); // Overlay6
            paint::overlay::paint_exit_overlay(
                &layout, &geom, bounds, font_size, base_font, exit_fg, window, cx,
            );
        });

        #[cfg(debug_assertions)]
        if let Some(paint_start) = _paint_start {
            let paint_elapsed = paint_start.elapsed();
            let paint_ms = paint_elapsed.as_secs_f64() * 1000.0;

            // Phase 2: paint() duration
            if paint_ms > 1.0 {
                log::warn!("[latency] paint: {paint_ms:.2}ms");
            }

            // Phase 3: total keystroke → pixel with per-phase breakdown
            if let Some(keystroke_at) = self.last_keystroke_at {
                let total_elapsed = keystroke_at.elapsed();
                let total_ms = total_elapsed.as_secs_f64() * 1000.0;
                let pty_to_paint_ms = total_ms - paint_ms;
                if total_ms > 8.0 {
                    log::warn!(
                        "[latency] keystroke→pixel: {total_ms:.2}ms \
                         (pty_write→paint_start: {pty_to_paint_ms:.2}ms, \
                         paint: {paint_ms:.2}ms)"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// IntoElement implementation
// ---------------------------------------------------------------------------

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

// ---------------------------------------------------------------------------
// IME InputHandler (US-017)
// ---------------------------------------------------------------------------

struct TerminalInputHandler {
    terminal_view: gpui::Entity<crate::terminal::TerminalView>,
    cursor_bounds: Option<Bounds<Pixels>>,
    alt_screen: bool,
}

fn ime_selected_text_range(alt_screen: bool) -> Option<gpui::UTF16Selection> {
    if alt_screen {
        None
    } else {
        Some(gpui::UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }
}

impl gpui::InputHandler for TerminalInputHandler {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<gpui::UTF16Selection> {
        ime_selected_text_range(self.alt_screen)
    }

    fn marked_text_range(
        &mut self,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<std::ops::Range<usize>> {
        self.terminal_view.read(cx).marked_text_range()
    }

    fn text_for_range(
        &mut self,
        _range_utf16: std::ops::Range<usize>,
        _adjusted_range: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<String> {
        None
    }

    fn replace_text_in_range(
        &mut self,
        _replacement_range: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut App,
    ) {
        // Commit: clear preedit and write text to PTY
        self.terminal_view.update(cx, |view, cx| {
            view.clear_marked_text(cx);
            view.commit_text(text, cx);
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<std::ops::Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        cx: &mut App,
    ) {
        // Preedit: update marked text for rendering
        self.terminal_view.update(cx, |view, cx| {
            view.set_marked_text(new_text.to_string(), cx);
        });
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut App) {
        // Cancel composition
        self.terminal_view.update(cx, |view, cx| {
            view.clear_marked_text(cx);
        });
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: std::ops::Range<usize>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        self.cursor_bounds
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<usize> {
        None
    }
}

#[cfg(test)]
mod ime_input_handler_tests {
    use super::ime_selected_text_range;

    #[test]
    fn ime_selection_is_disabled_in_alt_screen() {
        assert!(ime_selected_text_range(true).is_none());

        let selection = ime_selected_text_range(false).expect("normal screen accepts IME");
        assert_eq!(selection.range, 0..0);
        assert!(!selection.reversed);
    }
}

// ---------------------------------------------------------------------------
// US-005 fallback - block_char_coverages tests
//
// Discovered via pixel-probe analysis of Claude Code 2.1.119's banner robot:
// the `▐███▌` core uses U+2580..U+2590 (already covered) but the antennas /
// rounded corners use quadrant blocks (`U+2596..U+259F`) which originally
// fell back to font glyphs and rendered with visible vertical gaps. These
// tests lock in coverage for every codepoint added in the US-005 fallback
// extension so a future regression surfaces here instead of as a visual
// artifact reported weeks later.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod block_char_coverage_tests {
    use super::*;

    /// Every original-PRD codepoint must still resolve to a single rect with
    /// the same geometry as before the slice refactor - guards against an
    /// accidental table edit during the US-005 extension.
    #[test]
    fn original_block_chars_are_single_rect() {
        for c in [
            '▀', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█', '▉', '▊', '▋', '▌', '▍', '▎', '▏', '▐',
        ] {
            let rects = block_char_coverages(c)
                .unwrap_or_else(|| panic!("U+{:04X} '{c}' must be covered", c as u32));
            assert_eq!(
                rects.len(),
                1,
                "U+{:04X} '{c}' must emit exactly one rect (got {})",
                c as u32,
                rects.len(),
            );
        }
    }

    /// The full block must cover the entire cell - the canonical sanity check
    /// used by adjacent-block tests in `paint/background.rs`.
    #[test]
    fn full_block_covers_entire_cell() {
        let rects = block_char_coverages('█').expect("█ covered");
        assert_eq!(rects, &[(0.0, 0.0, 1.0, 1.0)]);
    }

    #[test]
    fn upper_one_eighth_block_u2594() {
        // ▔ is the upper-edge complement of ▁ (U+2581 lower 1/8). Same height,
        // anchored at y=0 instead of y=7/8.
        let rects = block_char_coverages('▔').expect("▔ covered");
        assert_eq!(rects.len(), 1);
        let (x, y, w, h) = rects[0];
        assert_eq!((x, y), (0.0, 0.0));
        assert_eq!(w, 1.0);
        assert!((h - 1.0 / 8.0).abs() < 1e-6, "expected h=1/8, got {h}");
    }

    #[test]
    fn single_quadrants_are_one_rect_each() {
        // U+2596..U+2598 + U+259D - the four single-quadrant blocks.
        // Each occupies exactly one corner of the cell, anchored on the grid
        // halfway point, with a 0.5×0.5 extent.
        let cases = [
            ('▖', (0.0, 0.5, 0.5, 0.5)), // lower-left
            ('▗', (0.5, 0.5, 0.5, 0.5)), // lower-right
            ('▘', (0.0, 0.0, 0.5, 0.5)), // upper-left
            ('▝', (0.5, 0.0, 0.5, 0.5)), // upper-right
        ];
        for (c, expected) in cases {
            let rects = block_char_coverages(c).unwrap();
            assert_eq!(rects, &[expected], "U+{:04X} '{c}'", c as u32);
        }
    }

    #[test]
    fn multi_quadrants_emit_two_rects() {
        // The six 3-quadrant + 2-quadrant chars all decompose into two rects.
        for c in ['▙', '▚', '▛', '▜', '▞', '▟'] {
            let rects = block_char_coverages(c).unwrap();
            assert_eq!(
                rects.len(),
                2,
                "U+{:04X} '{c}' must emit 2 rects (got {})",
                c as u32,
                rects.len(),
            );
        }
    }

    #[test]
    fn multi_quadrant_diagonals_have_no_overlap_or_gap() {
        // ▚ (U+259A) and ▞ (U+259E) are the two pure diagonals - opposing
        // quadrants only. Their rects must touch at the cell center but not
        // overlap, otherwise we'd double-paint or leave a sub-pixel hole.
        for c in ['▚', '▞'] {
            let rects = block_char_coverages(c).unwrap();
            // Total coverage area = exactly half the cell (two 0.5×0.5 quads).
            let total_area: f32 = rects.iter().map(|(_, _, w, h)| w * h).sum();
            assert!(
                (total_area - 0.5).abs() < 1e-6,
                "U+{:04X} '{c}' total coverage area = {total_area}, expected 0.5",
                c as u32,
            );
        }
    }

    #[test]
    fn three_quadrant_chars_cover_three_quarters_of_cell() {
        // ▙ ▛ ▜ ▟ each cover exactly 3 of 4 quadrants (= 0.75 of cell area).
        // Even though they emit only 2 rects, the second rect is half-cell-wide
        // (covering 2 quadrants in one go).
        for c in ['▙', '▛', '▜', '▟'] {
            let rects = block_char_coverages(c).unwrap();
            let total_area: f32 = rects.iter().map(|(_, _, w, h)| w * h).sum();
            assert!(
                (total_area - 0.75).abs() < 1e-6,
                "U+{:04X} '{c}' total coverage = {total_area}, expected 0.75",
                c as u32,
            );
        }
    }

    /// The US-005 extension targets exactly the codepoints found in the
    /// `claude` 2.1.119 binary that were *not* in the original table.
    /// If Claude Code (or another TUI) ships a new robot that uses a codepoint
    /// outside this list, the gap will reappear and this test won't catch it -
    /// but the pixel probe will, and the table is one match-arm away from
    /// covering the new char.
    #[test]
    fn us005_claude_code_codepoints_all_covered() {
        for c in [
            '▔', // U+2594 upper 1/8
            '▖', '▗', '▘', '▝', // single quadrants
            '▙', '▚', '▛', '▜', '▞', '▟', // multi quadrants
        ] {
            assert!(
                block_char_coverages(c).is_some(),
                "U+{:04X} '{c}' must be covered to render Claude Code's banner gap-free",
                c as u32,
            );
        }
    }

    /// Codepoints we deliberately *don't* cover - shaded blocks need alpha
    /// (out of scope for this fix), geometric shapes are a different path.
    /// Locks the boundary so a future "extend everything" edit can't sneak
    /// half-broken coverage past review.
    #[test]
    fn shaded_and_geometric_blocks_remain_uncovered() {
        for c in ['░', '▒', '▓', '■', '□', '●', '○'] {
            assert!(
                block_char_coverages(c).is_none(),
                "U+{:04X} '{c}' must NOT be covered (alpha or geometric path)",
                c as u32,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// US-002 - Window-free golden-frame net
// ---------------------------------------------------------------------------

/// Deterministic, platform-stable textual rendering of an [`Hsla`]. Fixed
/// precision so the golden bytes never drift on float `Debug` formatting.
#[cfg(test)]
fn hsla_repr(c: Hsla) -> String {
    format!("hsla({:.4},{:.4},{:.4},{:.4})", c.h, c.s, c.l, c.a)
}

#[cfg(test)]
impl LayoutState {
    /// Window-free, deterministic textual snapshot of the entire layout state
    /// for the golden-frame net (US-002). Does NOT rely on any GPUI `Debug`
    /// impl - every field is rendered explicitly at fixed float precision, so
    /// the golden is reproducible across platforms (Rust float formatting is
    /// platform-independent) and human-reviewable on diff. Regenerate goldens
    /// with `PANEFLOW_BLESS_GOLDEN=1 cargo test -p paneflow-app golden_frame`.
    pub(crate) fn golden_repr(&self) -> String {
        use std::fmt::Write as _;
        let mut s = String::new();
        let d = &self.dimensions;
        let _ = writeln!(
            s,
            "dims {:.3}x{:.3} grid {}x{} off={} hist={} exited={:?} signal={:?}",
            d.cell_width.as_f32(),
            d.line_height.as_f32(),
            self.desired_cols,
            self.desired_rows,
            self.display_offset,
            self.history_size,
            self.exited,
            self.exit_signal,
        );
        let _ = writeln!(
            s,
            "bg={} thumb={} link={}",
            hsla_repr(self.background_color),
            hsla_repr(self.scrollbar_thumb),
            hsla_repr(self.link_text_color),
        );
        let _ = writeln!(s, "runs[{}]:", self.batched_runs.len());
        for r in &self.batched_runs {
            let bold = r.font.weight == FontWeight::BOLD;
            let italic = r.font.style == FontStyle::Italic;
            let style = match (bold, italic) {
                (true, true) => "bold-italic",
                (true, false) => "bold",
                (false, true) => "italic",
                (false, false) => "normal",
            };
            let _ = writeln!(
                s,
                "  L{} C{} {:?} fg={} {} ul={} st={}",
                r.line,
                r.col_start,
                r.text,
                hsla_repr(r.color),
                style,
                r.underline.is_some(),
                r.strikethrough.is_some(),
            );
        }
        let rect_line = |s: &mut String, label: &str, rects: &[LayoutRect]| {
            use std::fmt::Write as _;
            let _ = writeln!(s, "{label}[{}]:", rects.len());
            for r in rects {
                let _ = writeln!(
                    s,
                    "  L{}+{}ln C{}+{}c {}",
                    r.line,
                    r.num_lines,
                    r.col,
                    r.num_cols,
                    hsla_repr(r.color),
                );
            }
        };
        rect_line(&mut s, "rects", &self.rects);
        let _ = writeln!(s, "blocks[{}]:", self.block_quads.len());
        for q in &self.block_quads {
            let _ = writeln!(
                s,
                "  L{} C{}+{}c cov=({:.3},{:.3},{:.3},{:.3}) {}",
                q.line,
                q.col,
                q.num_cols,
                q.coverage.0,
                q.coverage.1,
                q.coverage.2,
                q.coverage.3,
                hsla_repr(q.color),
            );
        }
        rect_line(&mut s, "selection_rects", &self.selection_rects);
        rect_line(&mut s, "search_rects", &self.search_rects);
        let cur_repr = |c: &Option<CursorInfo>| -> String {
            match c {
                None => "None".to_string(),
                Some(c) => format!(
                    "L{} C{} {:?} {} wide={} text={:?} bold={} italic={}",
                    c.line,
                    c.col,
                    c.shape,
                    hsla_repr(c.color),
                    c.wide,
                    c.text,
                    c.bold,
                    c.italic,
                ),
            }
        };
        let _ = writeln!(s, "cursor: {}", cur_repr(&self.cursor));
        let _ = writeln!(s, "anchor: {}", cur_repr(&self.anchor_cursor));
        match &self.ime_cursor_bounds {
            None => {
                let _ = writeln!(s, "ime: None");
            }
            Some(b) => {
                let _ = writeln!(
                    s,
                    "ime: x={:.3} y={:.3} w={:.3} h={:.3}",
                    b.origin.x.as_f32(),
                    b.origin.y.as_f32(),
                    b.size.width.as_f32(),
                    b.size.height.as_f32(),
                );
            }
        }
        s
    }
}

#[cfg(test)]
mod golden_frame_tests {
    //! US-002 golden-frame net: deterministic `LayoutState` snapshots over a
    //! fixed grid, run with **no `Window`/`App`/GPU/display**. The fact these
    //! tests construct `LayoutInputs` and call `layout_from_snapshot` directly
    //! never touching a GPUI context - is the Window-free proof (AC-1). Each
    //! fixture asserts against a committed golden under `golden/` (AC-2);
    //! regenerate with `PANEFLOW_BLESS_GOLDEN=1` (AC-3).
    use super::*;
    use crate::terminal::types::{RenderableCursor, Rgb};

    const COLS: usize = 12;
    const ROWS: usize = 4;

    fn test_dims() -> CellDimensions {
        CellDimensions {
            cell_width: px(8.0),
            line_height: px(16.0),
        }
    }

    fn test_font() -> Font {
        Font {
            family: "test-mono".into(),
            features: gpui::FontFeatures::default(),
            fallbacks: None,
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        }
    }

    fn default_fg() -> Color {
        Color::Named(NamedColor::Foreground)
    }
    fn default_bg() -> Color {
        Color::Named(NamedColor::Background)
    }

    fn cell(line: i32, col: usize, c: char, fg: Color, bg: Color, flags: CellFlags) -> Cell {
        Cell {
            point: GridPoint::new(line, col),
            c,
            fg,
            bg,
            flags,
            zerowidth: None,
            hyperlink: false,
        }
    }

    fn text_row(line: i32, text: &str, fg: Color, flags: CellFlags) -> Vec<Cell> {
        text.chars()
            .enumerate()
            .map(|(i, c)| cell(line, i, c, fg, default_bg(), flags))
            .collect()
    }

    fn white() -> Hsla {
        Hsla {
            h: 0.0,
            s: 0.0,
            l: 1.0,
            a: 1.0,
        }
    }

    fn cursor_at(col: usize, shape: CursorShape, text: Option<char>) -> CursorInfo {
        cursor_at_line(0, col, shape, text)
    }

    fn cursor_at_line(line: i32, col: usize, shape: CursorShape, text: Option<char>) -> CursorInfo {
        CursorInfo {
            line,
            col,
            shape,
            color: white(),
            cell_bg: crate::theme::one_dark().ansi_background,
            wide: false,
            text,
            bold: false,
            italic: false,
        }
    }

    fn renderable_cursor_at(col: usize, shape: CursorShape, text: char) -> RenderableCursor {
        RenderableCursor {
            point: GridPoint::new(0, col),
            shape,
            fg: default_fg(),
            bg: default_bg(),
            flags: CellFlags::empty(),
            wide: false,
            text,
            bold: false,
            italic: false,
        }
    }

    /// Build a `LayoutState` over the fixed test grid. Each call uses a fixed
    /// theme, font, and dimensions so the output is fully deterministic.
    fn run(
        cells: Vec<Cell>,
        cursor: Option<CursorInfo>,
        selection: Option<SelectionRange>,
    ) -> LayoutState {
        run_with_integrated_glyphs(cells, cursor, selection, true)
    }

    fn run_with_integrated_glyphs(
        cells: Vec<Cell>,
        cursor: Option<CursorInfo>,
        selection: Option<SelectionRange>,
        integrated_glyphs_enabled: bool,
    ) -> LayoutState {
        let theme = crate::theme::one_dark();
        layout_from_snapshot(LayoutInputs {
            cells: cells.into(),
            cursor,
            selection_range: selection,
            copy_mode_cursor: None,
            search_highlights: &[],
            display_offset: 0,
            history_size: 0,
            desired_cols: COLS,
            desired_rows: ROWS,
            first_visible_row: 0,
            last_visible_row: ROWS as i32,
            dims: test_dims(),
            base_font: test_font(),
            theme: &theme,
            exited: None,
            exit_signal: None,
            integrated_glyphs_enabled,
            color_emoji_enabled: true,
        })
    }

    fn run_selection_with_visible(
        selection: SelectionRange,
        first_visible_row: i32,
        last_visible_row: i32,
    ) -> LayoutState {
        let theme = crate::theme::one_dark();
        layout_from_snapshot(LayoutInputs {
            cells: Vec::new().into(),
            cursor: None,
            selection_range: Some(selection),
            copy_mode_cursor: None,
            search_highlights: &[],
            display_offset: 0,
            history_size: 0,
            desired_cols: COLS,
            desired_rows: ROWS,
            first_visible_row,
            last_visible_row,
            dims: test_dims(),
            base_font: test_font(),
            theme: &theme,
            exited: None,
            exit_signal: None,
            integrated_glyphs_enabled: true,
            color_emoji_enabled: true,
        })
    }

    fn golden_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/terminal/element/golden")
    }

    fn assert_golden(name: &str, state: &LayoutState) {
        assert_golden_text(name, state.golden_repr());
    }

    fn assert_golden_text(name: &str, actual: String) {
        let path = golden_dir().join(format!("{name}.txt"));
        if std::env::var_os("PANEFLOW_BLESS_GOLDEN").is_some() {
            std::fs::create_dir_all(golden_dir()).unwrap();
            std::fs::write(&path, actual.as_bytes()).unwrap();
            return;
        }
        let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "golden '{name}' missing ({e}); regenerate with \
                 PANEFLOW_BLESS_GOLDEN=1 cargo test -p paneflow-app golden_frame"
            )
        });
        let expected = normalize_golden_line_endings(&expected);
        assert_eq!(
            actual, expected,
            "golden '{name}' drifted; if intentional, regenerate with \
             PANEFLOW_BLESS_GOLDEN=1 cargo test -p paneflow-app golden_frame"
        );
    }

    fn normalize_golden_line_endings(text: &str) -> String {
        text.replace("\r\n", "\n")
    }

    #[test]
    fn golden_line_endings_are_checkout_agnostic() {
        assert_eq!(
            normalize_golden_line_endings("runs[1]:\r\n  L0 C0\r\n"),
            "runs[1]:\n  L0 C0\n"
        );
    }

    /// The full fixture corpus. One test so a `BLESS` run regenerates every
    /// golden in a single pass; each fixture still asserts independently.
    #[test]
    fn golden_frame_corpus() {
        // plain ASCII
        assert_golden(
            "plain",
            &run(
                text_row(0, "hi", default_fg(), CellFlags::empty()),
                None,
                None,
            ),
        );

        // ANSI-16 named colors
        let ansi16 = vec![
            cell(
                0,
                0,
                'R',
                Color::Named(NamedColor::Red),
                default_bg(),
                CellFlags::empty(),
            ),
            cell(
                0,
                1,
                'G',
                Color::Named(NamedColor::Green),
                default_bg(),
                CellFlags::empty(),
            ),
            cell(
                0,
                2,
                'B',
                Color::Named(NamedColor::Blue),
                default_bg(),
                CellFlags::empty(),
            ),
        ];
        assert_golden("ansi16", &run(ansi16, None, None));

        // DIM (SGR 2): foreground alpha reduced
        assert_golden(
            "dim",
            &run(text_row(0, "dim", default_fg(), CellFlags::DIM), None, None),
        );

        // INVERSE: fg/bg swapped on the raw ANSI tags
        let inverse = vec![cell(
            0,
            0,
            'x',
            Color::Named(NamedColor::Red),
            Color::Named(NamedColor::Blue),
            CellFlags::INVERSE,
        )];
        assert_golden("inverse", &run(inverse, None, None));

        // 256-color indexed (cube + grayscale): app-chosen, contrast-skipped
        let indexed = vec![
            cell(
                0,
                0,
                'a',
                Color::Indexed(33),
                default_bg(),
                CellFlags::empty(),
            ),
            cell(
                0,
                1,
                'b',
                Color::Indexed(201),
                default_bg(),
                CellFlags::empty(),
            ),
            cell(
                0,
                2,
                'g',
                Color::Indexed(240),
                default_bg(),
                CellFlags::empty(),
            ),
        ];
        assert_golden("indexed256", &run(indexed, None, None));

        // truecolor (SGR 38;2): exact RGB, contrast-skipped
        let truecolor = vec![cell(
            0,
            0,
            't',
            Color::Spec(Rgb {
                r: 200,
                g: 100,
                b: 50,
            }),
            default_bg(),
            CellFlags::empty(),
        )];
        assert_golden("truecolor", &run(truecolor, None, None));

        // block / half-block chars → BlockQuads, not glyph runs
        let blocks: Vec<Cell> = "█▀▄▌▙"
            .chars()
            .enumerate()
            .map(|(i, c)| cell(0, i, c, default_fg(), default_bg(), CellFlags::empty()))
            .collect();
        assert_golden("blocks", &run(blocks, None, None));

        // CJK wide char + its trailing spacer (spacer must be skipped)
        let cjk = vec![
            cell(0, 0, '中', default_fg(), default_bg(), CellFlags::WIDE_CHAR),
            cell(
                0,
                1,
                ' ',
                default_fg(),
                default_bg(),
                CellFlags::WIDE_CHAR_SPACER,
            ),
        ];
        assert_golden("cjk_spacer", &run(cjk, None, None));

        // selection: linear single-line range over columns 1..=3
        let sel = SelectionRange {
            start: GridPoint::new(0, 1),
            end: GridPoint::new(0, 3),
            is_block: false,
        };
        assert_golden(
            "selection",
            &run(
                text_row(0, "selected", default_fg(), CellFlags::empty()),
                None,
                Some(sel),
            ),
        );

        // each cursor shape
        let base = || text_row(0, "ab", default_fg(), CellFlags::empty());
        assert_golden(
            "cursor_block",
            &run(
                base(),
                Some(cursor_at(0, CursorShape::Block, Some('a'))),
                None,
            ),
        );
        assert_golden(
            "cursor_underline",
            &run(
                base(),
                Some(cursor_at(0, CursorShape::Underline, None)),
                None,
            ),
        );
        assert_golden(
            "cursor_beam",
            &run(base(), Some(cursor_at(0, CursorShape::Beam, None)), None),
        );
        assert_golden(
            "cursor_hollow",
            &run(
                base(),
                Some(cursor_at(0, CursorShape::HollowBlock, None)),
                None,
            ),
        );
        assert_golden("cursor_hidden", &run(base(), None, None));

        // APCA contrast: index-0..15 fg close to a dark bg gets bumped
        let apca = vec![cell(
            0,
            0,
            'z',
            Color::Named(NamedColor::Black),
            default_bg(),
            CellFlags::empty(),
        )];
        assert_golden("apca_contrast", &run(apca, None, None));
    }

    /// Structural invariant (AC-2/AC-4 of the spike risk): block-element cells
    /// emit `BlockQuad`s and no glyph runs, and multi-quadrant chars emit two
    /// quads each. Asserted independently of the golden text so a regression
    /// here is legible even if a golden is re-blessed.
    #[test]
    fn block_chars_emit_quads_not_runs() {
        let blocks: Vec<Cell> = "█▀▄▌▙"
            .chars()
            .enumerate()
            .map(|(i, c)| cell(0, i, c, default_fg(), default_bg(), CellFlags::empty()))
            .collect();
        let state = run(blocks, None, None);
        // █ ▀ ▄ ▌ = 1 quad each, ▙ = 2 quads → 6 total
        assert_eq!(
            state.block_quads.len(),
            6,
            "block chars should map to filled quads"
        );
        assert!(
            state.batched_runs.is_empty(),
            "block chars must not produce glyph text runs"
        );
    }

    #[test]
    fn block_chars_use_font_glyphs_when_integrated_glyphs_are_disabled() {
        let blocks: Vec<Cell> = "█▀▄▌▙"
            .chars()
            .enumerate()
            .map(|(i, c)| cell(0, i, c, default_fg(), default_bg(), CellFlags::empty()))
            .collect();
        let state = run_with_integrated_glyphs(blocks, None, None, false);

        assert!(
            state.block_quads.is_empty(),
            "integrated glyphs off must not emit block quads"
        );
        assert_eq!(
            state.batched_runs.len(),
            1,
            "block chars should fall back to one normal glyph run"
        );
    }

    #[test]
    fn codex_box_drawing_chars_emit_paths_not_text_runs() {
        let boxes: Vec<Cell> = "╭──╮│┌┼┐╰──╯"
            .chars()
            .enumerate()
            .map(|(i, c)| cell(0, i, c, default_fg(), default_bg(), CellFlags::empty()))
            .collect();
        let state = run(boxes, None, None);

        assert_eq!(state.box_drawing_glyphs.len(), 12);
        assert!(
            state.batched_runs.is_empty(),
            "integrated box drawing must not use font glyphs"
        );
    }

    #[test]
    fn box_drawing_uses_font_glyphs_when_integrated_glyphs_are_disabled() {
        let boxes: Vec<Cell> = "╭─╮│╰─╯"
            .chars()
            .enumerate()
            .map(|(i, c)| cell(0, i, c, default_fg(), default_bg(), CellFlags::empty()))
            .collect();
        let state = run_with_integrated_glyphs(boxes, None, None, false);

        assert!(state.box_drawing_glyphs.is_empty());
        assert_eq!(state.batched_runs.len(), 1);
    }

    #[test]
    fn shell_cursor_is_hidden_when_scrolled_away_from_live_edge() {
        let theme = crate::theme::one_dark();
        let state = layout_from_snapshot(LayoutInputs {
            cells: text_row(0, "history", default_fg(), CellFlags::empty()).into(),
            cursor: Some(cursor_at_line(3, 0, CursorShape::Block, None)),
            selection_range: None,
            copy_mode_cursor: None,
            search_highlights: &[],
            display_offset: 2,
            history_size: 10,
            desired_cols: COLS,
            desired_rows: ROWS,
            first_visible_row: 0,
            last_visible_row: ROWS as i32,
            dims: test_dims(),
            base_font: test_font(),
            theme: &theme,
            exited: None,
            exit_signal: None,
            integrated_glyphs_enabled: true,
            color_emoji_enabled: true,
        });

        assert!(
            state.cursor.is_none(),
            "live cursor must not float over scrollback"
        );
        assert!(
            state.ime_cursor_bounds.is_none(),
            "IME bounds should disappear with the hidden live cursor"
        );
    }

    #[test]
    fn unfocused_terminal_hides_live_cursor() {
        let cursor = renderable_cursor_at(0, CursorShape::Block, 'a');
        let theme = crate::theme::one_dark();

        assert!(
            cursor_from_content(cursor, true, true, white(), CursorShape::Block, &theme,).is_some(),
            "focused terminals should keep the live cursor"
        );
        assert!(
            cursor_from_content(cursor, true, false, white(), CursorShape::Block, &theme,)
                .is_none(),
            "unfocused terminals must not paint a hollow cursor outline"
        );
    }

    #[test]
    fn configured_custom_cursor_shapes_override_native_fallbacks() {
        let block_cursor = renderable_cursor_at(0, CursorShape::Block, 'a');
        let theme = crate::theme::one_dark();
        let vintage = cursor_from_content(
            block_cursor,
            true,
            true,
            white(),
            CursorShape::Vintage,
            &theme,
        )
        .unwrap();
        assert_eq!(vintage.shape, CursorShape::Vintage);
        assert!(
            vintage.text.is_none(),
            "vintage cursor should not use block inverse text"
        );

        let underline_cursor = renderable_cursor_at(0, CursorShape::Underline, 'a');
        let double = cursor_from_content(
            underline_cursor,
            true,
            true,
            white(),
            CursorShape::DoubleUnderline,
            &theme,
        )
        .unwrap();
        assert_eq!(double.shape, CursorShape::DoubleUnderline);
    }

    #[test]
    fn block_cursor_carries_cell_background_for_inverse_text() {
        let theme = crate::theme::one_dark();
        let explicit_bg = Color::Spec(Rgb {
            r: 12,
            g: 34,
            b: 56,
        });
        let mut cursor = renderable_cursor_at(0, CursorShape::Block, 'x');
        cursor.bg = explicit_bg;

        let info = cursor_from_content(cursor, true, true, white(), CursorShape::Block, &theme)
            .expect("cursor visible");
        assert_eq!(info.cell_bg, rgb_to_hsla(12, 34, 56));

        let mut inverse = renderable_cursor_at(0, CursorShape::Block, 'x');
        inverse.fg = Color::Spec(Rgb { r: 90, g: 8, b: 7 });
        inverse.flags = CellFlags::INVERSE;
        let info = cursor_from_content(inverse, true, true, white(), CursorShape::Block, &theme)
            .expect("cursor visible");
        assert_eq!(info.cell_bg, rgb_to_hsla(90, 8, 7));

        let transparent = renderable_cursor_at(0, CursorShape::Block, 'x');
        let info =
            cursor_from_content(transparent, true, true, white(), CursorShape::Block, &theme)
                .expect("cursor visible");
        assert_eq!(info.cell_bg.a, 0.0);
    }

    #[test]
    fn unfocused_terminal_hides_copy_mode_cursor() {
        let copy_cursor = CopyModeCursorState {
            grid_line: 0,
            col: 1,
            anchor_grid_line: Some(0),
            anchor_col: 0,
        };

        assert!(
            focused_copy_mode_cursor(Some(&copy_cursor), true).is_some(),
            "focused terminals should keep copy-mode cursor markers"
        );
        assert!(
            focused_copy_mode_cursor(Some(&copy_cursor), false).is_none(),
            "unfocused terminals should not paint copy-mode cursor markers"
        );
    }

    #[test]
    fn mouse_selection_renders_without_endpoint_markers() {
        let selection = SelectionRange {
            start: GridPoint::new(0, 1),
            end: GridPoint::new(0, 3),
            is_block: false,
        };
        let state = run(
            text_row(
                0,
                "abcdef",
                default_fg(),
                CellFlags::BOLD | CellFlags::ITALIC,
            ),
            None,
            Some(selection),
        );

        assert!(
            !state.selection_rects.is_empty(),
            "mouse selection should still paint the highlight"
        );
        assert!(
            state.cursor.is_none(),
            "mouse selection should hide the terminal cursor while dragging"
        );
        assert!(
            state.anchor_cursor.is_none(),
            "mouse selection should not paint a start/end marker"
        );
    }

    /// Structural invariant: a WIDE_CHAR_SPACER cell never contributes its own
    /// run or rect - it is the trailing half of the preceding wide glyph.
    #[test]
    fn wide_char_spacer_is_skipped() {
        let cjk = vec![
            cell(0, 0, '中', default_fg(), default_bg(), CellFlags::WIDE_CHAR),
            cell(
                0,
                1,
                ' ',
                default_fg(),
                default_bg(),
                CellFlags::WIDE_CHAR_SPACER,
            ),
        ];
        let state = run(cjk, None, None);
        assert_eq!(
            state.batched_runs.len(),
            1,
            "only the wide glyph produces a run"
        );
        assert_eq!(state.batched_runs[0].text, "中");
    }

    /// Viewport culling: rows outside `[first_visible_row, last_visible_row)`
    /// are dropped from the layout (mirrors the content-mask cull in
    /// `build_layout`). Window-free - the cull range is just two integers.
    #[test]
    fn viewport_cull_drops_offscreen_rows() {
        let theme = crate::theme::one_dark();
        let cells = vec![
            cell(0, 0, 'a', default_fg(), default_bg(), CellFlags::empty()),
            cell(2, 0, 'b', default_fg(), default_bg(), CellFlags::empty()),
        ];
        let state = layout_from_snapshot(LayoutInputs {
            cells: cells.into(),
            cursor: None,
            selection_range: None,
            copy_mode_cursor: None,
            search_highlights: &[],
            display_offset: 0,
            history_size: 0,
            desired_cols: COLS,
            desired_rows: ROWS,
            first_visible_row: 0,
            last_visible_row: 1, // only row 0 visible
            dims: test_dims(),
            base_font: test_font(),
            theme: &theme,
            exited: None,
            exit_signal: None,
            integrated_glyphs_enabled: true,
            color_emoji_enabled: true,
        });
        assert_eq!(state.batched_runs.len(), 1, "row 2 is culled");
        assert_eq!(state.batched_runs[0].text, "a");
    }

    #[test]
    fn selection_rects_are_culled_to_visible_rows() {
        let selection = SelectionRange {
            start: GridPoint::new(0, 0),
            end: GridPoint::new(5, 2),
            is_block: false,
        };
        let state = run_selection_with_visible(selection, 2, 4);

        let lines: Vec<i32> = state.selection_rects.iter().map(|rect| rect.line).collect();
        assert_eq!(lines, vec![2, 3]);
    }

    #[test]
    fn reversed_linear_selection_rects_are_normalized() {
        let selection = SelectionRange {
            start: GridPoint::new(2, 3),
            end: GridPoint::new(0, 1),
            is_block: false,
        };
        let state = run_selection_with_visible(selection, 0, ROWS as i32);

        assert_eq!(state.selection_rects.len(), 3);
        assert_eq!(state.selection_rects[0].line, 0);
        assert_eq!(state.selection_rects[0].col, 1);
        assert_eq!(state.selection_rects[1].line, 1);
        assert_eq!(state.selection_rects[1].col, 0);
        assert_eq!(state.selection_rects[1].num_cols, COLS);
        assert_eq!(state.selection_rects[2].line, 2);
        assert_eq!(state.selection_rects[2].col, 0);
        assert_eq!(state.selection_rects[2].num_cols, 4);
    }

    #[test]
    fn terminal_material_makes_default_backgrounds_transparent_only() {
        let theme = crate::theme::one_dark();
        let cells = vec![
            cell(0, 0, 'a', default_fg(), default_bg(), CellFlags::empty()),
            cell(
                0,
                1,
                'b',
                default_fg(),
                Color::Named(NamedColor::Blue),
                CellFlags::empty(),
            ),
        ];
        let state = layout_from_snapshot(LayoutInputs {
            cells: cells.into(),
            cursor: None,
            selection_range: None,
            copy_mode_cursor: None,
            search_highlights: &[],
            display_offset: 0,
            history_size: 0,
            desired_cols: COLS,
            desired_rows: ROWS,
            first_visible_row: 0,
            last_visible_row: ROWS as i32,
            dims: test_dims(),
            base_font: test_font(),
            theme: &theme,
            exited: None,
            exit_signal: None,
            integrated_glyphs_enabled: true,
            color_emoji_enabled: true,
        });

        assert_eq!(state.background_color.a, 0.0);
        assert!(
            state.rects.iter().any(|rect| rect.color.a == 0.0),
            "default background cells should be transparent"
        );
        assert!(
            state.rects.iter().any(|rect| rect.color.a > 0.0),
            "explicit ANSI backgrounds must remain painted"
        );
    }

    #[test]
    fn terminal_panel_grays_match_sidebar_card_surface() {
        let theme = crate::theme::one_dark();
        let card_bg = codex_panel_background_for_terminal(&theme);
        assert_ne!(
            card_bg,
            crate::theme::ui_colors_with(&theme).subtle,
            "dark terminal panels should not collapse back to Codex's #2a2a2a input fill"
        );
        let cells = vec![
            cell(
                0,
                0,
                'a',
                default_fg(),
                Color::Named(NamedColor::BrightBlack),
                CellFlags::empty(),
            ),
            cell(
                0,
                1,
                'b',
                default_fg(),
                Color::Indexed(236),
                CellFlags::empty(),
            ),
            cell(
                0,
                2,
                'c',
                default_fg(),
                Color::Spec(Rgb {
                    r: 42,
                    g: 42,
                    b: 42,
                }),
                CellFlags::empty(),
            ),
            cell(
                0,
                3,
                'd',
                default_fg(),
                Color::Spec(Rgb {
                    r: 48,
                    g: 48,
                    b: 48,
                }),
                CellFlags::empty(),
            ),
        ];
        let state = layout_from_snapshot(LayoutInputs {
            cells: cells.into(),
            cursor: None,
            selection_range: None,
            copy_mode_cursor: None,
            search_highlights: &[],
            display_offset: 0,
            history_size: 0,
            desired_cols: COLS,
            desired_rows: ROWS,
            first_visible_row: 0,
            last_visible_row: ROWS as i32,
            dims: test_dims(),
            base_font: test_font(),
            theme: &theme,
            exited: None,
            exit_signal: None,
            integrated_glyphs_enabled: true,
            color_emoji_enabled: true,
        });

        assert!(
            state.rects.iter().all(|rect| rect.color == card_bg),
            "neutral panel backgrounds should align with the Codex panel color"
        );
    }
}
