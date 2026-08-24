use std::sync::Arc;

use crate::{GhosttyError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: u32,
    pub cell_height: u32,
}

impl WindowSize {
    pub fn new(cols: usize, rows: usize, cell_width: u32, cell_height: u32) -> Result<Self> {
        let Ok(cols) = u16::try_from(cols) else {
            return Err(GhosttyError::InvalidDimensions {
                cols,
                rows,
                max: u16::MAX,
            });
        };
        let Ok(rows_u16) = u16::try_from(rows) else {
            return Err(GhosttyError::InvalidDimensions {
                cols: usize::from(cols),
                rows,
                max: u16::MAX,
            });
        };
        if cols == 0 || rows_u16 == 0 {
            return Err(GhosttyError::InvalidDimensions {
                cols: usize::from(cols),
                rows: usize::from(rows_u16),
                max: u16::MAX,
            });
        }
        Ok(Self {
            cols,
            rows: rows_u16,
            cell_width,
            cell_height,
        })
    }

    #[cfg(all(
        feature = "native",
        any(
            target_os = "linux",
            all(target_os = "windows", target_arch = "x86_64", target_env = "msvc")
        )
    ))]
    pub(crate) fn validate(self) -> Result<Self> {
        if self.cols == 0 || self.rows == 0 {
            return Err(GhosttyError::InvalidDimensions {
                cols: usize::from(self.cols),
                rows: usize::from(self.rows),
                max: u16::MAX,
            });
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorScheme {
    Light,
    #[default]
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalAppearance {
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    pub color_scheme: ColorScheme,
}

impl TerminalAppearance {
    pub const fn new(
        foreground: Rgb,
        background: Rgb,
        cursor: Rgb,
        color_scheme: ColorScheme,
    ) -> Self {
        Self {
            foreground,
            background,
            cursor,
            color_scheme,
        }
    }
}

impl Default for TerminalAppearance {
    fn default() -> Self {
        Self {
            foreground: Rgb {
                r: 0xdd,
                g: 0xdd,
                b: 0xdd,
            },
            background: Rgb {
                r: 0x11,
                g: 0x11,
                b: 0x11,
            },
            cursor: Rgb {
                r: 0xff,
                g: 0xff,
                b: 0xff,
            },
            color_scheme: ColorScheme::Dark,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Color {
    #[default]
    Default,
    Palette(u8),
    Rgb(Rgb),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Point {
    pub line: i32,
    pub column: usize,
}

impl Point {
    pub const fn new(line: i32, column: usize) -> Self {
        Self { line, column }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UnderlineStyle {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WideCell {
    #[default]
    Narrow,
    Wide,
    SpacerTail,
    SpacerHead,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellFlags {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    pub overline: bool,
    pub underline: UnderlineStyle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub point: Point,
    pub character: char,
    pub zerowidth: Option<Box<[char]>>,
    pub foreground: Color,
    pub background: Color,
    pub flags: CellFlags,
    pub wide: WideCell,
    pub selected: bool,
    pub hyperlink: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorShape {
    Bar,
    #[default]
    Block,
    Underline,
    HollowBlock,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub point: Point,
    pub shape: CursorShape,
    pub visible: bool,
    pub blinking: bool,
    pub wide_tail: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionRange {
    pub start: Point,
    pub end: Point,
    pub rectangle: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Content {
    pub cells: Arc<[Cell]>,
    pub cursor: Cursor,
    pub selection: Option<SelectionRange>,
    pub cols: usize,
    pub rows: usize,
    pub display_offset: usize,
    pub history_size: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    pub alternate_screen: bool,
    pub application_cursor: bool,
    pub application_keypad: bool,
    pub bracketed_paste: bool,
    pub focus_reporting: bool,
    pub alternate_scroll: bool,
    pub mouse_report_click: bool,
    pub mouse_drag: bool,
    pub mouse_motion: bool,
    pub sgr_mouse: bool,
    pub utf8_mouse: bool,
    pub kitty_keyboard: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchMatch {
    pub start: Point,
    pub end: Point,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchResult {
    pub matches: Vec<SearchMatch>,
    pub regex_error: Option<String>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hyperlink {
    pub point: Point,
    pub uri: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scroll {
    Top,
    Bottom,
    /// Relative viewport motion in Paneflow/Alacritty coordinates: positive
    /// moves up into history, negative moves down toward the live bottom.
    Delta(i32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendEvent {
    WritePty(Vec<u8>),
    ClipboardStore(String),
    Bell,
    Title(String),
    WorkingDirectory(String),
    CallbackPanicked,
    InputDropped {
        bytes: usize,
    },
    EffectsOverflow {
        dropped_events: usize,
        dropped_bytes: usize,
    },
}
