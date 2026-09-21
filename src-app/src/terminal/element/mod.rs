use std::sync::{Arc, Mutex};

use gpui::{
    App, Bounds, ContentMask, DispatchPhase, Element, ElementId, Font, FontStyle, FontWeight,
    GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId, MouseButton, MouseMoveEvent,
    Pixels, Point, SharedString, Style, Window, px, relative,
};

use crate::terminal::TerminalSessionBackend;
use crate::terminal::types::{
    Cell, CellFlags, Color, Content, CopyModeCursorState, CursorShape, NamedColor,
    Point as GridPoint, RenderableCursor, SearchHighlight, SelectionRange, TerminalWindowSize,
    terminal_metric_to_u16,
};

pub(super) mod color;
mod face_tables;
mod font;
mod geometry;
mod hyperlink;
mod oklab;
mod paint;
#[cfg(debug_assertions)]
pub(super) mod pixel_probe;
mod sprites;

use crate::theme::ThemePalette;
use color::{HarmonyTargets, convert_color, is_same_visible_color, rgb_to_hsla};
#[cfg(test)]
pub(crate) use font::base_font;
pub use font::{
    CellMetrics, MAX_FONT_SIZE, MIN_FONT_SIZE, global_font_size, resolve_font_family,
    resolve_frame_metrics, sanitize_font_override,
};
pub(crate) use font::{
    DEFAULT_CELL_WIDTH, DEFAULT_FONT_SIZE, DEFAULT_LINE_HEIGHT, apply_font_config,
    normalize_font_weight_key,
};
use geometry::CellGeometry;
pub use hyperlink::{
    detect_code_paths_on_line_mapped, detect_file_paths_on_line_mapped, detect_urls_on_line_mapped,
    is_url_scheme_openable,
};
use sprites::{Sprite, is_private_use, sprite_for};

use crate::terminal::scrollbar_reveal::ScrollbarPresence;
#[allow(unused_imports)]
pub(crate) use color::apca_contrast;
pub(crate) use color::ensure_minimum_contrast;
pub(crate) use paint::scrollbar::ScrollbarMetrics;

pub(crate) const SELECTION_MIN_APCA_CONTRAST: f32 = 45.0;

fn is_decorative_character(ch: char) -> bool {
    matches!(
        ch as u32,
        0x2500..=0x257F
        | 0x2580..=0x259F
        | 0x25A0..=0x25FF
        | 0x2800..=0x28FF
        | 0xE0B0..=0xE0D7
        | 0x1CC00..=0x1CEBF
        | 0x1FB00..=0x1FBFF
    )
}

fn is_correctable_source(color: Color) -> bool {
    matches!(color, Color::Spec(_) | Color::Indexed(16..=255))
}

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

fn merge_background_regions(mut rects: Vec<LayoutRect>) -> Vec<LayoutRect> {
    if rects.len() <= 1 {
        return rects;
    }
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

#[derive(Clone, Copy)]
pub(super) struct RenderColors<'a> {
    pub theme: &'a crate::theme::TerminalTheme,
    pub palette: &'a ThemePalette,
}

