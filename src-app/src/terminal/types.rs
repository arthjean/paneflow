//! Neutral type definitions shared between `terminal` (logic) and
//! `terminal_element` (rendering).
//!
//! Pulling these types out of `terminal_element.rs` breaks the circular
//! coupling where `terminal.rs` referenced `crate::terminal_element::…`
//! for hyperlink / search / copy-mode state. Both modules now depend on
//! this neutral leaf, allowing further decomposition (US-005 onward).
//!
//! ## Backend-neutral types (EP-003 / Zed #57483)
//!
//! This module is the primary **translation seam**: it is the UI-adjacent
//! file that owns backend conversions, and it owns the neutral
//! `Point` / `CursorShape` / `Color` / `CellFlags` / `Modes` / `SelectionRange`
//! / `Cell` / `Content` types plus their `From<alac>` conversions. Every other
//! rendering/input file should consume
//! these neutral types so a breaking `alacritty_terminal` bump ripples through
//! one module instead of the whole UI. Mirrors Zed's `terminal.rs:296-330`
//! neutral types + the `alacritty.rs` single-seam pattern.

use std::sync::Arc;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Point as AlacPoint;
use alacritty_terminal::selection::SelectionRange as AlacSelectionRange;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Term;
use alacritty_terminal::term::TermMode as AlacTermMode;
use alacritty_terminal::term::cell::Flags as AlacFlags;
use alacritty_terminal::vte::ansi::{
    Color as AlacColor, CursorShape as AlacCursorShape, NamedColor as AlacNamedColor,
};

use crate::terminal::ZedListener;

/// Shared terminal-grid handle - the single piece of cross-thread state. Aliased
/// in this seam module so the renderer can hold it without naming
/// `alacritty_terminal` directly (EP-003 confinement).
pub type SharedTerm = Arc<FairMutex<Term<ZedListener>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellQuoting {
    Posix,
    PowerShell,
    Cmd,
}

impl ShellQuoting {
    pub fn for_shell(shell: &str) -> Self {
        let basename = shell
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(shell)
            .to_ascii_lowercase();
        let key = basename.trim_end_matches(".exe");
        match key {
            "cmd" => Self::Cmd,
            "pwsh" | "powershell" => Self::PowerShell,
            "sh" | "bash" | "zsh" | "fish" | "dash" | "ksh" | "ash" | "mksh" => Self::Posix,
            _ => Self::default_for_platform(),
        }
    }

    #[cfg(windows)]
    pub const fn default_for_platform() -> Self {
        Self::PowerShell
    }

    #[cfg(not(windows))]
    pub const fn default_for_platform() -> Self {
        Self::Posix
    }
}

/// Last known terminal window metrics sent to terminal clients.
///
/// `cols` and `rows` drive the grid size. `cell_width` and `cell_height` are
/// needed by terminal size queries and platform PTY pixel fields, so callers
/// must treat changes to any field as a resize notification candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalWindowSize {
    pub cols: usize,
    pub rows: usize,
    pub cell_width: u16,
    pub cell_height: u16,
}

impl TerminalWindowSize {
    #[inline]
    pub const fn new(cols: usize, rows: usize, cell_width: u16, cell_height: u16) -> Self {
        Self {
            cols,
            rows,
            cell_width,
            cell_height,
        }
    }
}

/// Convert a measured pixel metric to the integer form used by PTY size
/// notifications and terminal size query replies.
#[inline]
pub fn terminal_metric_to_u16(value: f32) -> u16 {
    if !value.is_finite() {
        return 0;
    }
    value.round().clamp(0.0, u16::MAX as f32) as u16
}

// ---------------------------------------------------------------------------
// Neutral grid coordinate
// ---------------------------------------------------------------------------

/// A grid line index. Paneflow-owned mirror of `alacritty_terminal::index::Line`
/// (a `pub i32` newtype) - signed because alacritty's scrollback rows are
/// negative. Keeping the `.0` tuple shape lets callers that read `point.line.0`
/// migrate by swapping the import, not every field access.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Line(pub i32);

/// A grid column index. Paneflow-owned mirror of
/// `alacritty_terminal::index::Column` (a `pub usize` newtype).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Column(pub usize);

/// A grid position, Paneflow-owned mirror of `alacritty_terminal::index::Point`.
///
/// Depending on the producer, `line` is either grid-line coords (cursor) or
/// viewport-line coords (cells, after the `display_offset` shift). Ordering is
/// line-then-column, matching alacritty.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Point {
    pub line: Line,
    pub column: Column,
}

impl Point {
    /// Construct from raw line/column integers (the common call shape).
    #[inline]
    pub fn new(line: i32, column: usize) -> Self {
        Self {
            line: Line(line),
            column: Column(column),
        }
    }
}

