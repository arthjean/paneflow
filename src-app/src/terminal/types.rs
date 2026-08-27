//! Neutral type definitions shared between `terminal` (logic) and
//! `terminal_element` (rendering).
//!
//! Pulling these types out of `terminal_element.rs` breaks the circular
//! coupling where `terminal.rs` referenced `crate::terminal_element::…`
//! for hyperlink / search / copy-mode state. Both modules now depend on
//! this neutral leaf, allowing further decomposition (US-005 onward).
//!
//! ## Backend-neutral types
//!
//! This module owns the neutral `Point` / `CursorShape` / `Color` /
//! `CellFlags` / `Modes` / `SelectionRange` / `Cell` / `Content` types the
//! renderer and the input encoders consume. The Ghostty engine translates its
//! own values into them inside `terminal/ghostty_session.rs`, so no
//! rendering, input, or app file ever names an engine type.

use std::sync::Arc;

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

/// A grid line index. Signed, because scrollback rows are negative: row `0`
/// is the top of the viewport and history grows downward from `-1`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Line(pub i32);

/// A grid column index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Column(pub usize);

/// A grid position.
///
/// Depending on the producer, `line` is either grid-line coords (cursor) or
/// viewport-line coords (cells, after the `display_offset` shift). Ordering is
/// line-then-column.
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

/// Named palette slot. Exhaustive, which is why the renderer's `named_color`
/// match needs no wildcard arm.
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
    Background,
}

/// A terminal cell color, mirror of `vte::ansi::Color`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    Named(NamedColor),
    Spec(Rgb),
    Indexed(u8),
}

// ---------------------------------------------------------------------------
// Neutral cell attribute flags
// ---------------------------------------------------------------------------

/// Cell attribute flags: the subset of a cell's SGR state the renderer reads.
/// Hand-rolled (no `bitflags` dep) - the API surface the element needs is just
/// `empty`/`contains`/`insert`/`|`. `BOLD_ITALIC` is the combined mask, so
/// `contains(BOLD_ITALIC)` requires *both* bits.
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
    /// combined `BOLD_ITALIC` mask requires both bits).
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

// ---------------------------------------------------------------------------
// Neutral selection range
// ---------------------------------------------------------------------------

/// A computed selection span. `start`/`end` carry grid coordinates (scrollback
/// negative); `is_block` flags a rectangular selection.
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

// ---------------------------------------------------------------------------
// Neutral renderable cell + cursor + content snapshot (the seam output)
// ---------------------------------------------------------------------------

/// A single grid cell snapshotted as neutral value types on the engine's
/// runtime thread and handed to the Window-free layout pass. Carries no engine
/// handle and no GPUI handle, so the layout pass is deterministic and testable
/// with no GPU and no display (US-002 golden-frame net).
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
    /// snapshotted (the renderer just needs the underline affordance); the
    /// id/uri are resolved on demand by the hover/click path in `input.rs`, so
    /// we avoid allocating two `String`s per OSC 8 cell every frame.
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

/// A complete, neutral snapshot of the renderable terminal state, produced by
/// the engine on its runtime thread. The element consumes this instead of
/// reaching for the grid itself. Mirror of Zed's `TerminalContent`.
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
    fn composite_mouse_mode_matches_any_reporting_mode() {
        assert!(Modes::MOUSE_REPORT_CLICK.intersects(Modes::MOUSE_MODE));
        assert!(Modes::MOUSE_DRAG.intersects(Modes::MOUSE_MODE));
        assert!(Modes::MOUSE_MOTION.intersects(Modes::MOUSE_MODE));
        assert!(!Modes::ALT_SCREEN.intersects(Modes::MOUSE_MODE));
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

    /// Ghostty is the only terminal engine. Alacritty was removed wholesale,
    /// so no file under `src-app/src/` may name `alacritty_terminal` again -
    /// re-introducing it would mean a second engine, a second set of grid
    /// semantics, and the neutral types in this module losing their single
    /// producer. The guard fails with the offending `file:line`.
    #[test]
    fn alacritty_is_absent_from_the_app_crate() {
        use std::path::{Path, PathBuf};

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
                // This file names the crate once, in the guard's own message.
                if rel == "terminal/types.rs" {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                for (i, line) in text.lines().enumerate() {
                    if line.contains("alacritty") {
                        violations.push(format!("{rel}:{}", i + 1));
                    }
                }
            }
        }

        assert!(
            violations.is_empty(),
            "alacritty came back into the app crate; Ghostty is the only engine:\n{}",
            violations.join("\n")
        );
    }
}