fn resolved_cell_background(
    cell_fg: Color,
    cell_bg: Color,
    flags: CellFlags,
    colors: RenderColors<'_>,
) -> Hsla {
    let raw_bg = if flags.contains(CellFlags::INVERSE) {
        cell_fg
    } else {
        cell_bg
    };

    if matches!(raw_bg, Color::Named(NamedColor::Background)) {
        gpui::transparent_black()
    } else {
        terminal_panel_background(
            raw_bg,
            convert_color(raw_bg, colors.theme, colors.palette),
            colors.theme,
        )
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

#[derive(Clone, Copy, PartialEq)]
pub struct CellDimensions {
    pub cell_width: Pixels,
    pub line_height: Pixels,
}

#[derive(Clone)]
pub struct TerminalFrameMetrics {
    pub dimensions: CellDimensions,
    pub base_font: Font,
    pub font_size: Pixels,
    pub metrics: CellMetrics,
}

struct BatchedTextRun {
    text: SharedString,
    font: Font,
    color: Hsla,
    line: i32,
    col_start: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UnderlineKind {
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DecorationKind {
    Underline(UnderlineKind),
    Strikethrough,
}

pub(super) struct Decoration {
    line: i32,
    col_start: usize,
    num_cols: usize,
    kind: DecorationKind,
    color: Hsla,
}

pub(super) struct SymbolGlyph {
    line: i32,
    col: usize,
    span: usize,
    color: Hsla,
    ch: char,
    font: Font,
}

#[derive(Clone, Copy)]
struct LayoutRect {
    line: i32,
    num_lines: usize,
    col: usize,
    num_cols: usize,
    color: Hsla,
}

struct BlockQuad {
    line: i32,
    col: usize,
    num_cols: usize,
    color: Hsla,
    coverage: (f32, f32, f32, f32),
}

struct SpriteGlyph {
    line: i32,
    col: usize,
    num_cols: usize,
    color: Hsla,
    sprite: Sprite,
}

fn block_char_coverages(c: char) -> Option<&'static [(f32, f32, f32, f32)]> {
    match c {
        '▀' => Some(&[(0.0, 0.0, 1.0, 0.5)]),
        '▁' => Some(&[(0.0, 7.0 / 8.0, 1.0, 1.0 / 8.0)]),
        '▂' => Some(&[(0.0, 6.0 / 8.0, 1.0, 2.0 / 8.0)]),
        '▃' => Some(&[(0.0, 5.0 / 8.0, 1.0, 3.0 / 8.0)]),
        '▄' => Some(&[(0.0, 0.5, 1.0, 0.5)]),
        '▅' => Some(&[(0.0, 3.0 / 8.0, 1.0, 5.0 / 8.0)]),
        '▆' => Some(&[(0.0, 2.0 / 8.0, 1.0, 6.0 / 8.0)]),
        '▇' => Some(&[(0.0, 1.0 / 8.0, 1.0, 7.0 / 8.0)]),
        '█' => Some(&[(0.0, 0.0, 1.0, 1.0)]),
        '▉' => Some(&[(0.0, 0.0, 7.0 / 8.0, 1.0)]),
        '▊' => Some(&[(0.0, 0.0, 6.0 / 8.0, 1.0)]),
        '▋' => Some(&[(0.0, 0.0, 5.0 / 8.0, 1.0)]),
        '▌' => Some(&[(0.0, 0.0, 0.5, 1.0)]),
        '▍' => Some(&[(0.0, 0.0, 3.0 / 8.0, 1.0)]),
        '▎' => Some(&[(0.0, 0.0, 2.0 / 8.0, 1.0)]),
        '▏' => Some(&[(0.0, 0.0, 1.0 / 8.0, 1.0)]),
        '▐' => Some(&[(0.5, 0.0, 0.5, 1.0)]),
        '▕' => Some(&[(7.0 / 8.0, 0.0, 1.0 / 8.0, 1.0)]),

        '▔' => Some(&[(0.0, 0.0, 1.0, 1.0 / 8.0)]),

        '▖' => Some(&[(0.0, 0.5, 0.5, 0.5)]),
        '▗' => Some(&[(0.5, 0.5, 0.5, 0.5)]),
        '▘' => Some(&[(0.0, 0.0, 0.5, 0.5)]),
        '▝' => Some(&[(0.5, 0.0, 0.5, 0.5)]),

        '▙' => Some(&[(0.0, 0.0, 0.5, 0.5), (0.0, 0.5, 1.0, 0.5)]),
        '▚' => Some(&[(0.0, 0.0, 0.5, 0.5), (0.5, 0.5, 0.5, 0.5)]),
        '▛' => Some(&[(0.0, 0.0, 1.0, 0.5), (0.0, 0.5, 0.5, 0.5)]),
        '▜' => Some(&[(0.0, 0.0, 1.0, 0.5), (0.5, 0.5, 0.5, 0.5)]),
        '▞' => Some(&[(0.5, 0.0, 0.5, 0.5), (0.0, 0.5, 0.5, 0.5)]),
        '▟' => Some(&[(0.5, 0.0, 0.5, 0.5), (0.0, 0.5, 1.0, 0.5)]),
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
    text: Option<char>,
    bold: bool,
    italic: bool,
}

#[derive(Clone, Copy)]
struct CursorCellContext<'a> {
    desired_cols: usize,
    desired_rows: usize,
    colors: RenderColors<'a>,
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
                resolved_cell_background(cell.fg, cell.bg, cell.flags, ctx.colors),
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
                ctx.colors,
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
    focused: bool,
    cursor_color: Hsla,
    default_cursor_shape: CursorShape,
    colors: RenderColors<'_>,
) -> Option<CursorInfo> {
    if matches!(cursor.shape, CursorShape::Hidden) || !focused {
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
        cell_bg: resolved_cell_background(cursor.fg, cursor.bg, cursor.flags, colors),
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

pub(crate) struct LayoutInputs<'a> {
    pub cells: Arc<[Cell]>,
    pub cursor: Option<CursorInfo>,
    pub selection_range: Option<SelectionRange>,
    pub copy_mode_cursor: Option<&'a CopyModeCursorState>,
    pub search_highlights: &'a [SearchHighlight],
    pub display_offset: usize,
    pub history_size: usize,
    pub desired_cols: usize,
    pub desired_rows: usize,
    pub first_visible_row: i32,
    pub last_visible_row: i32,
    pub dims: CellDimensions,
    pub base_font: Font,
    pub theme: &'a crate::theme::TerminalTheme,
    pub palette: &'a ThemePalette,
    pub exited: Option<i32>,
    pub exit_signal: Option<String>,
    pub integrated_glyphs_enabled: bool,
    pub color_emoji_enabled: bool,
    pub minimum_contrast: f32,
}

struct RowLayout {
    batched_runs: Vec<BatchedTextRun>,
    decorations: Vec<Decoration>,
    symbols: Vec<SymbolGlyph>,
    rects: Vec<LayoutRect>,
    block_quads: Vec<BlockQuad>,
    sprites: Vec<SpriteGlyph>,
}

#[derive(Clone, PartialEq)]
struct RowLayoutKey {
    theme_generation: u64,
    dimensions: CellDimensions,
    base_font: Font,
    selection_range: Option<SelectionRange>,
    search_highlights: Vec<SearchHighlight>,
    display_offset: usize,
    desired_cols: usize,
    desired_rows: usize,
    first_visible_row: i32,
    last_visible_row: i32,
    integrated_glyphs_enabled: bool,
    minimum_contrast: f32,
}

#[derive(Default)]
pub(crate) struct RowLayoutCache {
    key: Option<RowLayoutKey>,
    rows: Vec<Option<(u64, Arc<RowLayout>)>>,
}

pub struct LayoutState {
    rows: Vec<Arc<RowLayout>>,
    rects: Vec<LayoutRect>,
    selection_rects: Vec<LayoutRect>,
    search_rects: Vec<LayoutRect>,
    cursor: Option<CursorInfo>,
    anchor_cursor: Option<CursorInfo>,
    #[cfg(test)]
    dimensions: CellDimensions,
    background_color: Hsla,
    scrollbar_thumb: Hsla,
    scrollbar_track: Hsla,
    exited: Option<i32>,
    exit_signal: Option<String>,
    display_offset: usize,
    history_size: usize,
    desired_cols: usize,
    desired_rows: usize,
    link_text_color: Hsla,
    ime_cursor_bounds: Option<Bounds<Pixels>>,
    color_emoji_enabled: bool,
}

impl LayoutState {
    fn batched_runs(&self) -> impl Iterator<Item = &BatchedTextRun> {
        self.rows.iter().flat_map(|row| row.batched_runs.iter())
    }

    fn decorations(&self) -> impl Iterator<Item = &Decoration> {
        self.rows.iter().flat_map(|row| row.decorations.iter())
    }

    fn symbols(&self) -> impl Iterator<Item = &SymbolGlyph> {
        self.rows.iter().flat_map(|row| row.symbols.iter())
    }

    fn block_quads(&self) -> impl Iterator<Item = &BlockQuad> {
        self.rows.iter().flat_map(|row| row.block_quads.iter())
    }

    fn sprites(&self) -> impl Iterator<Item = &SpriteGlyph> {
        self.rows.iter().flat_map(|row| row.sprites.iter())
    }
}

#[derive(Clone, Copy, PartialEq)]
struct CellStyle {
    bold: bool,
    italic: bool,
    fg: Hsla,
    bg: Hsla,
    underline: UnderlineKind,
    strikethrough: bool,
}

#[derive(Clone, PartialEq)]
pub(crate) struct LayoutCacheKey {
    content_generation: u64,
    theme_generation: u64,
    bounds: Bounds<Pixels>,
    first_visible_row: i32,
    last_visible_row: i32,
    dimensions: CellDimensions,
    base_font: Font,
    focused: bool,
    copy_mode_cursor: Option<CopyModeCursorState>,
    search_highlights: Vec<SearchHighlight>,
    exited: Option<i32>,
    exit_signal: Option<String>,
    default_cursor_shape: CursorShape,
    cursor_color_override: Option<Hsla>,
    integrated_glyphs_enabled: bool,
    color_emoji_enabled: bool,
    minimum_contrast: f32,
}

#[derive(Default)]
pub(crate) struct TerminalRenderCache {
    layout: Option<(LayoutCacheKey, Arc<LayoutState>)>,
    match_ticks: paint::scrollbar::MatchTickCache,
    rows: RowLayoutCache,
}

pub(crate) type SharedLayoutCache = Arc<Mutex<TerminalRenderCache>>;

pub struct TerminalElement {
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
    default_cursor_shape: CursorShape,
    cursor_color_override: Option<Hsla>,
    needs_initial_clear: Arc<std::sync::atomic::AtomicBool>,
    terminal_window_size: Arc<Mutex<Option<TerminalWindowSize>>>,
    scrollbar_metrics: Arc<Mutex<Option<ScrollbarMetrics>>>,
    scrollbar_presence: ScrollbarPresence,
    search_rail_lines: Arc<[usize]>,
    integrated_glyphs_enabled: bool,
    color_emoji_enabled: bool,
    minimum_contrast: f32,
    frame_metrics: TerminalFrameMetrics,
    alt_screen: bool,
    #[cfg(debug_assertions)]
    last_keystroke_at: Option<std::time::Instant>,
    layout_cache: SharedLayoutCache,
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
        scrollbar_presence: ScrollbarPresence,
        search_rail_lines: Arc<[usize]>,
        default_cursor_shape: CursorShape,
        cursor_color_override: Option<Hsla>,
        integrated_glyphs_enabled: bool,
        color_emoji_enabled: bool,
        minimum_contrast: f32,
        frame_metrics: TerminalFrameMetrics,
        alt_screen: bool,
        layout_cache: SharedLayoutCache,
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
            scrollbar_presence,
            search_rail_lines,
            cursor_color_override,
            integrated_glyphs_enabled,
            color_emoji_enabled,
            minimum_contrast,
            frame_metrics,
            alt_screen,
            layout_cache,
            #[cfg(debug_assertions)]
            last_keystroke_at,
        }
    }

    fn build_layout(
        &self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut App,
    ) -> Arc<LayoutState> {
        let dims = self.frame_metrics.dimensions;
        let theme = crate::theme::active_theme();

        let inset_x = px(crate::app::constants::PANE_CONTENT_INSET_X);
        let inset_y = px(crate::app::constants::PANE_CONTENT_INSET_Y);
        let available_width = (bounds.size.width - inset_x * 2.).max(px(0.0));
        let available_height = (bounds.size.height - inset_y * 2.).max(px(0.0));
        let desired_cols = (available_width / dims.cell_width)
            .next_up()
            .floor()
            .max(1.0) as usize;
        let desired_rows = (available_height / dims.line_height)
            .next_up()
            .floor()
            .max(1.0) as usize;

        let content_mask = window.content_mask();
        let visible_top = content_mask.bounds.origin.y;
        let visible_bottom = visible_top + content_mask.bounds.size.height;
        let grid_top = bounds.origin.y + inset_y;
        let first_visible_row = ((visible_top - grid_top) / dims.line_height)
            .floor()
            .max(0.0) as i32;
        let last_visible_row = ((visible_bottom - grid_top) / dims.line_height)
            .ceil()
            .max(0.0) as i32;

        let cursor_color = self.cursor_color_override.unwrap_or(theme.cursor);
        let window_size = TerminalWindowSize::new(
            desired_cols,
            desired_rows,
            terminal_metric_to_u16(dims.cell_width.as_f32()),
            terminal_metric_to_u16(dims.line_height.as_f32()),
        );

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

        let palette = crate::theme::active_palette();
        let cursor_snapshot = cursor_from_content(
            content.cursor,
            self.focused,
            cursor_color,
            self.default_cursor_shape,
            RenderColors {
                theme: &theme,
                palette: &palette,
            },
        );
        let copy_mode_cursor =
            focused_copy_mode_cursor(self.copy_mode_cursor.as_ref(), self.focused);

        let key = LayoutCacheKey {
            content_generation: content.generation,
            theme_generation: crate::theme::theme_generation(),
            bounds,
            first_visible_row,
            last_visible_row,
            dimensions: dims,
            base_font: self.frame_metrics.base_font.clone(),
            focused: self.focused,
            copy_mode_cursor: self.copy_mode_cursor.clone(),
            search_highlights: self.search_highlights.clone(),
            exited: self.exited,
            exit_signal: self.exit_signal.clone(),
            default_cursor_shape: self.default_cursor_shape,
            cursor_color_override: self.cursor_color_override,
            integrated_glyphs_enabled: self.integrated_glyphs_enabled,
            color_emoji_enabled: self.color_emoji_enabled,
            minimum_contrast: self.minimum_contrast,
        };
        {
            let cache = self
                .layout_cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some((cached_key, cached_layout)) = cache.layout.as_ref()
                && *cached_key == key
            {
                return cached_layout.clone();
            }
        }

        let cells = content.cells;

        let mut cache = self
            .layout_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.layout = None;
        let layout = Arc::new(layout_from_snapshot_cached(
            LayoutInputs {
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
                palette: &palette,
                exited: self.exited,
                exit_signal: self.exit_signal.clone(),
                integrated_glyphs_enabled: self.integrated_glyphs_enabled,
                color_emoji_enabled: self.color_emoji_enabled,
                minimum_contrast: self.minimum_contrast,
            },
            &content.row_versions,
            key.theme_generation,
            &mut cache.rows,
        ));
        cache.layout = Some((key, layout.clone()));
        layout
    }
}

#[cfg(test)]
pub(crate) fn layout_from_snapshot(inputs: LayoutInputs<'_>) -> LayoutState {
    layout_from_snapshot_cached(inputs, &[], 0, &mut RowLayoutCache::default())
}

pub(crate) fn layout_from_snapshot_cached(
    inputs: LayoutInputs<'_>,
    row_versions: &[u64],
    theme_generation: u64,
    cache: &mut RowLayoutCache,
) -> LayoutState {
    let key = RowLayoutKey {
        theme_generation,
        dimensions: inputs.dims,
        base_font: inputs.base_font.clone(),
        selection_range: inputs.selection_range,
        search_highlights: inputs.search_highlights.to_vec(),
        display_offset: inputs.display_offset,
        desired_cols: inputs.desired_cols,
        desired_rows: inputs.desired_rows,
        first_visible_row: inputs.first_visible_row,
        last_visible_row: inputs.last_visible_row,
        integrated_glyphs_enabled: inputs.integrated_glyphs_enabled,
        minimum_contrast: inputs.minimum_contrast,
    };
    if cache.key.as_ref() != Some(&key) {
        cache.rows.clear();
        cache.rows.resize_with(inputs.desired_rows, || None);
        cache.key = Some(key);
    }
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
        palette,
        exited,
        exit_signal,
        integrated_glyphs_enabled,
        color_emoji_enabled,
        minimum_contrast,
    } = inputs;

    let colors = RenderColors { theme, palette };

    let background_color = gpui::transparent_black();
    let selection_color = theme.selection;

    let cursor_snapshot = cursor_snapshot.and_then(|mut cursor| {
        cursor.line += display_offset as i32;
        (cursor.line >= 0 && cursor.line < desired_rows as i32).then_some(cursor)
    });

    let (cursor_snapshot, anchor_cursor) = if let Some(cm) = copy_mode_cursor {
        let display_line = cm.grid_line + display_offset as i32;
        let marker_color = selection_marker_color();
        let cursor_ctx = CursorCellContext {
            desired_cols,
            desired_rows,
            colors,
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
        (None, None)
    } else {
        (cursor_snapshot, None)
    };

    let search_match_color = Hsla {
        h: 0.14,
        s: 0.95,
        l: 0.55,
        a: 1.0,
    };
    let search_active_color = Hsla {
        h: 0.10,
        s: 1.0,
        l: 0.55,
        a: 1.0,
    };
    let search_foreground = Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.1,
        a: 1.0,
    };

    let mut search_rects = Vec::new();
    let mut search_cover: Vec<Vec<(usize, usize)>> = vec![Vec::new(); desired_rows];
    for highlight in search_highlights {
        let start_line = highlight.start.line.0.saturating_add(display_offset as i32);
        let end_line = highlight.end.line.0.saturating_add(display_offset as i32);
        for display_line in start_line.max(0)..=end_line.min(desired_rows as i32 - 1) {
            let color = if highlight.is_active {
                search_active_color
            } else {
                search_match_color
            };

            let col_start = if display_line == start_line {
                highlight.start.column.0
            } else {
                0
            };
            let col_end = if display_line == end_line {
                highlight.end.column.0
            } else {
                desired_cols.saturating_sub(1)
            };
            search_rects.push(LayoutRect {
                line: display_line,
                num_lines: 1,
                col: col_start,
                num_cols: col_end.saturating_sub(col_start) + 1,
                color,
            });
            if let Some(spans) = search_cover.get_mut(display_line as usize) {
                spans.push((col_start, col_end));
            }
        }
    }

    let harmony = (minimum_contrast > 0.0).then(|| HarmonyTargets::from_theme(theme));

    let build_row = |cells: &[Cell]| {
        let mut batch = BatchAccumulator::new(base_font.clone());
        let mut rects: Vec<LayoutRect> = Vec::new();
        let mut block_quads: Vec<BlockQuad> = Vec::new();
        let mut sprites: Vec<SpriteGlyph> = Vec::new();
        let mut symbols: Vec<SymbolGlyph> = Vec::new();
        let mut current_rect: Option<LayoutRect> = None;
        let mut last_line: i32 = i32::MIN;
        let mut previous_cell_had_extras = false;
        let mut last_symbol: Option<(i32, usize)> = None;
        let mut run_correction: Option<(Color, Color, Hsla)> = None;

        for (index, cell) in cells.iter().enumerate() {
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

            if point.line.0 < first_visible_row || point.line.0 >= last_visible_row {
                continue;
            }

            if flags.contains(CellFlags::WIDE_CHAR_SPACER) {
                continue;
            }

            if point.line.0 != last_line {
                batch.flush();
                if let Some(rect) = current_rect.take() {
                    rects.push(rect);
                }
                last_line = point.line.0;
            }

            let (raw_fg, raw_bg) = if flags.contains(CellFlags::INVERSE) {
                (*cell_bg, *cell_fg)
            } else {
                (*cell_fg, *cell_bg)
            };
            let mut fg = convert_color(raw_fg, theme, palette);
            let bg =
                terminal_panel_background(raw_bg, convert_color(raw_bg, theme, palette), theme);

            let decorative = is_decorative_character(*c);
            let searched = !decorative
                && search_cover
                    .get(point.line.0 as usize)
                    .is_some_and(|spans| {
                        spans
                            .iter()
                            .any(|(start, end)| (*start..=*end).contains(&point.column.0))
                    });
            let selected = !decorative
                && selection_range
                    .as_ref()
                    .is_some_and(|sel| is_cell_in_selection(point, sel, display_offset));

            if searched {
                fg = search_foreground;
            } else if selected {
                fg = theme.selection_foreground;
            } else {
                if minimum_contrast > 0.0
                    && is_correctable_source(raw_fg)
                    && !decorative
                    && !is_same_visible_color(fg, bg)
                {
                    fg = match run_correction {
                        Some((run_fg, run_bg, corrected))
                            if run_fg == raw_fg && run_bg == raw_bg =>
                        {
                            corrected
                        }
                        _ => {
                            let corrected =
                                ensure_minimum_contrast(fg, bg, minimum_contrast, harmony.as_ref());
                            run_correction = Some((raw_fg, raw_bg, corrected));
                            corrected
                        }
                    };
                }

                if flags.contains(CellFlags::DIM) {
                    fg.a *= 0.5;
                }
            }

            let cell_cols = if flags.contains(CellFlags::WIDE_CHAR) {
                2
            } else {
                1
            };
            let cell_bg_color = resolved_cell_background(*cell_fg, *cell_bg, flags, colors);
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

            let c = *c;
            if c == ' ' && previous_cell_had_extras {
                previous_cell_had_extras = false;
                continue;
            }

            let has_extras = matches!(zw, Some(chars) if !chars.is_empty());

            if c == ' ' || c == '\0' {
                previous_cell_had_extras = has_extras;
                batch.flush();
                continue;
            }

            if integrated_glyphs_enabled && let Some(sprite) = sprite_for(c) {
                batch.flush();
                sprites.push(SpriteGlyph {
                    line: point.line.0,
                    col: point.column.0,
                    num_cols: cell_cols,
                    color: fg,
                    sprite,
                });
                previous_cell_had_extras = false;
                continue;
            }

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

            if integrated_glyphs_enabled && is_private_use(c) {
                batch.flush();
                let next_is_empty = cells.get(index + 1).is_some_and(|next| {
                    next.point.line.0 == point.line.0
                        && next.point.column.0 == point.column.0 + cell_cols
                        && (next.c == ' ' || next.c == '\0')
                });
                let after_symbol = last_symbol
                    .is_some_and(|(line, col)| line == point.line.0 && col + 1 == point.column.0);
                let at_line_end = point.column.0 + cell_cols >= desired_cols;
                let span = if cell_cols == 2 || (next_is_empty && !after_symbol && !at_line_end) {
                    2
                } else {
                    1
                };
                symbols.push(SymbolGlyph {
                    line: point.line.0,
                    col: point.column.0,
                    span,
                    color: fg,
                    ch: c,
                    font: base_font.clone(),
                });
                last_symbol = Some((point.line.0, point.column.0));
                previous_cell_had_extras = false;
                continue;
            }

            let underline = if flags.contains(CellFlags::UNDERCURL) {
                UnderlineKind::Curly
            } else if flags.contains(CellFlags::DOUBLE_UNDERLINE) {
                UnderlineKind::Double
            } else if flags.contains(CellFlags::DOTTED_UNDERLINE) {
                UnderlineKind::Dotted
            } else if flags.contains(CellFlags::DASHED_UNDERLINE) {
                UnderlineKind::Dashed
            } else if flags.contains(CellFlags::UNDERLINE) || *hyperlink {
                UnderlineKind::Single
            } else {
                UnderlineKind::None
            };
            let style = CellStyle {
                bold: flags.contains(CellFlags::BOLD) || flags.contains(CellFlags::BOLD_ITALIC),
                italic: flags.contains(CellFlags::ITALIC) || flags.contains(CellFlags::BOLD_ITALIC),
                fg,
                bg,
                underline,
                strikethrough: flags.contains(CellFlags::STRIKEOUT),
            };

            if batch.can_append(style, point.line.0, point.column.0) {
                batch.append(c, cell_cols);
            } else {
                batch.flush();
                batch.start(c, cell_cols, style, point.line.0, point.column.0);
            }

            if let Some(chars) = zw {
                batch.append_zerowidth(chars);
            }
            previous_cell_had_extras = has_extras;
        }

        batch.flush();
        if let Some(rect) = current_rect {
            rects.push(rect);
        }
        RowLayout {
            batched_runs: batch.runs,
            decorations: batch.decorations,
            symbols,
            rects,
            block_quads,
            sprites,
        }
    };
    let visible_start = first_visible_row.max(0).min(desired_rows as i32) as usize;
    let visible_end = last_visible_row.max(0).min(desired_rows as i32) as usize;
    let mut rows = Vec::with_capacity(visible_end.saturating_sub(visible_start));
    let mut rects = Vec::new();
    let versions_valid = row_versions.len() == desired_rows;
    for (row, cached_row) in cache.rows.iter_mut().enumerate() {
        if !versions_valid
            || cached_row
                .as_ref()
                .is_some_and(|(version, _)| *version != row_versions[row])
        {
            *cached_row = None;
        }
    }
    for (row, cached_row) in cache
        .rows
        .iter_mut()
        .enumerate()
        .take(visible_end)
        .skip(visible_start)
    {
        let version = versions_valid.then(|| row_versions[row]);
        let reused = cached_row
            .as_ref()
            .filter(|(cached_version, _)| version == Some(*cached_version));
        let layout = if let Some((_, layout)) = reused {
            Arc::clone(layout)
        } else {
            let start = cells.partition_point(|cell| cell.point.line.0 < row as i32);
            let end = cells.partition_point(|cell| cell.point.line.0 <= row as i32);
            let layout = Arc::new(build_row(&cells[start..end]));
            *cached_row = version.map(|version| (version, Arc::clone(&layout)));
            layout
        };
        rects.extend_from_slice(&layout.rects);
        rows.push(layout);
    }
    let rects = merge_background_regions(rects);

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
                push_selection_rect(
                    &mut selection_rects,
                    s_line,
                    s_col,
                    num_cols.saturating_sub(s_col),
                );
                let middle_start = s_line.saturating_add(1).max(visible_start);
                let middle_end = e_line.min(visible_end);
                for line in middle_start..middle_end {
                    push_selection_rect(&mut selection_rects, line, 0, num_cols);
                }
                push_selection_rect(&mut selection_rects, e_line, 0, e_col.saturating_add(1));
            }
        }
    }

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
        rows,
        rects,
        selection_rects,
        search_rects,
        cursor: cursor_snapshot,
        anchor_cursor,
        #[cfg(test)]
        dimensions: dims,
        background_color,
        scrollbar_thumb: theme.scrollbar_thumb,
        scrollbar_track: theme.scrollbar_track,
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
    decorations: Vec<Decoration>,
    text: String,
    style: Option<CellStyle>,
    base_font: Font,
    font: Font,
    fg: Hsla,
    underline: UnderlineKind,
    strikethrough: bool,
    line: i32,
    col_start: usize,
    col_end: usize,
}