impl From<AlacPoint> for Point {
    #[inline]
    fn from(p: AlacPoint) -> Self {
        Self::new(p.line.0, p.column.0)
    }
}

impl From<Point> for AlacPoint {
    #[inline]
    fn from(p: Point) -> Self {
        AlacPoint::new(
            alacritty_terminal::index::Line(p.line.0),
            alacritty_terminal::index::Column(p.column.0),
        )
    }
}

// ---------------------------------------------------------------------------
// Neutral cursor shape
// ---------------------------------------------------------------------------

/// Cursor rendering shape. Native variants mirror `vte::ansi::CursorShape`;
/// Paneflow adds Windows Terminal-style `Vintage` and `DoubleUnderline` for
/// user-configured defaults. The `From` conversion is exhaustive with no
/// wildcard arm, so a future upstream variant is caught at compile time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Vintage,
    Block,
    Underline,
    DoubleUnderline,
    Beam,
    HollowBlock,
    Hidden,
}

impl From<AlacCursorShape> for CursorShape {
    #[inline]
    fn from(s: AlacCursorShape) -> Self {
        match s {
            AlacCursorShape::Block => Self::Block,
            AlacCursorShape::Underline => Self::Underline,
            AlacCursorShape::Beam => Self::Beam,
            AlacCursorShape::HollowBlock => Self::HollowBlock,
            AlacCursorShape::Hidden => Self::Hidden,
        }
    }
}

// ---------------------------------------------------------------------------
// Neutral color
// ---------------------------------------------------------------------------

/// A 24-bit truecolor value, mirror of `vte::ansi::Rgb`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// Named palette slot, mirror of `vte::ansi::NamedColor` (exhaustive - the
/// alacritty enum has exactly these 29 variants, which is why the renderer's
/// `named_color` match needs no wildcard arm).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
    Foreground,
    BrightForeground,
    Background,
    DimBlack,
    DimRed,
    DimGreen,
    DimYellow,
    DimBlue,
    DimMagenta,
    DimCyan,
    DimWhite,
    DimForeground,
    Cursor,
}

/// A terminal cell color, mirror of `vte::ansi::Color`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    Named(NamedColor),
    Spec(Rgb),
    Indexed(u8),
}

impl From<alacritty_terminal::vte::ansi::Rgb> for Rgb {
    #[inline]
    fn from(c: alacritty_terminal::vte::ansi::Rgb) -> Self {
        Self {
            r: c.r,
            g: c.g,
            b: c.b,
        }
    }
}

impl From<AlacNamedColor> for NamedColor {
    #[inline]
    fn from(n: AlacNamedColor) -> Self {
        match n {
            AlacNamedColor::Black => Self::Black,
            AlacNamedColor::Red => Self::Red,
            AlacNamedColor::Green => Self::Green,
            AlacNamedColor::Yellow => Self::Yellow,
            AlacNamedColor::Blue => Self::Blue,
            AlacNamedColor::Magenta => Self::Magenta,
            AlacNamedColor::Cyan => Self::Cyan,
            AlacNamedColor::White => Self::White,
            AlacNamedColor::BrightBlack => Self::BrightBlack,
            AlacNamedColor::BrightRed => Self::BrightRed,
            AlacNamedColor::BrightGreen => Self::BrightGreen,
            AlacNamedColor::BrightYellow => Self::BrightYellow,
            AlacNamedColor::BrightBlue => Self::BrightBlue,
            AlacNamedColor::BrightMagenta => Self::BrightMagenta,
            AlacNamedColor::BrightCyan => Self::BrightCyan,
            AlacNamedColor::BrightWhite => Self::BrightWhite,
            AlacNamedColor::Foreground => Self::Foreground,
            AlacNamedColor::BrightForeground => Self::BrightForeground,
            AlacNamedColor::Background => Self::Background,
            AlacNamedColor::DimBlack => Self::DimBlack,
            AlacNamedColor::DimRed => Self::DimRed,
            AlacNamedColor::DimGreen => Self::DimGreen,
            AlacNamedColor::DimYellow => Self::DimYellow,
            AlacNamedColor::DimBlue => Self::DimBlue,
            AlacNamedColor::DimMagenta => Self::DimMagenta,
            AlacNamedColor::DimCyan => Self::DimCyan,
            AlacNamedColor::DimWhite => Self::DimWhite,
            AlacNamedColor::DimForeground => Self::DimForeground,
            AlacNamedColor::Cursor => Self::Cursor,
        }
    }
}

impl From<AlacColor> for Color {
    #[inline]
    fn from(c: AlacColor) -> Self {
        match c {
            AlacColor::Named(n) => Self::Named(n.into()),
            AlacColor::Spec(rgb) => Self::Spec(rgb.into()),
            AlacColor::Indexed(i) => Self::Indexed(i),
        }
    }
}

