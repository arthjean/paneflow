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

#[inline]
pub fn terminal_metric_to_u16(value: f32) -> u16 {
    if !value.is_finite() {
        return 0;
    }
    value.round().clamp(0.0, u16::MAX as f32) as u16
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Line(pub i32);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Column(pub usize);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Point {
    pub line: Line,
    pub column: Column,
}

impl Point {
    #[inline]
    pub fn new(line: i32, column: usize) -> Self {
        Self {
            line: Line(line),
            column: Column(column),
        }
    }
}

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    Named(NamedColor),
    Spec(Rgb),
    Indexed(u8),
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionRange {
    pub start: Point,
    pub end: Point,
    pub is_block: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectionGeometry {
    pub columns: usize,
    pub screen_lines: usize,
    pub display_offset: usize,
    pub cell_width: f32,
    pub line_height: f32,
}

impl SelectionGeometry {
    pub fn height(&self) -> f32 {
        self.line_height * self.screen_lines as f32
    }

    pub fn cell_at(&self, position: (f32, f32)) -> Point {
        let column = if self.cell_width > 0.0 {
            (position.0.max(0.0) / self.cell_width) as usize
        } else {
            0
        };
        let row = if self.line_height > 0.0 {
            (position.1.max(0.0) / self.line_height) as i32
        } else {
            0
        };
        Point::new(
            row.min(self.screen_lines.saturating_sub(1) as i32) - self.display_offset as i32,
            column.min(self.columns.saturating_sub(1)),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionKind {
    Simple,
    Semantic,
    Lines,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridMetrics {
    pub columns: usize,
    pub screen_lines: usize,
    pub display_offset: usize,
    pub topmost_line: Line,
    pub bottommost_line: Line,
    pub cursor: Point,
}

pub struct GridLineText {
    pub line: Line,
    pub text: String,
    pub char_to_column: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct Cell {
    pub point: Point,
    pub c: char,
    pub fg: Color,
    pub bg: Color,
    pub flags: CellFlags,
    pub zerowidth: Option<Vec<char>>,
    pub hyperlink: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct RenderableCursor {
    pub point: Point,
    pub shape: CursorShape,
    pub fg: Color,
    pub bg: Color,
    pub flags: CellFlags,
    pub wide: bool,
    pub text: char,
    pub bold: bool,
    pub italic: bool,
}

#[derive(Clone, Debug)]
pub struct Content {
    pub generation: u64,
    pub cols: usize,
    pub rows: usize,
    pub cells: Arc<[Cell]>,
    pub cursor: RenderableCursor,
    pub selection: Option<SelectionRange>,
    pub display_offset: usize,
    pub history_size: usize,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SearchHighlight {
    pub start: Point,
    pub end: Point,
    pub is_active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum HyperlinkSource {
    Osc8,
    Regex,
    FilePath,
    CodePath,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct HyperlinkZone {
    pub uri: String,
    pub id: String,
    pub start: Point,
    pub end: Point,
    pub is_openable: bool,
    pub source: HyperlinkSource,
    pub line: Option<u32>,
    pub col: Option<u32>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct CopyModeCursorState {
    pub grid_line: i32,
    pub col: usize,
    pub anchor_grid_line: Option<i32>,
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