impl BatchAccumulator {
    fn new(base_font: Font) -> Self {
        Self {
            runs: Vec::new(),
            decorations: Vec::new(),
            text: String::new(),
            style: None,
            font: base_font.clone(),
            base_font,
            fg: Hsla::default(),
            underline: UnderlineKind::None,
            strikethrough: false,
            line: 0,
            col_start: 0,
            col_end: 0,
        }
    }

    fn can_append(&self, style: CellStyle, line: i32, col: usize) -> bool {
        match &self.style {
            Some(cs) => *cs == style && self.line == line && col == self.col_end,
            None => false,
        }
    }

    fn append(&mut self, c: char, cell_cols: usize) {
        self.text.push(c);
        self.col_end += cell_cols;
    }

    fn append_zerowidth(&mut self, chars: &[char]) {
        if self.text.is_empty() {
            return;
        }
        for &c in chars {
            self.text.push(c);
        }
    }

    fn start(&mut self, c: char, cell_cols: usize, style: CellStyle, line: i32, col_start: usize) {
        self.text.push(c);
        let mut font = self.base_font.clone();
        if style.bold {
            font.weight = FontWeight::BOLD;
        }
        if style.italic {
            font.style = FontStyle::Italic;
        }
        self.font = font;
        self.fg = style.fg;
        self.underline = style.underline;
        self.strikethrough = style.strikethrough;
        self.style = Some(style);
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
            line: self.line,
            col_start: self.col_start,
        });
        let num_cols = self.col_end.saturating_sub(self.col_start);
        if self.underline != UnderlineKind::None {
            self.decorations.push(Decoration {
                line: self.line,
                col_start: self.col_start,
                num_cols,
                kind: DecorationKind::Underline(self.underline),
                color: self.fg,
            });
        }
        if self.strikethrough {
            self.decorations.push(Decoration {
                line: self.line,
                col_start: self.col_start,
                num_cols,
                kind: DecorationKind::Strikethrough,
                color: self.fg,
            });
        }
        self.style = None;
    }
}

impl TerminalElement {
    fn track_drag_beyond_the_pane(
        &self,
        bounds: Bounds<Pixels>,
        origin: Point<Pixels>,
        cell_width: Pixels,
        line_height: Pixels,
        window: &mut Window,
    ) {
        let backend = self.backend.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _window, _cx| {
            if phase != DispatchPhase::Bubble
                || event.pressed_button != Some(MouseButton::Left)
                || bounds.contains(&event.position)
            {
                return;
            }
            let geometry = backend.selection_geometry(cell_width.into(), line_height.into());
            let position = (
                f32::from(event.position.x - origin.x),
                f32::from(event.position.y - origin.y),
            );
            backend.drag_selection(
                geometry.cell_at(position),
                position,
                geometry,
                event.modifiers.alt,
            );
        });
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Option<Arc<LayoutState>>;

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

        let mut origin = Point {
            x: bounds.origin.x + px(crate::app::constants::PANE_CONTENT_INSET_X),
            y: bounds.origin.y + px(crate::app::constants::PANE_CONTENT_INSET_Y),
        };
        let scale_factor = window.scale_factor().max(1.0);
        let snap_px = |v: Pixels| px((f32::from(v) * scale_factor).floor() / scale_factor);
        origin.x = snap_px(origin.x);
        origin.y = snap_px(origin.y);
        *self
            .element_origin
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = origin;
        let font_size = self.frame_metrics.font_size;

        let geom = CellGeometry::new(origin, self.frame_metrics.metrics);
        let cell_width = geom.cell_width;
        let line_height = geom.line_height;

        self.track_drag_beyond_the_pane(bounds, origin, cell_width, line_height, window);

        let kitty_placements = self.backend.kitty_placements();

        #[cfg(debug_assertions)]
        pixel_probe::record_origin(origin);

        let base_font = &self.frame_metrics.base_font;

        let (cell_x_bounds, cell_y_bounds) = if layout.desired_cols == 0 || layout.desired_rows == 0
        {
            (Vec::new(), Vec::new())
        } else {
            (
                geom.x_boundaries(layout.desired_cols),
                geom.y_boundaries(layout.desired_rows),
            )
        };

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            paint::background::paint_base_fill(&layout, bounds, window);

            paint::background::paint_cell_backgrounds(
                &layout,
                bounds,
                &cell_x_bounds,
                &cell_y_bounds,
                window,
            );

            paint::selection::paint_selection(&layout, &geom, window);

            paint::overlay::paint_search_highlights(&layout, &geom, window);

            paint::background::paint_block_quads(&layout, &cell_x_bounds, &cell_y_bounds, window);

            paint::sprites::paint_sprites(&layout, &geom, window);

            paint::kitty::paint_below_text(&kitty_placements, &geom, window);

            paint::decorations::paint_decorations(&layout, &geom, window);

            paint::text::paint_text_runs(&layout, &geom, base_font, font_size, window, cx);
            paint::text::paint_symbols(&layout, &geom, font_size, window, cx);

            paint::kitty::paint_above_text(&kitty_placements, &geom, window);

            #[cfg(debug_assertions)]
            if pixel_probe::overlay_enabled() {
                paint::overlay::paint_pixel_probe_overlay(&layout, &geom, window);
            }

            paint::overlay::paint_hyperlink_underline(self, &layout, &geom, window);

            if self.cursor_visible {
                paint::cursor::paint_cursor(&layout, &geom, base_font, font_size, window, cx);
            }

            paint::cursor::paint_anchor_cursor(&layout, &geom, base_font, font_size, window, cx);

            paint::scrollbar::paint_scrollbar(
                &layout,
                self.scrollbar_presence,
                &geom,
                bounds,
                window,
            );

            paint::scrollbar::paint_match_ticks(
                &self.search_rail_lines,
                &self.layout_cache,
                crate::theme::ui_colors().vc_modified,
                &layout,
                &geom,
                bounds,
                window,
            );

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