// ---------------------------------------------------------------------------
// Neutral cell attribute flags
// ---------------------------------------------------------------------------

/// Cell attribute flags, Paneflow-owned mirror of the `term::cell::Flags`
/// subset the renderer reads. Hand-rolled (no `bitflags` dep) - the API surface
/// the element needs is just `empty`/`contains`/`insert`/`|`. `BOLD_ITALIC` is
/// the combined mask, so `contains(BOLD_ITALIC)` requires *both* bits, matching
/// alacritty.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellFlags(u16);

impl CellFlags {
    pub const INVERSE: Self = Self(1 << 0);
    pub const BOLD: Self = Self(1 << 1);
    pub const ITALIC: Self = Self(1 << 2);
    pub const BOLD_ITALIC: Self = Self((1 << 1) | (1 << 2));
    pub const UNDERLINE: Self = Self(1 << 3);
    pub const DOUBLE_UNDERLINE: Self = Self(1 << 4);
    pub const UNDERCURL: Self = Self(1 << 5);
    pub const DOTTED_UNDERLINE: Self = Self(1 << 6);
    pub const DASHED_UNDERLINE: Self = Self(1 << 7);
    pub const STRIKEOUT: Self = Self(1 << 8);
    pub const DIM: Self = Self(1 << 9);
    pub const WIDE_CHAR: Self = Self(1 << 10);
    pub const WIDE_CHAR_SPACER: Self = Self(1 << 11);