            let exit_fg = rgb_to_hsla(0x6c, 0x70, 0x86);
            paint::overlay::paint_exit_overlay(
                &layout, &geom, bounds, font_size, base_font, exit_fg, window, cx,
            );
        });

        #[cfg(debug_assertions)]
        if let Some(paint_start) = _paint_start {
            let paint_elapsed = paint_start.elapsed();
            let paint_ms = paint_elapsed.as_secs_f64() * 1000.0;

            if paint_ms > 1.0 {
                log::warn!("[latency] paint: {paint_ms:.2}ms");
            }

            if let Some(keystroke_at) = self.last_keystroke_at {
                let total_elapsed = keystroke_at.elapsed();
                let total_ms = total_elapsed.as_secs_f64() * 1000.0;
                let handler_to_paint_ms = total_ms - paint_ms;
                if total_ms > 8.0 {
                    log::warn!(
                        "[latency] key-handler→paint-cpu: {total_ms:.2}ms \
                         (key-handler→paint-start: {handler_to_paint_ms:.2}ms, \
                         paint: {paint_ms:.2}ms)"
                    );
                }
            }
        }
    }
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

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
        self.terminal_view.update(cx, |view, cx| {
            view.set_marked_text(new_text.to_string(), cx);
        });
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut App) {
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

#[cfg(test)]
mod block_char_coverage_tests {
    use super::*;

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

    #[test]
    fn full_block_covers_entire_cell() {
        let rects = block_char_coverages('█').expect("█ covered");
        assert_eq!(rects, &[(0.0, 0.0, 1.0, 1.0)]);
    }

    #[test]
    fn upper_one_eighth_block_u2594() {
        let rects = block_char_coverages('▔').expect("▔ covered");
        assert_eq!(rects.len(), 1);
        let (x, y, w, h) = rects[0];
        assert_eq!((x, y), (0.0, 0.0));
        assert_eq!(w, 1.0);
        assert!((h - 1.0 / 8.0).abs() < 1e-6, "expected h=1/8, got {h}");
    }

    #[test]
    fn single_quadrants_are_one_rect_each() {
        let cases = [
            ('▖', (0.0, 0.5, 0.5, 0.5)),
            ('▗', (0.5, 0.5, 0.5, 0.5)),
            ('▘', (0.0, 0.0, 0.5, 0.5)),
            ('▝', (0.5, 0.0, 0.5, 0.5)),
        ];
        for (c, expected) in cases {
            let rects = block_char_coverages(c).unwrap();
            assert_eq!(rects, &[expected], "U+{:04X} '{c}'", c as u32);
        }
    }

    #[test]
    fn multi_quadrants_emit_two_rects() {
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
        for c in ['▚', '▞'] {
            let rects = block_char_coverages(c).unwrap();
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

    #[test]
    fn us005_claude_code_codepoints_all_covered() {
        for c in ['▔', '▖', '▗', '▘', '▝', '▙', '▚', '▛', '▜', '▞', '▟'] {
            assert!(
                block_char_coverages(c).is_some(),
                "U+{:04X} '{c}' must be covered to render Claude Code's banner gap-free",
                c as u32,
            );
        }
    }

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

#[cfg(test)]
fn hsla_repr(c: Hsla) -> String {
    format!("hsla({:.4},{:.4},{:.4},{:.4})", c.h, c.s, c.l, c.a)
}

#[cfg(test)]
impl LayoutState {
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
            "bg={} thumb={} track={} link={}",
            hsla_repr(self.background_color),
            hsla_repr(self.scrollbar_thumb),
            hsla_repr(self.scrollbar_track),
            hsla_repr(self.link_text_color),
        );
        let _ = writeln!(s, "runs[{}]:", self.batched_runs().count());
        for r in self.batched_runs() {
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
                "  L{} C{} {:?} fg={} {}",
                r.line,
                r.col_start,
                r.text,
                hsla_repr(r.color),
                style,
            );
        }
        let _ = writeln!(s, "decorations[{}]:", self.decorations().count());
        for d in self.decorations() {
            let _ = writeln!(
                s,
                "  L{} C{}+{}c {:?} {}",
                d.line,
                d.col_start,
                d.num_cols,
                d.kind,
                hsla_repr(d.color),
            );
        }
        let _ = writeln!(s, "symbols[{}]:", self.symbols().count());
        for g in self.symbols() {
            let _ = writeln!(
                s,
                "  L{} C{} span={} U+{:04X} {}",
                g.line,
                g.col,
                g.span,
                g.ch as u32,
                hsla_repr(g.color),
            );
        }
        let _ = writeln!(s, "sprites[{}]:", self.sprites().count());
        for g in self.sprites() {
            let _ = writeln!(
                s,
                "  L{} C{}+{}c {:?} {}",
                g.line,
                g.col,
                g.num_cols,
                g.sprite,
                hsla_repr(g.color),
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
        let _ = writeln!(s, "blocks[{}]:", self.block_quads().count());
        for q in self.block_quads() {
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
    use super::*;
    use crate::terminal::types::{RenderableCursor, Rgb};
    use paneflow_terminal_ghostty as ghostty;

    const COLS: usize = 12;
    const ROWS: usize = 4;
    const TEST_MINIMUM_CONTRAST: f32 = 45.0;

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
            cell_bg: crate::theme::paneflow_dark().ansi_background,
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
        let theme = crate::theme::paneflow_dark();
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
            palette: &ThemePalette::from_theme(&theme),
            exited: None,
            exit_signal: None,
            integrated_glyphs_enabled,
            color_emoji_enabled: true,
            minimum_contrast: TEST_MINIMUM_CONTRAST,
        })
    }

    fn cached_inputs<'a>(
        cells: Arc<[Cell]>,
        theme: &'a crate::theme::TerminalTheme,
        palette: &'a ThemePalette,
    ) -> LayoutInputs<'a> {
        LayoutInputs {
            cells,
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
            theme,
            palette,
            exited: None,
            exit_signal: None,
            integrated_glyphs_enabled: true,
            color_emoji_enabled: true,
            minimum_contrast: 0.0,
        }
    }

    fn test_colors<'a>(
        theme: &'a crate::theme::TerminalTheme,
        palette: &'a ThemePalette,
    ) -> RenderColors<'a> {
        RenderColors { theme, palette }
    }

    fn cached_test_cells() -> Arc<[Cell]> {
        [
            text_row(0, "first", default_fg(), CellFlags::empty()),
            text_row(1, "second", default_fg(), CellFlags::UNDERLINE),
            text_row(2, "third", default_fg(), CellFlags::empty()),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    #[test]
    fn row_cache_rebuilds_only_changed_rows_across_skipped_publications() {
        let theme = crate::theme::paneflow_dark();
        let palette = ThemePalette::from_theme(&theme);
        let cells = cached_test_cells();
        let mut cache = RowLayoutCache::default();
        let first = layout_from_snapshot_cached(
            cached_inputs(cells.clone(), &theme, &palette),
            &[1; ROWS],
            1,
            &mut cache,
        );
        let mut changed = cells.to_vec();
        changed
            .iter_mut()
            .find(|cell| cell.point.line.0 == 1)
            .unwrap()
            .c = 'X';
        let changed: Arc<[Cell]> = changed.into();
        let mut versions = [1; ROWS];
        versions[1] = 9;
        let second = layout_from_snapshot_cached(
            cached_inputs(changed.clone(), &theme, &palette),
            &versions,
            1,
            &mut cache,
        );
        for row in 0..ROWS {
            assert_eq!(Arc::ptr_eq(&first.rows[row], &second.rows[row]), row != 1);
        }
        let full = layout_from_snapshot(cached_inputs(changed.clone(), &theme, &palette));
        assert_eq!(second.golden_repr(), full.golden_repr());
        assert_eq!(Arc::strong_count(&cells), 1);
        assert_eq!(Arc::strong_count(&changed), 1);
    }

    #[test]
    fn row_cache_updates_cursor_and_metadata_without_rebuilding_cells() {
        let theme = crate::theme::paneflow_dark();
        let palette = ThemePalette::from_theme(&theme);
        let cells = cached_test_cells();
        let mut cache = RowLayoutCache::default();
        let first = layout_from_snapshot_cached(
            cached_inputs(cells.clone(), &theme, &palette),
            &[1; ROWS],
            1,
            &mut cache,
        );
        let mut inputs = cached_inputs(cells, &theme, &palette);
        inputs.cursor = Some(cursor_at(3, CursorShape::Beam, None));
        inputs.history_size = 100;
        inputs.exited = Some(0);
        let second = layout_from_snapshot_cached(inputs, &[1; ROWS], 1, &mut cache);
        assert!(
            first
                .rows
                .iter()
                .zip(&second.rows)
                .all(|(first, second)| Arc::ptr_eq(first, second))
        );
        assert_eq!(second.cursor.as_ref().unwrap().col, 3);
        assert_eq!(second.history_size, 100);
        assert_eq!(second.exited, Some(0));
        assert!(second.ime_cursor_bounds.is_some());
    }

    #[test]
    fn row_cache_invalidates_selection_search_geometry_theme_and_scroll() {
        let theme = crate::theme::paneflow_dark();
        let palette = ThemePalette::from_theme(&theme);
        let cells = cached_test_cells();
        let highlight = [SearchHighlight {
            start: GridPoint::new(0, 0),
            end: GridPoint::new(0, 2),
            is_active: true,
        }];
        for scenario in 0..7 {
            let mut cache = RowLayoutCache::default();
            let first = layout_from_snapshot_cached(
                cached_inputs(cells.clone(), &theme, &palette),
                &[1; ROWS],
                1,
                &mut cache,
            );
            let mut inputs = cached_inputs(cells.clone(), &theme, &palette);
            let mut generation = 1;
            match scenario {
                0 => {
                    inputs.selection_range = Some(SelectionRange {
                        start: GridPoint::new(0, 0),
                        end: GridPoint::new(0, 2),
                        is_block: false,
                    })
                }
                1 => inputs.search_highlights = &highlight,
                2 => inputs.dims.cell_width += px(1.0),
                3 => generation = 2,
                4 => inputs.display_offset = 1,
                5 => inputs.desired_cols += 1,
                _ => inputs.minimum_contrast = TEST_MINIMUM_CONTRAST,
            }
            let second = layout_from_snapshot_cached(inputs, &[1; ROWS], generation, &mut cache);
            assert!(
                first
                    .rows
                    .iter()
                    .zip(&second.rows)
                    .all(|(first, second)| !Arc::ptr_eq(first, second)),
                "scenario {scenario}"
            );
        }
    }

    #[test]
    fn row_cache_without_complete_versions_rebuilds_every_row() {
        let theme = crate::theme::paneflow_dark();
        let palette = ThemePalette::from_theme(&theme);
        let cells = cached_test_cells();
        let mut cache = RowLayoutCache::default();
        let first = layout_from_snapshot_cached(
            cached_inputs(cells.clone(), &theme, &palette),
            &[1; ROWS],
            1,
            &mut cache,
        );
        let second = layout_from_snapshot_cached(
            cached_inputs(cells, &theme, &palette),
            &[1],
            1,
            &mut cache,
        );
        assert!(
            first
                .rows
                .iter()
                .zip(&second.rows)
                .all(|(first, second)| !Arc::ptr_eq(first, second))
        );
    }

    fn run_selection_with_visible(
        selection: SelectionRange,
        first_visible_row: i32,
        last_visible_row: i32,
    ) -> LayoutState {
        let theme = crate::theme::paneflow_dark();
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
            palette: &ThemePalette::from_theme(&theme),
            exited: None,
            exit_signal: None,
            integrated_glyphs_enabled: true,
            color_emoji_enabled: true,
            minimum_contrast: TEST_MINIMUM_CONTRAST,
        })
    }

    #[test]
    fn search_highlights_wrapped_matches_with_start_above_viewport() {
        let theme = crate::theme::paneflow_dark();
        let highlights = [SearchHighlight {
            start: GridPoint::new(-3, 7),
            end: GridPoint::new(0, 2),
            is_active: true,
        }];
        let layout = layout_from_snapshot(LayoutInputs {
            cells: Vec::new().into(),
            cursor: None,
            selection_range: None,
            copy_mode_cursor: None,
            search_highlights: &highlights,
            display_offset: 1,
            history_size: 3,
            desired_cols: COLS,
            desired_rows: ROWS,
            first_visible_row: 0,
            last_visible_row: ROWS as i32,
            dims: test_dims(),
            base_font: test_font(),
            theme: &theme,
            palette: &ThemePalette::from_theme(&theme),
            exited: None,
            exit_signal: None,
            integrated_glyphs_enabled: true,
            color_emoji_enabled: true,
            minimum_contrast: TEST_MINIMUM_CONTRAST,
        });
        assert_eq!(layout.search_rects.len(), 2);
        assert_eq!(layout.search_rects[0].line, 0);
        assert_eq!(layout.search_rects[0].col, 0);
        assert_eq!(layout.search_rects[0].num_cols, COLS);
        assert_eq!(layout.search_rects[1].line, 1);
        assert_eq!(layout.search_rects[1].col, 0);
        assert_eq!(layout.search_rects[1].num_cols, 3);
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

    #[test]
    fn golden_frame_corpus() {
        assert_golden(
            "plain",
            &run(
                text_row(0, "hi", default_fg(), CellFlags::empty()),
                None,
                None,
            ),
        );

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

        assert_golden(
            "dim",
            &run(text_row(0, "dim", default_fg(), CellFlags::DIM), None, None),
        );

        let inverse = vec![cell(
            0,
            0,
            'x',
            Color::Named(NamedColor::Red),
            Color::Named(NamedColor::Blue),
            CellFlags::INVERSE,
        )];
        assert_golden("inverse", &run(inverse, None, None));

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

        let blocks: Vec<Cell> = "█▀▄▌▙"
            .chars()
            .enumerate()
            .map(|(i, c)| cell(0, i, c, default_fg(), default_bg(), CellFlags::empty()))
            .collect();
        assert_golden("blocks", &run(blocks, None, None));

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

        let apca = vec![cell(
            0,
            0,
            'z',
            Color::Named(NamedColor::Black),
            default_bg(),
            CellFlags::empty(),
        )];
        assert_golden("apca_contrast", &run(apca, None, None));

        let decorated: Vec<Cell> = [
            CellFlags::UNDERLINE,
            CellFlags::DOUBLE_UNDERLINE,
            CellFlags::UNDERCURL,
            CellFlags::DOTTED_UNDERLINE,
            CellFlags::DASHED_UNDERLINE,
            CellFlags::STRIKEOUT,
        ]
        .iter()
        .enumerate()
        .map(|(i, flag)| cell(0, i, 'u', default_fg(), default_bg(), *flag))
        .collect();
        assert_golden("decorations", &run(decorated, None, None));

        let icons: Vec<Cell> = "\u{f09b} a\u{e62b}b\u{ea61}\u{ea61} "
            .chars()
            .enumerate()
            .map(|(i, c)| cell(0, i, c, default_fg(), default_bg(), CellFlags::empty()))
            .collect();
        assert_golden("icons", &run(icons, None, None));

        let sprites: Vec<Cell> = "┣╪┄╭╳▒⣿\u{e0b0}"
            .chars()
            .enumerate()
            .map(|(i, c)| cell(0, i, c, default_fg(), default_bg(), CellFlags::empty()))
            .collect();
        assert_golden("sprites", &run(sprites, None, None));
    }

    #[test]
    fn block_chars_emit_quads_not_runs() {
        let blocks: Vec<Cell> = "█▀▄▌▙"
            .chars()
            .enumerate()
            .map(|(i, c)| cell(0, i, c, default_fg(), default_bg(), CellFlags::empty()))
            .collect();
        let state = run(blocks, None, None);
        assert_eq!(
            state.block_quads().count(),
            6,
            "block chars should map to filled quads"
        );
        assert!(
            state.batched_runs().next().is_none(),
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
            state.block_quads().next().is_none(),
            "integrated glyphs off must not emit block quads"
        );
        assert_eq!(
            state.batched_runs().count(),
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

        assert_eq!(state.sprites().count(), 12);
        assert!(
            state.batched_runs().next().is_none(),
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

        assert!(state.sprites().next().is_none());
        assert_eq!(state.batched_runs().count(), 1);
    }

    #[test]
    fn underline_styles_are_distinct_decorations() {
        let flags = [
            CellFlags::UNDERLINE,
            CellFlags::DOUBLE_UNDERLINE,
            CellFlags::UNDERCURL,
            CellFlags::DOTTED_UNDERLINE,
            CellFlags::DASHED_UNDERLINE,
            CellFlags::STRIKEOUT,
        ];
        let cells: Vec<Cell> = flags
            .iter()
            .enumerate()
            .map(|(i, flag)| cell(0, i, 'a', default_fg(), default_bg(), *flag))
            .collect();
        let state = run(cells, None, None);
        let kinds: Vec<DecorationKind> = state.decorations().map(|d| d.kind).collect();
        assert_eq!(
            kinds,
            vec![
                DecorationKind::Underline(UnderlineKind::Single),
                DecorationKind::Underline(UnderlineKind::Double),
                DecorationKind::Underline(UnderlineKind::Curly),
                DecorationKind::Underline(UnderlineKind::Dotted),
                DecorationKind::Underline(UnderlineKind::Dashed),
                DecorationKind::Strikethrough,
            ]
        );
        for (i, d) in state.decorations().enumerate() {
            assert_eq!((d.line, d.col_start, d.num_cols), (0, i, 1));
        }
        assert_eq!(state.batched_runs().count(), 6);
    }

    #[test]
    fn hyperlink_cells_get_a_single_underline() {
        let mut link = cell(0, 0, 'x', default_fg(), default_bg(), CellFlags::empty());
        link.hyperlink = true;
        let state = run(vec![link], None, None);
        assert_eq!(state.decorations().count(), 1);
        assert_eq!(
            state.decorations().next().unwrap().kind,
            DecorationKind::Underline(UnderlineKind::Single)
        );
    }

    #[test]
    fn nerd_font_icon_spans_two_cells_only_before_an_empty_cell() {
        let icon = '\u{f09b}';
        let row = |text: &str| -> Vec<Cell> {
            text.chars()
                .enumerate()
                .map(|(i, c)| cell(0, i, c, default_fg(), default_bg(), CellFlags::empty()))
                .collect()
        };
        let state = run(row(&format!("{icon} ab")), None, None);
        assert_eq!(state.symbols().count(), 1);
        assert_eq!(
            (
                state.symbols().next().unwrap().col,
                state.symbols().next().unwrap().span
            ),
            (0, 2)
        );
        assert_eq!(
            state.batched_runs().count(),
            1,
            "text after the icon still shapes"
        );

        let state = run(row(&format!("{icon}ab")), None, None);
        assert_eq!(state.symbols().next().unwrap().span, 1);

        let state = run(row(&format!("{icon}{icon} ")), None, None);
        assert_eq!(state.symbols().count(), 2);
        assert_eq!(state.symbols().next().unwrap().span, 1);
        assert_eq!(state.symbols().nth(1).unwrap().span, 1);

        let mut last = row(&" ".repeat(COLS));
        last[COLS - 1].c = icon;
        let state = run(last, None, None);
        assert_eq!(state.symbols().next().unwrap().span, 1);

        let state = run_with_integrated_glyphs(row(&format!("{icon} ab")), None, None, false);
        assert!(state.symbols().next().is_none());
        assert_eq!(state.batched_runs().count(), 2);
    }

    #[test]
    fn sprites_cover_heavy_double_shade_braille_and_powerline() {
        let text = "━║╌╭╳░⣿\u{e0b0}";
        let cells: Vec<Cell> = text
            .chars()
            .enumerate()
            .map(|(i, c)| cell(0, i, c, default_fg(), default_bg(), CellFlags::empty()))
            .collect();
        let state = run(cells, None, None);
        assert_eq!(state.sprites().count(), 8);
        assert!(state.batched_runs().next().is_none());
        assert!(matches!(
            state.sprites().nth(5).unwrap().sprite,
            Sprite::Shade(_)
        ));
        assert!(matches!(
            state.sprites().nth(6).unwrap().sprite,
            Sprite::Braille(0xff)
        ));
    }

    #[test]
    fn dim_halves_the_foreground_alpha() {
        let state = run(text_row(0, "dim", default_fg(), CellFlags::DIM), None, None);
        let bright = run(
            text_row(0, "dim", default_fg(), CellFlags::empty()),
            None,
            None,
        );
        assert!(
            (state.batched_runs().next().unwrap().color.a
                - bright.batched_runs().next().unwrap().color.a * 0.5)
                .abs()
                < 1e-6
        );
    }

    #[test]
    fn contrast_floor_off_leaves_theme_colors_untouched() {
        let theme = crate::theme::paneflow_dark();
        let low = Color::Named(NamedColor::Black);
        let cells = vec![cell(0, 0, 'a', low, default_bg(), CellFlags::empty())];
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
            palette: &ThemePalette::from_theme(&theme),
            exited: None,
            exit_signal: None,
            integrated_glyphs_enabled: true,
            color_emoji_enabled: true,
            minimum_contrast: 0.0,
        });
        assert_eq!(
            state.batched_runs().next().unwrap().color,
            convert_color(low, &theme, &ThemePalette::from_theme(&theme))
        );
    }

    #[test]
    fn shell_cursor_is_hidden_when_scrolled_away_from_live_edge() {
        let theme = crate::theme::paneflow_dark();
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
            palette: &ThemePalette::from_theme(&theme),
            exited: None,
            exit_signal: None,
            integrated_glyphs_enabled: true,
            color_emoji_enabled: true,
            minimum_contrast: 0.0,
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
        let theme = crate::theme::paneflow_dark();
        let palette = ThemePalette::from_theme(&theme);

        assert!(
            cursor_from_content(
                cursor,
                true,
                white(),
                CursorShape::Block,
                test_colors(&theme, &palette)
            )
            .is_some(),
            "focused terminals should keep the live cursor"
        );
        assert!(
            cursor_from_content(
                cursor,
                false,
                white(),
                CursorShape::Block,
                test_colors(&theme, &palette)
            )
            .is_none(),
            "unfocused terminals must not paint a hollow cursor outline"
        );
    }

    #[test]
    fn configured_custom_cursor_shapes_override_native_fallbacks() {
        let block_cursor = renderable_cursor_at(0, CursorShape::Block, 'a');
        let theme = crate::theme::paneflow_dark();
        let palette = ThemePalette::from_theme(&theme);
        let vintage = cursor_from_content(
            block_cursor,
            true,
            white(),
            CursorShape::Vintage,
            test_colors(&theme, &palette),
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
            white(),
            CursorShape::DoubleUnderline,
            test_colors(&theme, &palette),
        )
        .unwrap();
        assert_eq!(double.shape, CursorShape::DoubleUnderline);
    }

    #[test]
    fn block_cursor_carries_cell_background_for_inverse_text() {
        let theme = crate::theme::paneflow_dark();
        let palette = ThemePalette::from_theme(&theme);
        let explicit_bg = Color::Spec(Rgb {
            r: 12,
            g: 34,
            b: 56,
        });
        let mut cursor = renderable_cursor_at(0, CursorShape::Block, 'x');
        cursor.bg = explicit_bg;

        let info = cursor_from_content(
            cursor,
            true,
            white(),
            CursorShape::Block,
            test_colors(&theme, &palette),
        )
        .expect("cursor visible");
        assert_eq!(info.cell_bg, rgb_to_hsla(12, 34, 56));

        let mut inverse = renderable_cursor_at(0, CursorShape::Block, 'x');
        inverse.fg = Color::Spec(Rgb { r: 90, g: 8, b: 7 });
        inverse.flags = CellFlags::INVERSE;
        let info = cursor_from_content(
            inverse,
            true,
            white(),
            CursorShape::Block,
            test_colors(&theme, &palette),
        )
        .expect("cursor visible");
        assert_eq!(info.cell_bg, rgb_to_hsla(90, 8, 7));

        let transparent = renderable_cursor_at(0, CursorShape::Block, 'x');
        let info = cursor_from_content(
            transparent,
            true,
            white(),
            CursorShape::Block,
            test_colors(&theme, &palette),
        )
        .expect("cursor visible");
        assert_eq!(info.cell_bg.a, 0.0);
    }

    #[test]
    fn the_placeholder_grid_paints_no_opaque_background() {
        let placeholder = crate::terminal::ghostty_session::blank_content(COLS, ROWS);
        let state = run(placeholder.cells.to_vec(), None, None);

        let covered: usize = state
            .rects
            .iter()
            .filter(|rect| rect.color.a > 0.0)
            .map(|rect| rect.num_lines * rect.num_cols)
            .sum();
        assert_eq!(
            covered, 0,
            "the grid shown before the first engine frame must let the pane background through"
        );
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
            state.batched_runs().count(),
            1,
            "only the wide glyph produces a run"
        );
        assert_eq!(state.batched_runs().next().unwrap().text, "中");
    }

    #[test]
    fn viewport_cull_drops_offscreen_rows() {
        let theme = crate::theme::paneflow_dark();
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
            last_visible_row: 1,
            dims: test_dims(),
            base_font: test_font(),
            theme: &theme,
            palette: &ThemePalette::from_theme(&theme),
            exited: None,
            exit_signal: None,
            integrated_glyphs_enabled: true,
            color_emoji_enabled: true,
            minimum_contrast: 0.0,
        });
        assert_eq!(state.batched_runs().count(), 1, "row 2 is culled");
        assert_eq!(state.batched_runs().next().unwrap().text, "a");
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
        let theme = crate::theme::paneflow_dark();
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
            palette: &ThemePalette::from_theme(&theme),
            exited: None,
            exit_signal: None,
            integrated_glyphs_enabled: true,
            color_emoji_enabled: true,
            minimum_contrast: 0.0,
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
        let theme = crate::theme::paneflow_dark();
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
            palette: &ThemePalette::from_theme(&theme),
            exited: None,
            exit_signal: None,
            integrated_glyphs_enabled: true,
            color_emoji_enabled: true,
            minimum_contrast: 0.0,
        });

        assert!(
            state.rects.iter().all(|rect| rect.color == card_bg),
            "neutral panel backgrounds should align with the Codex panel color"
        );
    }

    fn run_with_contrast(
        cells: Vec<Cell>,
        theme: &crate::theme::TerminalTheme,
        minimum_contrast: f32,
    ) -> LayoutState {
        let palette = ThemePalette::from_theme(theme);
        layout_from_snapshot(LayoutInputs {
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
            theme,
            palette: &palette,
            exited: None,
            exit_signal: None,
            integrated_glyphs_enabled: false,
            color_emoji_enabled: true,
            minimum_contrast,
        })
    }

    fn only_run_color(state: &LayoutState) -> Hsla {
        state
            .batched_runs()
            .next()
            .expect("the fixture must lay out one run")
            .color
    }

    fn pale_spec() -> Color {
        Color::Spec(Rgb {
            r: 255,
            g: 245,
            b: 190,
        })
    }

    fn default_background_color(theme: &crate::theme::TerminalTheme) -> Hsla {
        let palette = ThemePalette::from_theme(theme);
        terminal_panel_background(
            default_bg(),
            convert_color(default_bg(), theme, &palette),
            theme,
        )
    }

    #[test]
    fn truecolor_and_indexed_foregrounds_are_corrected() {
        let theme = corpus_theme("Paneflow Light");
        let bg = default_background_color(&theme);
        let mut moved = 0usize;
        for source in [pale_spec(), Color::Indexed(230)] {
            let _ = color::take_correction_calls();
            let cells = text_row(0, "owner", source, CellFlags::empty());
            let uncorrected = only_run_color(&run_with_contrast(cells.clone(), &theme, 0.0));
            assert_eq!(
                color::take_correction_calls(),
                0,
                "{source:?}: a zero threshold must call no correction"
            );

            let corrected = only_run_color(&run_with_contrast(cells, &theme, 60.0));
            assert!(
                color::take_correction_calls() > 0,
                "{source:?}: the correction must run on a program-chosen color"
            );
            let lc = apca_contrast(corrected, bg).abs();
            assert!(
                lc >= 60.0,
                "{source:?}: the corrected foreground must reach the threshold, Lc {lc:.1}"
            );
            if apca_contrast(uncorrected, bg).abs() < 60.0 {
                moved += 1;
                assert_ne!(
                    (corrected.h, corrected.s, corrected.l),
                    (uncorrected.h, uncorrected.s, uncorrected.l),
                    "{source:?}: an illegible foreground must move"
                );
            }
        }
        assert!(
            moved > 0,
            "at least one fixture must start below the threshold for this test to mean anything"
        );
    }

    #[test]
    fn the_themes_own_colors_are_never_corrected() {
        let theme = corpus_theme("Paneflow Dark");
        let mut sources = vec![
            Color::Named(NamedColor::Red),
            Color::Named(NamedColor::Blue),
            Color::Named(NamedColor::Magenta),
            Color::Named(NamedColor::Foreground),
        ];
        sources.extend((0u8..16).map(Color::Indexed));
        for source in sources {
            let _ = color::take_correction_calls();
            let cells = text_row(0, "theme", source, CellFlags::empty());
            let corrected = only_run_color(&run_with_contrast(cells.clone(), &theme, 90.0));
            assert_eq!(
                color::take_correction_calls(),
                0,
                "{source:?}: the theme's own colors must never reach the correction"
            );
            let uncorrected = only_run_color(&run_with_contrast(cells, &theme, 0.0));
            assert_eq!(
                (corrected.h, corrected.s, corrected.l, corrected.a),
                (uncorrected.h, uncorrected.s, uncorrected.l, uncorrected.a),
                "{source:?}: the theme's own colors must render identically at any threshold"
            );
        }
    }

    #[test]
    fn a_foreground_equal_to_its_background_is_left_untouched() {
        let theme = corpus_theme("Paneflow Light");
        let spec = pale_spec();
        let cells: Vec<Cell> = "shape"
            .chars()
            .enumerate()
            .map(|(col, c)| cell(0, col, c, spec, spec, CellFlags::empty()))
            .collect();
        let _ = color::take_correction_calls();
        let corrected = only_run_color(&run_with_contrast(cells.clone(), &theme, 90.0));
        assert_eq!(
            color::take_correction_calls(),
            0,
            "a cell drawn in its own background color must never be corrected"
        );
        let untouched = only_run_color(&run_with_contrast(cells, &theme, 0.0));
        assert_eq!(corrected, untouched);
    }

    #[test]
    fn a_decorative_codepoint_is_never_corrected() {
        let theme = corpus_theme("Paneflow Light");
        let spec = pale_spec();
        let cells: Vec<Cell> = "\u{2500}\u{2591}\u{2801}"
            .chars()
            .enumerate()
            .map(|(col, c)| cell(0, col, c, spec, default_bg(), CellFlags::empty()))
            .collect();
        let _ = color::take_correction_calls();
        let state = run_with_contrast(cells.clone(), &theme, 90.0);
        assert_eq!(
            color::take_correction_calls(),
            0,
            "decorative glyphs must never reach the correction"
        );
        let uncorrected = run_with_contrast(cells, &theme, 0.0);
        let corrected_colors: Vec<Hsla> = state.batched_runs().map(|run| run.color).collect();
        let plain_colors: Vec<Hsla> = uncorrected.batched_runs().map(|run| run.color).collect();
        assert_eq!(corrected_colors, plain_colors);
    }

    #[test]
    fn dim_halves_the_alpha_after_the_correction() {
        let theme = corpus_theme("Paneflow Light");
        let spec = pale_spec();
        let opaque = only_run_color(&run_with_contrast(
            text_row(0, "dim", spec, CellFlags::empty()),
            &theme,
            60.0,
        ));
        let dimmed = only_run_color(&run_with_contrast(
            text_row(0, "dim", spec, CellFlags::DIM),
            &theme,
            60.0,
        ));
        assert_eq!(
            (dimmed.h, dimmed.s, dimmed.l),
            (opaque.h, opaque.s, opaque.l),
            "the correction must run on the opaque color"
        );
        assert!(
            (dimmed.a - opaque.a * 0.5).abs() < 1e-6,
            "dim text must stay dim, alpha {}",
            dimmed.a
        );
    }

    #[test]
    fn the_decorative_ranges_cover_every_ghostty_graphics_block() {
        let ranges: &[(&str, u32, u32)] = &[
            ("Box Drawing", 0x2500, 0x257F),
            ("Block Elements", 0x2580, 0x259F),
            ("Geometric Shapes", 0x25A0, 0x25FF),
            ("Braille Patterns", 0x2800, 0x28FF),
            ("Powerline (Private Use)", 0xE0B0, 0xE0D7),
            ("Symbols for Legacy Computing Supplement", 0x1CC00, 0x1CEBF),
            ("Symbols for Legacy Computing", 0x1FB00, 0x1FBFF),
        ];
        for (block, first, last) in ranges {
            for codepoint in [*first, (first + last) / 2, *last] {
                let ch = char::from_u32(codepoint)
                    .unwrap_or_else(|| panic!("{block}: U+{codepoint:04X} must be a char"));
                assert!(
                    is_decorative_character(ch),
                    "{block}: U+{codepoint:04X} must be decorative"
                );
            }
        }
        for ch in ['a', 'Z', '0', '\u{4E2D}', '\u{00E9}', ' ', '\u{2713}'] {
            assert!(
                !is_decorative_character(ch),
                "{ch:?} is text, not a graphics element"
            );
        }
    }

    const WIDE_COLS: usize = 220;

    fn run_wide(
        cells: Vec<Cell>,
        theme: &crate::theme::TerminalTheme,
        minimum_contrast: f32,
        selection: Option<SelectionRange>,
        highlights: &[SearchHighlight],
    ) -> LayoutState {
        let palette = ThemePalette::from_theme(theme);
        layout_from_snapshot(LayoutInputs {
            cells: cells.into(),
            cursor: None,
            selection_range: selection,
            copy_mode_cursor: None,
            search_highlights: highlights,
            display_offset: 0,
            history_size: 0,
            desired_cols: WIDE_COLS,
            desired_rows: ROWS,
            first_visible_row: 0,
            last_visible_row: ROWS as i32,
            dims: test_dims(),
            base_font: test_font(),
            theme,
            palette: &palette,
            exited: None,
            exit_signal: None,
            integrated_glyphs_enabled: false,
            color_emoji_enabled: true,
            minimum_contrast,
        })
    }

    fn run_texts(state: &LayoutState) -> Vec<String> {
        state
            .batched_runs()
            .map(|run| run.text.to_string())
            .collect()
    }

    #[test]
    fn a_single_color_line_is_corrected_once_for_the_whole_run() {
        let theme = corpus_theme("Paneflow Light");
        let cells = text_row(0, &"x".repeat(WIDE_COLS), pale_spec(), CellFlags::empty());
        let _ = color::take_correction_calls();
        let state = run_wide(cells, &theme, 60.0, None, &[]);
        assert_eq!(
            color::take_correction_calls(),
            1,
            "a 220-column single-color line must cost one correction"
        );
        let runs: Vec<_> = state.batched_runs().collect();
        assert_eq!(runs.len(), 1, "the line must batch into a single run");
        assert_eq!(runs[0].text.chars().count(), WIDE_COLS);
    }

    #[test]
    fn a_decorative_glyph_forms_its_own_uncorrected_run_between_corrected_neighbors() {
        let theme = corpus_theme("Paneflow Light");
        let palette = ThemePalette::from_theme(&theme);
        let uncorrected = convert_color(pale_spec(), &theme, &palette);
        let cells = text_row(0, "ab\u{2592}cd", pale_spec(), CellFlags::empty());
        let _ = color::take_correction_calls();
        let state = run_wide(cells, &theme, 60.0, None, &[]);
        assert_eq!(
            color::take_correction_calls(),
            1,
            "the neighbors of a decorative cell must share one correction"
        );
        let runs: Vec<_> = state.batched_runs().collect();
        assert_eq!(run_texts(&state), ["ab", "\u{2592}", "cd"]);
        assert_eq!(runs[1].color, uncorrected, "the decorative cell is spared");
        assert_ne!(runs[0].color, uncorrected, "its neighbors are corrected");
        assert_eq!(runs[0].color, runs[2].color);
    }

    #[test]
    fn a_dim_cell_splits_the_run_so_alpha_stays_per_cell() {
        let theme = corpus_theme("Paneflow Light");
        let mut cells = Vec::new();
        for (col, c) in "abcde".chars().enumerate() {
            let flags = if col == 2 {
                CellFlags::DIM
            } else {
                CellFlags::empty()
            };
            cells.push(cell(0, col, c, pale_spec(), default_bg(), flags));
        }
        let _ = color::take_correction_calls();
        let state = run_wide(cells, &theme, 60.0, None, &[]);
        assert_eq!(
            color::take_correction_calls(),
            1,
            "a dim cell reuses the run's correction"
        );
        let runs: Vec<_> = state.batched_runs().collect();
        assert_eq!(run_texts(&state), ["ab", "c", "de"]);
        assert_eq!(runs[1].color.a, runs[0].color.a * 0.5);
        assert_eq!(
            (runs[1].color.h, runs[1].color.s, runs[1].color.l),
            (runs[0].color.h, runs[0].color.s, runs[0].color.l)
        );
    }

    #[test]
    fn a_selected_run_keeps_the_selection_foreground_and_is_never_corrected() {
        let theme = corpus_theme("Paneflow Light");
        let cells = text_row(0, "owner", pale_spec(), CellFlags::empty());
        let selection = SelectionRange {
            start: GridPoint::new(0, 0),
            end: GridPoint::new(0, 4),
            is_block: false,
        };
        let _ = color::take_correction_calls();
        let state = run_wide(cells, &theme, 60.0, Some(selection), &[]);
        assert_eq!(
            color::take_correction_calls(),
            0,
            "a fully selected run must not pay for a correction"
        );
        let runs: Vec<_> = state.batched_runs().collect();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].color, theme.selection_foreground);
    }

    #[test]
    fn a_search_match_splits_the_run_and_keeps_the_search_foreground() {
        let theme = corpus_theme("Paneflow Light");
        let cells = text_row(0, "owner", pale_spec(), CellFlags::empty());
        let highlights = [SearchHighlight {
            start: GridPoint::new(0, 0),
            end: GridPoint::new(0, 1),
            is_active: true,
        }];
        let _ = color::take_correction_calls();
        let state = run_wide(cells, &theme, 60.0, None, &highlights);
        assert_eq!(
            color::take_correction_calls(),
            1,
            "only the uncovered tail of the run is corrected"
        );
        let runs: Vec<_> = state.batched_runs().collect();
        assert_eq!(run_texts(&state), ["ow", "ner"]);
        assert_eq!(
            runs[0].color,
            Hsla {
                h: 0.0,
                s: 0.0,
                l: 0.1,
                a: 1.0,
            },
            "the highlighted segment keeps the search foreground"
        );
        assert_ne!(runs[1].color, runs[0].color);
    }

    const CORPUS_COLS: usize = 80;
    const CORPUS_ROWS: usize = 16;
    const CORPUS_FIXTURES: &[&str] =
        &["lsd-la.ansi", "btop.ansi", "lazygit.ansi", "agent-cli.ansi"];
    const CORPUS_TIER: f32 = 45.0;

    fn corpus_fixture_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/contrast")
    }

    fn corpus_stream(name: &str) -> Vec<u8> {
        let path = corpus_fixture_dir().join(name);
        let raw = std::fs::read(&path).unwrap_or_else(|error| {
            panic!(
                "contrast corpus fixture {} could not be read: {error}",
                path.display()
            )
        });
        assert!(
            !raw.is_empty(),
            "contrast corpus fixture {} is empty",
            path.display()
        );
        let mut stream = Vec::with_capacity(raw.len() + 64);
        for byte in raw {
            match byte {
                b'\r' => {}
                b'\n' => stream.extend_from_slice(b"\r\n"),
                other => stream.push(other),
            }
        }
        stream
    }

    fn corpus_content(name: &str) -> Content {
        let size = ghostty::WindowSize::new(CORPUS_COLS, CORPUS_ROWS, 8, 16)
            .expect("the corpus grid is valid");
        let mut terminal =
            ghostty::DisplayTerminal::new(size, 256, ghostty::TerminalAppearance::default())
                .expect("libghostty must initialize");
        terminal
            .feed(&corpus_stream(name))
            .expect("the corpus fixture must parse");
        let snapshot = terminal
            .snapshot()
            .expect("the corpus snapshot must succeed");
        crate::terminal::ghostty_session::CellMirror::default().publish(snapshot)
    }

    fn corpus_layout(
        content: &Content,
        theme: &crate::theme::TerminalTheme,
        palette: &ThemePalette,
        minimum_contrast: f32,
    ) -> LayoutState {
        layout_from_snapshot(LayoutInputs {
            cells: content.cells.clone(),
            cursor: None,
            selection_range: None,
            copy_mode_cursor: None,
            search_highlights: &[],
            display_offset: 0,
            history_size: 0,
            desired_cols: CORPUS_COLS,
            desired_rows: CORPUS_ROWS,
            first_visible_row: 0,
            last_visible_row: CORPUS_ROWS as i32,
            dims: test_dims(),
            base_font: test_font(),
            theme,
            palette,
            exited: None,
            exit_signal: None,
            integrated_glyphs_enabled: true,
            color_emoji_enabled: true,
            minimum_contrast,
        })
    }

    fn composite_over(top: Hsla, under: Hsla) -> Hsla {
        if top.a >= 1.0 {
            return top;
        }
        let top_rgba = gpui::Rgba::from(top);
        let under_rgba = gpui::Rgba::from(under);
        let mix = |a: f32, b: f32| a * top_rgba.a + b * (1.0 - top_rgba.a);
        Hsla::from(gpui::Rgba {
            r: mix(top_rgba.r, under_rgba.r),
            g: mix(top_rgba.g, under_rgba.g),
            b: mix(top_rgba.b, under_rgba.b),
            a: 1.0,
        })
    }

    fn corpus_background_grid(
        state: &LayoutState,
        theme: &crate::theme::TerminalTheme,
    ) -> Vec<Hsla> {
        let mut grid = vec![theme.ansi_background; CORPUS_COLS * CORPUS_ROWS];
        for rect in &state.rects {
            for line in rect.line..rect.line.saturating_add(rect.num_lines as i32) {
                if line < 0 || line >= CORPUS_ROWS as i32 {
                    continue;
                }
                for col in rect.col..rect.col.saturating_add(rect.num_cols) {
                    if col >= CORPUS_COLS {
                        continue;
                    }
                    grid[line as usize * CORPUS_COLS + col] =
                        composite_over(rect.color, theme.ansi_background);
                }
            }
        }
        grid
    }

    fn corpus_source_colors(content: &Content) -> std::collections::HashMap<(i32, usize), Color> {
        content
            .cells
            .iter()
            .filter(|cell| !cell.flags.contains(CellFlags::INVERSE))
            .map(|cell| ((cell.point.line.0, cell.point.column.0), cell.fg))
            .collect()
    }

    struct CorpusCell {
        line: i32,
        col: usize,
        source: Option<Color>,
        color: Hsla,
        bg: Hsla,
        lc: f32,
        baseline_lc: f32,
    }

    fn corpus_cells(
        content: &Content,
        state: &LayoutState,
        theme: &crate::theme::TerminalTheme,
    ) -> Vec<CorpusCell> {
        let backgrounds = corpus_background_grid(state, theme);
        let sources = corpus_source_colors(content);
        let mut measured = Vec::new();
        for run in state.batched_runs() {
            if run.line < 0 || run.line >= CORPUS_ROWS as i32 {
                continue;
            }
            for (offset, ch) in run.text.chars().enumerate() {
                let col = run.col_start + offset;
                if ch == ' ' || ch == '\0' || is_decorative_character(ch) || col >= CORPUS_COLS {
                    continue;
                }
                let bg = backgrounds[run.line as usize * CORPUS_COLS + col];
                let source = sources.get(&(run.line, col)).copied();
                let baseline = match source {
                    Some(Color::Indexed(index)) if index >= 16 => xterm_cube_color(index),
                    _ => run.color,
                };
                measured.push(CorpusCell {
                    line: run.line,
                    col,
                    source,
                    color: run.color,
                    bg,
                    lc: apca_contrast(run.color, bg).abs(),
                    baseline_lc: apca_contrast(baseline, bg).abs(),
                });
            }
        }
        measured
    }

    fn xterm_cube_color(index: u8) -> Hsla {
        if index < 232 {
            let offset = index - 16;
            let axis = |value: u8| if value == 0 { 0 } else { 55 + 40 * value };
            return rgb_to_hsla(axis(offset / 36), axis((offset % 36) / 6), axis(offset % 6));
        }
        let grey = 8 + 10 * (index - 232);
        rgb_to_hsla(grey, grey, grey)
    }

    fn corpus_theme(name: &str) -> crate::theme::TerminalTheme {
        crate::theme::theme_by_name(name).unwrap_or_else(|| panic!("preset {name} must exist"))
    }

    #[test]
    fn a_missing_corpus_fixture_fails_with_its_path() {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let missing = std::panic::catch_unwind(|| corpus_stream("does-not-exist.ansi"));
        std::panic::set_hook(previous);
        let payload = missing.expect_err("a missing fixture must fail the test");
        let message = payload
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default();
        assert!(
            message.contains("does-not-exist.ansi") && message.contains("fixtures"),
            "the failure must name the fixture path, got {message:?}"
        );
    }

    #[test]
    fn the_corpus_renders_text_on_every_preset() {
        for fixture in CORPUS_FIXTURES {
            let content = corpus_content(fixture);
            for preset in crate::theme::PRESETS {
                for name in [preset.light, preset.dark] {
                    let theme = corpus_theme(name);
                    let palette = ThemePalette::from_theme(&theme);
                    let state = corpus_layout(&content, &theme, &palette, 0.0);
                    let cells = corpus_cells(&content, &state, &theme);
                    assert!(
                        cells.len() > 40,
                        "{name}/{fixture}: the corpus must render text cells, got {}",
                        cells.len()
                    );
                }
            }
        }
    }

    #[test]
    fn light_presets_keep_indexed_corpus_text_legible_without_correction() {
        use std::fmt::Write as _;
        let mut measured = 0usize;
        let mut gap_indices = std::collections::BTreeSet::new();
        let mut gaps = std::collections::BTreeSet::new();
        for fixture in CORPUS_FIXTURES {
            let content = corpus_content(fixture);
            for preset in crate::theme::PRESETS {
                let theme = corpus_theme(preset.light);
                let palette = ThemePalette::from_theme(&theme);
                let state = corpus_layout(&content, &theme, &palette, 0.0);
                for cell in corpus_cells(&content, &state, &theme) {
                    let Some(Color::Indexed(index)) = cell.source else {
                        continue;
                    };
                    if index < 16 {
                        continue;
                    }
                    measured += 1;
                    assert!(
                        index < 232 || cell.lc >= CORPUS_TIER,
                        "{}/{fixture}: grey ramp index {index} must stay legible, Lc {:.1}",
                        preset.light,
                        cell.lc
                    );
                    if cell.lc < CORPUS_TIER {
                        gap_indices.insert(index);
                        gaps.insert(format!(
                            "{} {fixture} idx {index} Lc {:.1}",
                            preset.light, cell.lc
                        ));
                    }
                }
            }
        }
        assert!(
            measured > 300,
            "the corpus must measure indexed text on every light preset, got {measured}"
        );
        assert!(
            gap_indices.len() <= 1,
            "the render palette must leave at most one indexed color below Lc {CORPUS_TIER} \
             before any correction runs, got {gap_indices:?}"
        );
        let mut report = String::new();
        for gap in gaps {
            let _ = writeln!(report, "{gap}");
        }
        assert_golden_text("contrast_corpus_light_indexed_gap", report);
    }

    #[test]
    fn the_corpus_second_pass_is_served_by_the_contrast_cache() {
        let automatic = paneflow_config::schema::TerminalConfig::DEFAULT_MINIMUM_CONTRAST;
        let theme = corpus_theme("Paneflow Light");
        let palette = ThemePalette::from_theme(&theme);
        let contents: Vec<Content> = CORPUS_FIXTURES
            .iter()
            .map(|fixture| corpus_content(fixture))
            .collect();
        let mut attempt = 0usize;
        let (hits, lookups) = loop {
            let generation = crate::theme::theme_generation();
            for content in &contents {
                let _ = corpus_layout(content, &theme, &palette, automatic);
            }
            let _ = color::take_contrast_cache_stats();
            for content in &contents {
                let _ = corpus_layout(content, &theme, &palette, automatic);
            }
            let (hits, misses) = color::take_contrast_cache_stats();
            if crate::theme::theme_generation() == generation {
                break (hits, hits + misses);
            }
            attempt += 1;
            assert!(
                attempt < 5,
                "the theme generation kept moving during the measurement"
            );
        };
        assert!(
            lookups >= 50,
            "the corpus must reach the contrast cache, got {lookups} lookups"
        );
        let rate = hits as f64 / lookups as f64;
        assert!(
            rate >= 0.99,
            "the second pass must hit the cache, got {:.3} over {lookups} lookups",
            rate
        );
    }

    #[test]
    fn the_corpus_legibility_share_is_recorded_per_preset() {
        use std::fmt::Write as _;
        let automatic = paneflow_config::schema::TerminalConfig::DEFAULT_MINIMUM_CONTRAST;
        let mut report = String::new();
        for fixture in CORPUS_FIXTURES {
            let content = corpus_content(fixture);
            for preset in crate::theme::PRESETS {
                for name in [preset.light, preset.dark] {
                    let theme = corpus_theme(name);
                    let palette = ThemePalette::from_theme(&theme);
                    let state = corpus_layout(&content, &theme, &palette, 0.0);
                    let cells = corpus_cells(&content, &state, &theme);
                    let corrected_state = corpus_layout(&content, &theme, &palette, automatic);
                    let corrected = corpus_cells(&content, &corrected_state, &theme);
                    let total = cells.len();
                    let indexed = cells
                        .iter()
                        .filter(|cell| matches!(cell.source, Some(Color::Indexed(16..=255))))
                        .count();
                    let at_45 = cells.iter().filter(|cell| cell.lc >= 45.0).count();
                    let at_60 = cells.iter().filter(|cell| cell.lc >= 60.0).count();
                    let xterm_45 = cells.iter().filter(|cell| cell.baseline_lc >= 45.0).count();
                    let xterm_60 = cells.iter().filter(|cell| cell.baseline_lc >= 60.0).count();
                    let acc_45 = corrected.iter().filter(|cell| cell.lc >= 45.0).count();
                    let acc_60 = corrected.iter().filter(|cell| cell.lc >= 60.0).count();
                    let acc_indexed_60 = corrected
                        .iter()
                        .filter(|cell| matches!(cell.source, Some(Color::Indexed(16..=255))))
                        .filter(|cell| cell.lc >= 60.0)
                        .count();
                    let _ = writeln!(
                        report,
                        "{fixture} {name}: cells={total} indexed={indexed} lc45={at_45} \
                         lc60={at_60} xterm_lc45={xterm_45} xterm_lc60={xterm_60} \
                         acc_lc45={acc_45} acc_lc60={acc_60} acc_indexed_lc60={acc_indexed_60}"
                    );
                }
            }
        }
        assert_golden_text("contrast_corpus_share", report);
    }

    #[test]
    fn the_automatic_default_lifts_foreign_colors_and_spares_the_theme() {
        let automatic = paneflow_config::schema::TerminalConfig::DEFAULT_MINIMUM_CONTRAST;
        assert_eq!(automatic, 60.0);
        for fixture in CORPUS_FIXTURES {
            let content = corpus_content(fixture);
            for preset in crate::theme::PRESETS {
                for name in [preset.light, preset.dark] {
                    let theme = corpus_theme(name);
                    let palette = ThemePalette::from_theme(&theme);
                    let off = corpus_cells(
                        &content,
                        &corpus_layout(&content, &theme, &palette, 0.0),
                        &theme,
                    );
                    let on = corpus_cells(
                        &content,
                        &corpus_layout(&content, &theme, &palette, automatic),
                        &theme,
                    );
                    assert_eq!(off.len(), on.len(), "{name}/{fixture}: cell count");
                    for (off, on) in off.iter().zip(&on) {
                        assert_eq!((off.line, off.col), (on.line, on.col));
                        match on.source {
                            Some(Color::Spec(_)) | Some(Color::Indexed(16..=255)) => assert!(
                                on.lc >= automatic || on.lc >= off.lc,
                                "{name}/{fixture} L{} C{}: a foreign color must not lose \
                                 contrast, Lc {:.1} from {:.1}",
                                on.line,
                                on.col,
                                on.lc,
                                off.lc
                            ),
                            _ => assert_eq!(
                                on.color, off.color,
                                "{name}/{fixture} L{} C{}: the theme's own color must be \
                                 byte-identical with the correction on",
                                on.line, on.col
                            ),
                        }
                    }
                }
            }
        }
    }

    fn corpus_hue_drift(before: f32, after: f32) -> f32 {
        let gap = (before - after).abs();
        gap.min(360.0 - gap)
    }

    struct HarmonyRow {
        distance: f32,
        source_chroma: f32,
        realized_chroma: f32,
        hue_drift: f32,
    }

    fn lsd_harmony_rows(
        theme: &crate::theme::TerminalTheme,
        content: &Content,
    ) -> std::collections::BTreeMap<u8, HarmonyRow> {
        let automatic = paneflow_config::schema::TerminalConfig::DEFAULT_MINIMUM_CONTRAST;
        let palette = ThemePalette::from_theme(theme);
        let off = corpus_cells(
            content,
            &corpus_layout(content, theme, &palette, 0.0),
            theme,
        );
        let on = corpus_cells(
            content,
            &corpus_layout(content, theme, &palette, automatic),
            theme,
        );
        let foreground = color::oklab_of(theme.foreground);
        let dim = color::oklab_of(theme.dim_foreground);
        let mut rows = std::collections::BTreeMap::new();
        for (off, on) in off.iter().zip(&on) {
            let Some(Color::Indexed(index)) = off.source else {
                continue;
            };
            if index < 16 {
                continue;
            }
            let before = color::oklch_of(off.color);
            let after = color::oklch_of(on.color);
            let corrected = color::oklab_of(on.color);
            rows.insert(
                index,
                HarmonyRow {
                    distance: corrected.distance(foreground).min(corrected.distance(dim)),
                    source_chroma: before.c,
                    realized_chroma: after.c,
                    hue_drift: corpus_hue_drift(before.h, after.h),
                },
            );
        }
        rows
    }

    #[test]
    fn the_lsd_harmony_is_recorded_per_preset() {
        use std::fmt::Write as _;
        let content = corpus_content("lsd-la.ansi");
        let mut report = String::new();
        let mut measured = 0usize;
        for preset in crate::theme::PRESETS {
            for name in [preset.light, preset.dark] {
                let theme = corpus_theme(name);
                for (index, row) in lsd_harmony_rows(&theme, &content) {
                    measured += 1;
                    let _ = writeln!(
                        report,
                        "{name} idx {index}: dist {:.3} chroma {:.4} -> {:.4} hue_drift {:.2}",
                        row.distance, row.source_chroma, row.realized_chroma, row.hue_drift
                    );
                }
            }
        }
        assert!(
            measured > 100,
            "the harmony golden must cover the indexed columns of every variant, got {measured}"
        );
        assert_golden_text("contrast_corpus_harmony", report);
    }

    #[test]
    fn the_lsd_columns_keep_their_hue_on_the_theme_palette() {
        let content = corpus_content("lsd-la.ansi");
        let mut checked = 0usize;
        for preset in crate::theme::PRESETS {
            for name in [preset.light, preset.dark] {
                let theme = corpus_theme(name);
                let rows = lsd_harmony_rows(&theme, &content);
                for (index, tier) in [(230u8, 2.0f32), (187, 2.0), (229, 2.0), (40, 5.0)] {
                    let Some(row) = rows.get(&index) else {
                        panic!("{name}: the lsd fixture must carry index {index}");
                    };
                    checked += 1;
                    assert!(
                        row.hue_drift <= tier,
                        "{name} idx {index}: the theme palette needs no chroma reduction, so the \
                         hue must hold within {tier} degrees, drifted {:.2}",
                        row.hue_drift
                    );
                    assert!(
                        row.realized_chroma >= row.source_chroma * 0.6,
                        "{name} idx {index}: a pull here would collapse a distinct column onto \
                         the theme text color, chroma {:.4} -> {:.4}",
                        row.source_chroma,
                        row.realized_chroma
                    );
                }
            }
        }
        assert_eq!(checked, 40, "every variant must contribute four columns");
    }

    struct PullRow {
        source_chroma: f32,
        realized_chroma: f32,
        pulled: bool,
        distance: f32,
    }

    fn agent_pull_rows(
        theme: &crate::theme::TerminalTheme,
        content: &Content,
    ) -> std::collections::BTreeMap<(u8, u8, u8), PullRow> {
        let automatic = paneflow_config::schema::TerminalConfig::DEFAULT_MINIMUM_CONTRAST;
        let palette = ThemePalette::from_theme(theme);
        let off = corpus_cells(
            content,
            &corpus_layout(content, theme, &palette, 0.0),
            theme,
        );
        let on = corpus_cells(
            content,
            &corpus_layout(content, theme, &palette, automatic),
            theme,
        );
        let mut rows = std::collections::BTreeMap::new();
        for (off, on) in off.iter().zip(&on) {
            let Some(Color::Spec(rgb)) = off.source else {
                continue;
            };
            let plain = color::corrected_without_harmony(off.color, off.bg, automatic);
            let pulled = (plain.h, plain.s, plain.l) != (on.color.h, on.color.s, on.color.l);
            let target = color::nearest_theme_color(theme, plain);
            rows.insert(
                (rgb.r, rgb.g, rgb.b),
                PullRow {
                    source_chroma: color::oklch_of(off.color).c,
                    realized_chroma: color::oklch_of(plain).c,
                    pulled,
                    distance: color::oklab_of(on.color).distance(color::oklab_of(target)),
                },
            );
        }
        rows
    }

    #[test]
    fn the_truecolor_pull_is_recorded_per_preset() {
        use std::fmt::Write as _;
        let content = corpus_content("agent-cli.ansi");
        let mut report = String::new();
        let mut pulls = 0usize;
        for preset in crate::theme::PRESETS {
            for name in [preset.light, preset.dark] {
                let theme = corpus_theme(name);
                for ((r, g, b), row) in agent_pull_rows(&theme, &content) {
                    if row.pulled {
                        pulls += 1;
                    }
                    let ratio = if row.source_chroma > 0.0 {
                        row.realized_chroma / row.source_chroma
                    } else {
                        1.0
                    };
                    let _ = writeln!(
                        report,
                        "{name} rgb({r},{g},{b}): ratio {ratio:.2} pulled {} dist {:.3}",
                        row.pulled, row.distance
                    );
                }
            }
        }
        assert!(
            pulls > 0,
            "the agent fixture must exercise the harmony pull, otherwise the golden records              nothing and US-009 stays unjudged"
        );
        assert_golden_text("contrast_corpus_pull", report);
    }

    #[test]
    fn a_drained_truecolor_reaches_the_theme_through_the_layout() {
        let theme = corpus_theme("Paneflow Dark");
        let bg = default_background_color(&theme);
        let source = Color::Spec(Rgb {
            r: 255,
            g: 0,
            b: 255,
        });
        let cells = text_row(0, "magenta", source, CellFlags::empty());
        let uncorrected = only_run_color(&run_with_contrast(cells.clone(), &theme, 0.0));
        let corrected = only_run_color(&run_with_contrast(cells, &theme, 60.0));
        let plain = color::corrected_without_harmony(uncorrected, bg, 60.0);

        assert!(
            apca_contrast(corrected, bg).abs() >= 60.0,
            "the pulled color must meet the threshold"
        );
        assert_ne!(
            (corrected.h, corrected.s, corrected.l),
            (plain.h, plain.s, plain.l),
            "the layout must apply the theme pull, not the bare lightness move"
        );
        let target = color::nearest_theme_color(&theme, plain);
        assert!(
            color::oklab_of(corrected).distance(color::oklab_of(target))
                < color::oklab_of(plain).distance(color::oklab_of(target)),
            "the pull must land closer to its theme target than the bare correction"
        );
    }

    #[test]
    fn the_corpus_keeps_its_hues_through_the_perceptual_correction() {
        let automatic = paneflow_config::schema::TerminalConfig::DEFAULT_MINIMUM_CONTRAST;
        let mut checked = 0usize;
        for fixture in CORPUS_FIXTURES {
            let content = corpus_content(fixture);
            for preset in crate::theme::PRESETS {
                for name in [preset.light, preset.dark] {
                    let theme = corpus_theme(name);
                    let palette = ThemePalette::from_theme(&theme);
                    let off = corpus_cells(
                        &content,
                        &corpus_layout(&content, &theme, &palette, 0.0),
                        &theme,
                    );
                    let on = corpus_cells(
                        &content,
                        &corpus_layout(&content, &theme, &palette, automatic),
                        &theme,
                    );
                    for (off, on) in off.iter().zip(&on) {
                        let plain = if off.source.is_some_and(is_correctable_source) {
                            color::corrected_without_harmony(off.color, off.bg, automatic)
                        } else {
                            on.color
                        };
                        if (plain.h, plain.s, plain.l) != (on.color.h, on.color.s, on.color.l) {
                            continue;
                        }
                        let before = color::oklch_of(off.color);
                        let after = color::oklch_of(plain);
                        if before.c < 0.02 || after.c < before.c * 0.6 {
                            continue;
                        }
                        checked += 1;
                        let gap = (before.h - after.h).abs();
                        let drift = gap.min(360.0 - gap);
                        assert!(
                            drift <= 2.0,
                            "{name}/{fixture} L{} C{}: hue drifted {drift:.2} degrees without a \
                             chroma reduction",
                            on.line,
                            on.col
                        );
                    }
                }
            }
        }
        assert!(
            checked > 500,
            "the corpus must exercise chroma-preserving corrections, got {checked}"
        );
    }

    #[test]
    fn the_lsd_greens_reach_the_automatic_threshold_on_paneflow_dark() {
        let automatic = paneflow_config::schema::TerminalConfig::DEFAULT_MINIMUM_CONTRAST;
        let content = corpus_content("lsd-la.ansi");
        let theme = corpus_theme("Paneflow Dark");
        let palette = ThemePalette::from_theme(&theme);
        let off = corpus_cells(
            &content,
            &corpus_layout(&content, &theme, &palette, 0.0),
            &theme,
        );
        let on = corpus_cells(
            &content,
            &corpus_layout(&content, &theme, &palette, automatic),
            &theme,
        );
        let measured = off
            .iter()
            .zip(&on)
            .filter(|(off, _)| off.source == Some(Color::Indexed(40)))
            .count();
        assert!(
            measured > 0,
            "the lsd fixture must carry index 40 cells for this test to mean anything"
        );
        for (off, on) in off.iter().zip(&on) {
            if off.source != Some(Color::Indexed(40)) {
                continue;
            }
            assert!(
                off.lc < automatic,
                "L{} C{}: index 40 is the regression baseline, it must start below Lc \
                 {automatic}, got {:.1}",
                off.line,
                off.col,
                off.lc
            );
            assert!(
                on.lc >= automatic,
                "L{} C{}: index 40 must be lifted to Lc {automatic}, got {:.1}",
                on.line,
                on.col,
                on.lc
            );
        }
    }

    #[test]
    fn a_libghostty_palette_cell_reaches_the_pixel_through_the_render_palette() {
        let size = ghostty::WindowSize::new(CORPUS_COLS, CORPUS_ROWS, 8, 16)
            .expect("the corpus grid is valid");
        let mut terminal =
            ghostty::DisplayTerminal::new(size, 256, ghostty::TerminalAppearance::default())
                .expect("libghostty must initialize");
        terminal
            .feed(b"\x1b[38;5;200mmagenta\x1b[0m")
            .expect("the indexed sequence must parse");
        let snapshot = terminal.snapshot().expect("the snapshot must succeed");
        let content = crate::terminal::ghostty_session::CellMirror::default().publish(snapshot);
        assert!(
            content
                .cells
                .iter()
                .any(|cell| cell.fg == Color::Indexed(200)),
            "libghostty must report the palette index rather than a resolved color"
        );

        for name in ["Paneflow Light", "Paneflow Dark"] {
            let theme = corpus_theme(name);
            let palette = ThemePalette::from_theme(&theme);
            let state = corpus_layout(&content, &theme, &palette, 0.0);
            let run = state
                .batched_runs()
                .find(|run| run.text.contains("magenta"))
                .expect("the indexed run must be laid out");
            assert_eq!(
                run.color,
                palette.color(200),
                "{name}: index 200 must paint the render palette entry"
            );
        }
    }

    #[test]
    fn a_theme_change_never_serves_a_stale_palette_entry() {
        let cells: Arc<[Cell]> =
            text_row(0, "indexed", Color::Indexed(200), CellFlags::empty()).into();
        let light = corpus_theme("Paneflow Light");
        let light_palette = ThemePalette::with_generation(&light, 41);
        let dark = corpus_theme("Paneflow Dark");
        let dark_palette = ThemePalette::with_generation(&dark, 42);
        assert_ne!(
            light_palette.color(200),
            dark_palette.color(200),
            "the two presets must disagree for this test to mean anything"
        );

        let mut cache = RowLayoutCache::default();
        let first = layout_from_snapshot_cached(
            cached_inputs(cells.clone(), &light, &light_palette),
            &[1; ROWS],
            light_palette.generation(),
            &mut cache,
        );
        assert_eq!(
            first.batched_runs().next().expect("a run").color,
            light_palette.color(200)
        );

        let second = layout_from_snapshot_cached(
            cached_inputs(cells, &dark, &dark_palette),
            &[1; ROWS],
            dark_palette.generation(),
            &mut cache,
        );
        assert_eq!(
            second.batched_runs().next().expect("a run").color,
            dark_palette.color(200),
            "a new theme generation must re-resolve the indexed color"
        );
    }

    #[test]
    fn inverse_cells_swap_after_the_palette_resolves_both_sides() {
        let theme = corpus_theme("Paneflow Light");
        let palette = ThemePalette::from_theme(&theme);
        let cells = vec![cell(
            0,
            0,
            'z',
            Color::Indexed(200),
            Color::Indexed(33),
            CellFlags::INVERSE,
        )];
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
            palette: &palette,
            exited: None,
            exit_signal: None,
            integrated_glyphs_enabled: true,
            color_emoji_enabled: true,
            minimum_contrast: 0.0,
        });

        assert_eq!(
            state.batched_runs().next().expect("a run").color,
            palette.color(33),
            "the inverse foreground must be the palette entry of the cell background"
        );
        assert!(
            state
                .rects
                .iter()
                .any(|rect| rect.color == palette.color(200)),
            "the inverse background must be the palette entry of the cell foreground"
        );
    }
}