    #[inline]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// `true` iff every bit set in `other` is also set in `self` (so the
    /// combined `BOLD_ITALIC` mask requires both bits, like alacritty).
    #[inline]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for CellFlags {
    type Output = Self;
    #[inline]
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for CellFlags {
    #[inline]
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl From<AlacFlags> for CellFlags {
    #[inline]
    fn from(f: AlacFlags) -> Self {
        let mut out = CellFlags::empty();
        // Map only the subset the renderer reads. Flags map individually, so a
        // cell carrying alacritty's BOLD + ITALIC ends up with both neutral
        // bits and `contains(BOLD_ITALIC)` is true.
        if f.contains(AlacFlags::INVERSE) {
            out |= CellFlags::INVERSE;
        }
        if f.contains(AlacFlags::BOLD) {
            out |= CellFlags::BOLD;
        }
        if f.contains(AlacFlags::ITALIC) {
            out |= CellFlags::ITALIC;
        }
        if f.contains(AlacFlags::UNDERLINE) {
            out |= CellFlags::UNDERLINE;
        }
        if f.contains(AlacFlags::DOUBLE_UNDERLINE) {
            out |= CellFlags::DOUBLE_UNDERLINE;
        }
        if f.contains(AlacFlags::UNDERCURL) {
            out |= CellFlags::UNDERCURL;
        }
        if f.contains(AlacFlags::DOTTED_UNDERLINE) {
            out |= CellFlags::DOTTED_UNDERLINE;
        }
        if f.contains(AlacFlags::DASHED_UNDERLINE) {
            out |= CellFlags::DASHED_UNDERLINE;
        }
        if f.contains(AlacFlags::STRIKEOUT) {
            out |= CellFlags::STRIKEOUT;
        }
        if f.contains(AlacFlags::DIM) {
            out |= CellFlags::DIM;
        }
        if f.contains(AlacFlags::WIDE_CHAR) {
            out |= CellFlags::WIDE_CHAR;
        }
        if f.contains(AlacFlags::WIDE_CHAR_SPACER) {
            out |= CellFlags::WIDE_CHAR_SPACER;
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Neutral terminal modes
// ---------------------------------------------------------------------------

/// Terminal private-mode flags, Paneflow-owned mirror of the `term::TermMode`
/// subset the neutral renderer/input layers read (the element gates IME on
/// `ALT_SCREEN`, `keys` picks app-cursor sequences, `mouse` picks the SGR/UTF-8
/// mouse encoding). Backend adapters translate their native mode bits into
/// this complete consumer-facing surface before any UI code sees them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes(u16);

impl Modes {
    pub const ALT_SCREEN: Self = Self(1 << 0);
    pub const APP_CURSOR: Self = Self(1 << 1);
    pub const SGR_MOUSE: Self = Self(1 << 2);
    pub const UTF8_MOUSE: Self = Self(1 << 3);
    pub const APP_KEYPAD: Self = Self(1 << 4);
    pub const BRACKETED_PASTE: Self = Self(1 << 5);
    pub const FOCUS_IN_OUT: Self = Self(1 << 6);
    pub const ALTERNATE_SCROLL: Self = Self(1 << 7);
    pub const MOUSE_REPORT_CLICK: Self = Self(1 << 8);
    pub const MOUSE_DRAG: Self = Self(1 << 9);
    pub const MOUSE_MOTION: Self = Self(1 << 10);
    pub const KITTY_KEYBOARD: Self = Self(1 << 11);
    pub const MOUSE_MODE: Self =
        Self(Self::MOUSE_REPORT_CLICK.0 | Self::MOUSE_DRAG.0 | Self::MOUSE_MOTION.0);

    #[inline]
    pub const fn empty() -> Self {
        Self(0)
    }

    #[inline]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    #[inline]
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

impl std::ops::BitOr for Modes {
    type Output = Self;
    #[inline]
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl From<AlacTermMode> for Modes {
    #[inline]
    fn from(m: AlacTermMode) -> Self {
        let mut out = Modes::empty();
        if m.contains(AlacTermMode::ALT_SCREEN) {
            out = out | Modes::ALT_SCREEN;
        }
        if m.contains(AlacTermMode::APP_CURSOR) {
            out = out | Modes::APP_CURSOR;
        }
        if m.contains(AlacTermMode::SGR_MOUSE) {
            out = out | Modes::SGR_MOUSE;
        }
        if m.contains(AlacTermMode::UTF8_MOUSE) {
            out = out | Modes::UTF8_MOUSE;
        }
        if m.contains(AlacTermMode::APP_KEYPAD) {
            out = out | Modes::APP_KEYPAD;
        }
        if m.contains(AlacTermMode::BRACKETED_PASTE) {
            out = out | Modes::BRACKETED_PASTE;
        }
        if m.contains(AlacTermMode::FOCUS_IN_OUT) {
            out = out | Modes::FOCUS_IN_OUT;
        }
        if m.contains(AlacTermMode::ALTERNATE_SCROLL) {
            out = out | Modes::ALTERNATE_SCROLL;
        }
        if m.contains(AlacTermMode::MOUSE_REPORT_CLICK) {
            out = out | Modes::MOUSE_REPORT_CLICK;
        }
        if m.contains(AlacTermMode::MOUSE_DRAG) {
            out = out | Modes::MOUSE_DRAG;
        }
        if m.contains(AlacTermMode::MOUSE_MOTION) {
            out = out | Modes::MOUSE_MOTION;
        }
        if m.intersects(AlacTermMode::KITTY_KEYBOARD_PROTOCOL) {
            out = out | Modes::KITTY_KEYBOARD;
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Neutral selection range
// ---------------------------------------------------------------------------

/// A computed selection span, Paneflow-owned mirror of
/// `alacritty_terminal::selection::SelectionRange`. `start`/`end` carry grid
/// coordinates (scrollback negative); `is_block` flags a rectangular selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionRange {
    pub start: Point,
    pub end: Point,
    pub is_block: bool,
}

/// Endpoint affinity for a selection update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionSide {
    Left,
    Right,
}

/// Selection expansion policy requested by the input layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionKind {
    Simple,
    Semantic,
    Lines,
}

/// Grid and viewport facts captured under one backend lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridMetrics {
    pub columns: usize,
    pub screen_lines: usize,
    pub display_offset: usize,
    pub topmost_line: Line,
    pub bottommost_line: Line,
    pub cursor: Point,
}

/// One logical terminal line copied for link detection.
pub struct GridLineText {
    pub line: Line,
    pub text: String,
    pub char_to_column: Vec<usize>,
}

impl From<AlacSelectionRange> for SelectionRange {
    #[inline]
    fn from(s: AlacSelectionRange) -> Self {
        Self {
            start: s.start.into(),
            end: s.end.into(),
            is_block: s.is_block,
        }
    }
}

// ---------------------------------------------------------------------------
// Neutral renderable cell + cursor + content snapshot (the seam output)
// ---------------------------------------------------------------------------

/// A single grid cell snapshotted as neutral value types under the `Term` lock
/// and handed to the Window-free layout pass. Carries no `Term` lock and no
/// GPUI/alacritty handle, so the layout pass is deterministic and testable with
/// no GPU/display (US-002 golden-frame net). Replaces the alacritty-typed
/// `CellSnapshot` (US-009).
#[derive(Clone, Debug)]
pub struct Cell {
    /// Viewport-line coordinates (scrollback rows negative), `display_offset`
    /// already applied by the producer.
    pub point: Point,
    pub c: char,
    pub fg: Color,
    pub bg: Color,
    pub flags: CellFlags,
    pub zerowidth: Option<Vec<char>>,
    /// Whether the cell carries an OSC 8 hyperlink. Only the boolean is
    /// snapshotted (the renderer just needs the underline affordance - alacritty
    /// 0.26 doesn't auto-set `UNDERLINE` on OSC 8 cells); the id/uri are read
    /// straight off the `Term` by the hover/click path in `input.rs`, so we
    /// avoid allocating two `String`s per OSC 8 cell every frame.
    pub hyperlink: bool,
}

/// The grid cursor as read under lock, before the element applies its
/// focus/visibility overrides (hidden when `!cursor_visible`, hollow when
/// unfocused) and the theme cursor color. `point` stays in raw grid-line
/// coords (no `display_offset` shift), matching the prior `build_layout`.
#[derive(Clone, Copy, Debug)]
pub struct RenderableCursor {
    pub point: Point,
    pub shape: CursorShape,
    /// Raw foreground of the cell under the cursor, before inverse-mode swap.
    pub fg: Color,
    /// Raw background of the cell under the cursor, before inverse-mode swap.
    pub bg: Color,
    pub flags: CellFlags,
    /// Whether the cell under the cursor is a wide (CJK) glyph.
    pub wide: bool,
    /// Char under the cursor (for the block-cursor inverse glyph).
    pub text: char,
    pub bold: bool,
    pub italic: bool,
}

/// A complete, neutral snapshot of the renderable terminal state - the output
/// of the single read seam ([`content_from_term`]). The element consumes this
/// instead of locking `Term` and importing alacritty types (US-009). Mirror of
/// Zed's `TerminalContent`.
#[derive(Clone, Debug)]
pub struct Content {
    /// Grid dimensions owned by this exact snapshot. Ghostty applies host
    /// resizes on its runtime thread, so these can briefly differ from the
    /// latest GPUI bounds while the resize transaction is in flight.
    pub cols: usize,
    pub rows: usize,
    pub cells: Arc<[Cell]>,
    pub cursor: RenderableCursor,
    pub selection: Option<SelectionRange>,
    pub display_offset: usize,
    pub history_size: usize,
}

/// The single read seam: lock-free snapshot of `Term` into neutral [`Content`].
///
/// Reproduces exactly what `TerminalElement::build_layout` read under lock -
/// cells in viewport coords (`display_offset` applied), the cursor in raw
/// grid coords plus its under-cursor cell attributes, the selection, and the
/// scroll/history metadata. The layout pass applies `display_offset` to that
/// raw cursor and hides it when the live cursor is outside the scrolled viewport.
/// This keeps the snapshot faithful to alacritty while preventing a floating
/// cursor during scrollback. Swapping the element onto this producer
/// (US-009) is a zero `LayoutState` delta change. The caller holds the lock;
/// this takes `&Term` so the same guard can also drive a resize.
#[allow(dead_code)]
pub fn content_from_term(term: &Term<ZedListener>) -> Content {
    content_from_term_rows(term, None)
}

/// Snapshot only the viewport rows needed by the renderer. Row bounds are in
/// viewport coordinates after `display_offset` is applied, matching
/// [`Content::cells`].
pub fn content_from_term_visible(
    term: &Term<ZedListener>,
    first_visible_row: i32,
    last_visible_row: i32,
) -> Content {
    content_from_term_rows(term, Some(first_visible_row..last_visible_row))
}

fn content_from_term_rows(
    term: &Term<ZedListener>,
    visible_rows: Option<std::ops::Range<i32>>,
) -> Content {
    let content = term.renderable_content();
    let display_offset = content.display_offset;
    let display_offset_i = display_offset as i32;

    // Transform grid-line coords (scrollback negative) into viewport-line coords
    // so culling, Y positioning, hyperlink zones, and batching all speak the
    // same coordinate system. The cursor intentionally stays raw below because
    // layout needs to know when it has scrolled out of the viewport.
    let cells: Arc<[Cell]> = content
        .display_iter
        .filter_map(|ic| {
            let line = ic.point.line.0 + display_offset_i;
            if visible_rows
                .as_ref()
                .is_some_and(|rows| line < rows.start || line >= rows.end)
            {
                return None;
            }
            Some(Cell {
                point: Point::new(line, ic.point.column.0),
                c: ic.cell.c,
                fg: ic.cell.fg.into(),
                bg: ic.cell.bg.into(),
                flags: ic.cell.flags.into(),
                zerowidth: ic
                    .cell
                    .zerowidth()
                    .filter(|characters| !characters.is_empty())
                    .map(<[_]>::to_vec),
                hyperlink: ic.cell.hyperlink().is_some(),
            })
        })
        .collect::<Vec<_>>()
        .into();

    let cur = &content.cursor;
    let cursor_cell = &term.grid()[cur.point];
    let cursor = RenderableCursor {
        // Raw grid-line coords (no display_offset), matching the prior cursor
        // snapshot in build_layout.
        point: Point::new(cur.point.line.0, cur.point.column.0),
        shape: cur.shape.into(),
        fg: cursor_cell.fg.into(),
        bg: cursor_cell.bg.into(),
        flags: cursor_cell.flags.into(),
        wide: cursor_cell.flags.contains(AlacFlags::WIDE_CHAR),
        text: cursor_cell.c,
        bold: cursor_cell.flags.contains(AlacFlags::BOLD)
            || cursor_cell.flags.contains(AlacFlags::BOLD_ITALIC),
        italic: cursor_cell.flags.contains(AlacFlags::ITALIC)
            || cursor_cell.flags.contains(AlacFlags::BOLD_ITALIC),
    };

    let selection = content.selection.map(SelectionRange::from);

    Content {
        cols: term.columns(),
        rows: term.screen_lines(),
        cells,
        cursor,
        selection,
        display_offset,
        history_size: term.history_size(),
    }
}

/// Resize the grid to `cols`×`rows` if it differs from the current size,
/// returning whether a resize actually happened (so the caller fires SIGWINCH
/// via the PTY notifier). Confines the `Dimensions`/`resize` call to this seam
/// module so the renderer's `build_layout` stays off `alacritty_terminal`.
pub fn resize_if_needed(term: &mut Term<ZedListener>, cols: usize, rows: usize) -> bool {
    if cols > 0 && rows > 0 && (term.columns() != cols || term.screen_lines() != rows) {
        term.resize(crate::terminal::SpikeTermSize {
            columns: cols,
            screen_lines: rows,
        });
        true
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Rendering glue types (Paneflow-owned, neutral)
// ---------------------------------------------------------------------------

/// A search match highlight to be painted by TerminalElement.
pub struct SearchHighlight {
    pub start: Point,
    pub end: Point,
    pub is_active: bool,
}

/// Where a hyperlink was detected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum HyperlinkSource {
    /// Explicit OSC 8 escape sequence from the program.
    Osc8,
    /// Regex pattern match on terminal output.
    Regex,
    /// Markdown file path (`.md` / `.markdown`) - opens in the in-pane
    /// markdown viewer via `TerminalEvent::OpenMarkdownPath`.
    FilePath,
    /// Source-code file path (`.rs`, `.ts`, `.py`, ...) optionally followed
    /// by `:line[:col]`. Opens in the user's `$VISUAL`/`$EDITOR` (or a probed
    /// fallback) via `TerminalEvent::OpenCodePath`. `uri` holds the resolved
    /// absolute path; `line` / `col` carry the optional location captured
    /// from `path:42` or `path:42:7` style references that compilers, test
    /// runners, and linters emit.
    CodePath,
}

/// A detected OSC 8 hyperlink zone spanning one or more cells.
/// Fields are populated here (US-014) and consumed by hover/click (US-015/US-016).
/// `Clone` (US-012): the press point's link is stashed on mouse-down so the
/// open can fire on mouse-up only if no drag occurred.
#[derive(Clone)]
#[allow(dead_code)]
pub struct HyperlinkZone {
    pub uri: String,
    pub id: String,
    pub start: Point,
    pub end: Point,
    /// Whether this URL's scheme is in the openable allowlist.
    pub is_openable: bool,
    /// How this hyperlink was detected (OSC 8 takes priority over regex).
    pub source: HyperlinkSource,
    /// 1-based line number for `CodePath` matches (`file.rs:42` → `Some(42)`).
    /// `None` for `Osc8`, `Regex`, `FilePath`, and `CodePath` with no `:line`
    /// suffix in the matched text.
    pub line: Option<u32>,
    /// 1-based column number for `CodePath` matches (`file.rs:42:7` →
    /// `Some(7)`). Always `None` when `line` is `None`.
    pub col: Option<u32>,
}

/// Copy mode cursor state for rendering.
pub struct CopyModeCursorState {
    /// Grid-coordinate line of the copy cursor (current/end of selection)
    pub grid_line: i32,
    /// Column of the copy cursor
    pub col: usize,
    /// Grid-coordinate line of the selection anchor (start), when a selection is active.
    /// Rendered as a distinct tmux-style marker so the user can see where the selection began.
    pub anchor_grid_line: Option<i32>,
    /// Column of the selection anchor.
    pub anchor_col: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_roundtrips_through_alac() {
        for (line, column) in [(0i32, 0usize), (5, 12), (-7, 3), (i32::MIN + 1, 200)] {
            let neutral = Point::new(line, column);
            let alac: AlacPoint = neutral.into();
            let back: Point = alac.into();
            assert_eq!(neutral, back);
            assert_eq!(alac.line.0, line);
            assert_eq!(alac.column.0, column);
        }
    }

    #[test]
    fn composite_mouse_mode_matches_any_reporting_mode() {
        assert!(Modes::MOUSE_REPORT_CLICK.intersects(Modes::MOUSE_MODE));
        assert!(Modes::MOUSE_DRAG.intersects(Modes::MOUSE_MODE));
        assert!(Modes::MOUSE_MOTION.intersects(Modes::MOUSE_MODE));
        assert!(!Modes::ALT_SCREEN.intersects(Modes::MOUSE_MODE));
    }

    #[test]
    fn cursor_shape_maps_every_variant() {
        use AlacCursorShape as A;
        assert_eq!(CursorShape::from(A::Block), CursorShape::Block);
        assert_eq!(CursorShape::from(A::Underline), CursorShape::Underline);
        assert_eq!(CursorShape::from(A::Beam), CursorShape::Beam);
        assert_eq!(CursorShape::from(A::HollowBlock), CursorShape::HollowBlock);
        assert_eq!(CursorShape::from(A::Hidden), CursorShape::Hidden);
    }

    #[test]
    fn color_maps_all_three_variants_losslessly() {
        // Named (every variant exercised via the 16 base + a few specials).
        assert_eq!(
            Color::from(AlacColor::Named(AlacNamedColor::Background)),
            Color::Named(NamedColor::Background)
        );
        assert_eq!(
            Color::from(AlacColor::Named(AlacNamedColor::Cursor)),
            Color::Named(NamedColor::Cursor)
        );
        // Spec (truecolor) - every channel preserved.
        let rgb = alacritty_terminal::vte::ansi::Rgb {
            r: 1,
            g: 254,
            b: 127,
        };
        assert_eq!(
            Color::from(AlacColor::Spec(rgb)),
            Color::Spec(Rgb {
                r: 1,
                g: 254,
                b: 127
            })
        );
        // Indexed - full byte range endpoints.
        assert_eq!(Color::from(AlacColor::Indexed(0)), Color::Indexed(0));
        assert_eq!(Color::from(AlacColor::Indexed(255)), Color::Indexed(255));
    }

    #[test]
    fn named_color_maps_every_alac_variant() {
        // Exhaustive over the alacritty enum: if a variant is ever added
        // upstream, the `From` match (no wildcard) fails to compile, flagging
        // the new color here.
        let all = [
            AlacNamedColor::Black,
            AlacNamedColor::Red,
            AlacNamedColor::Green,
            AlacNamedColor::Yellow,
            AlacNamedColor::Blue,
            AlacNamedColor::Magenta,
            AlacNamedColor::Cyan,
            AlacNamedColor::White,
            AlacNamedColor::BrightBlack,
            AlacNamedColor::BrightRed,
            AlacNamedColor::BrightGreen,
            AlacNamedColor::BrightYellow,
            AlacNamedColor::BrightBlue,
            AlacNamedColor::BrightMagenta,
            AlacNamedColor::BrightCyan,
            AlacNamedColor::BrightWhite,
            AlacNamedColor::Foreground,
            AlacNamedColor::BrightForeground,
            AlacNamedColor::Background,
            AlacNamedColor::DimBlack,
            AlacNamedColor::DimRed,
            AlacNamedColor::DimGreen,
            AlacNamedColor::DimYellow,
            AlacNamedColor::DimBlue,
            AlacNamedColor::DimMagenta,
            AlacNamedColor::DimCyan,
            AlacNamedColor::DimWhite,
            AlacNamedColor::DimForeground,
            AlacNamedColor::Cursor,
        ];
        // 29 variants, all convert without panic.
        assert_eq!(all.len(), 29);
        for n in all {
            let _: NamedColor = n.into();
        }
    }

    #[test]
    fn cell_flags_combined_bold_italic_requires_both_bits() {
        let bold_only = CellFlags::BOLD;
        assert!(bold_only.contains(CellFlags::BOLD));
        assert!(!bold_only.contains(CellFlags::BOLD_ITALIC));

        let both = CellFlags::BOLD | CellFlags::ITALIC;
        assert!(both.contains(CellFlags::BOLD));
        assert!(both.contains(CellFlags::ITALIC));
        assert!(both.contains(CellFlags::BOLD_ITALIC));

        assert!(CellFlags::empty().contains(CellFlags::empty()));
        assert!(!CellFlags::empty().contains(CellFlags::DIM));
    }

    #[test]
    fn cell_flags_map_from_alac_subset() {
        let mut alac = AlacFlags::INVERSE;
        alac.insert(AlacFlags::DIM);
        alac.insert(AlacFlags::WIDE_CHAR);
        let neutral: CellFlags = alac.into();
        assert!(neutral.contains(CellFlags::INVERSE));
        assert!(neutral.contains(CellFlags::DIM));
        assert!(neutral.contains(CellFlags::WIDE_CHAR));
        assert!(!neutral.contains(CellFlags::UNDERLINE));
        assert!(!neutral.contains(CellFlags::WIDE_CHAR_SPACER));
    }

    #[test]
    fn modes_map_consumed_subset() {
        let m = Modes::from(AlacTermMode::SGR_MOUSE | AlacTermMode::APP_CURSOR);
        assert!(m.contains(Modes::SGR_MOUSE));
        assert!(m.contains(Modes::APP_CURSOR));
        assert!(!m.contains(Modes::ALT_SCREEN));
        assert!(!m.contains(Modes::UTF8_MOUSE));

        let alt = Modes::from(AlacTermMode::ALT_SCREEN);
        assert!(alt.contains(Modes::ALT_SCREEN));
        assert!(!alt.contains(Modes::SGR_MOUSE));
    }

    #[test]
    fn kitty_mode_maps_when_any_protocol_flag_is_enabled() {
        for flag in [
            AlacTermMode::DISAMBIGUATE_ESC_CODES,
            AlacTermMode::REPORT_EVENT_TYPES,
            AlacTermMode::REPORT_ALTERNATE_KEYS,
            AlacTermMode::REPORT_ALL_KEYS_AS_ESC,
            AlacTermMode::REPORT_ASSOCIATED_TEXT,
        ] {
            assert!(Modes::from(flag).contains(Modes::KITTY_KEYBOARD));
        }
    }

    #[test]
    fn selection_range_roundtrips_coords() {
        let alac = AlacSelectionRange {
            start: AlacPoint::new(
                alacritty_terminal::index::Line(-3),
                alacritty_terminal::index::Column(4),
            ),
            end: AlacPoint::new(
                alacritty_terminal::index::Line(2),
                alacritty_terminal::index::Column(9),
            ),
            is_block: true,
        };
        let neutral: SelectionRange = alac.into();
        assert_eq!(neutral.start, Point::new(-3, 4));
        assert_eq!(neutral.end, Point::new(2, 9));
        assert!(neutral.is_block);
    }

    /// EP-003 / US-010 confinement guard: `alacritty_terminal` must only be
    /// imported by the backend seam (`types`), the Term-driving backend modules,
    /// and the one documented coordinate helper. The renderer (`element/*`),
    /// input encoding (`mouse`/`keys`), event handlers, and all app/UI code must
    /// go through the neutral types in this module. A new leak fails here with
    /// the offending `file:line` so it is caught at review, not at the next
    /// `alacritty_terminal` bump.
    #[test]
    fn alacritty_confined_to_backend_allowlist() {
        use std::path::{Path, PathBuf};

        // Paths are relative to `src-app/src/`, forward-slash normalized.
        const ALLOWLIST: &[&str] = &[
            "terminal/types.rs",          // native-to-neutral value translation
            "terminal/pty_session.rs",    // Alacritty adapter, PTY and event loop
            "terminal/listener.rs",       // Alacritty EventListener adapter
            "search.rs",                  // private grid search used only by the adapter
            "terminal/backend_corpus.rs", // Alacritty differential baseline tests only
        ];

        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut violations = Vec::new();
        let mut stack: Vec<PathBuf> = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let rel = path
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if ALLOWLIST.contains(&rel.as_str()) {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                for (i, line) in text.lines().enumerate() {
                    // Match real references (paths + imports), not doc-comment
                    // prose that merely names the crate.
                    if line.contains("alacritty_terminal::")
                        || line.contains("use alacritty_terminal")
                    {
                        violations.push(format!("{rel}:{}", i + 1));
                    }
                }
            }
        }

        assert!(
            violations.is_empty(),
            "alacritty_terminal leaked outside the EP-003 backend allowlist. Route \
             these through crate::terminal::types neutral types (or, if the file is \
             genuinely backend, add it to ALLOWLIST with a rationale):\n{}",
            violations.join("\n")
        );

        let opaque_handle_violations = ["app", "layout", "workspace", "terminal/element"]
            .into_iter()
            .flat_map(|relative_dir| {
                let base = root.join(relative_dir);
                let mut files = Vec::new();
                let mut dirs = vec![base];
                while let Some(dir) = dirs.pop() {
                    let Ok(entries) = std::fs::read_dir(dir) else {
                        continue;
                    };
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir() {
                            dirs.push(path);
                        } else if path.extension().and_then(|extension| extension.to_str())
                            == Some("rs")
                        {
                            files.push(path);
                        }
                    }
                }
                files
            })
            .filter_map(|path| {
                let text = std::fs::read_to_string(&path).ok()?;
                text.contains("SharedTerm").then(|| {
                    path.strip_prefix(&root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .replace('\\', "/")
                })
            })
            .collect::<Vec<_>>();
        assert!(
            opaque_handle_violations.is_empty(),
            "raw Alacritty terminal handles leaked across the backend facade:\n{}",
            opaque_handle_violations.join("\n")
        );

        let adapter = std::fs::read_to_string(root.join("terminal/pty_session.rs")).unwrap();
        assert!(
            !adapter.contains("pub term:") && !adapter.contains("pub notifier:"),
            "TerminalState exposed a concrete Alacritty handle; keep it private behind TerminalSessionBackend"
        );
    }
}
