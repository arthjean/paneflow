use std::sync::Arc;
use std::time::Instant;

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

    #[cfg(ghostty_native)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CellAttribute {
    Bold = 0,
    Italic = 1,
    Dim = 2,
    Inverse = 4,
    Invisible = 5,
    Strikethrough = 6,
    Overline = 7,
}

impl CellAttribute {
    const fn bit(self) -> u8 {
        1 << self as u8
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct CellFlags {
    attributes: u8,
    underline: UnderlineStyle,
}

impl CellFlags {
    pub(crate) const fn from_attribute_bits(attributes: u8, underline: UnderlineStyle) -> Self {
        Self {
            attributes,
            underline,
        }
    }

    pub const fn has(self, attribute: CellAttribute) -> bool {
        self.attributes & attribute.bit() != 0
    }

    pub const fn bold(self) -> bool {
        self.has(CellAttribute::Bold)
    }

    pub const fn dim(self) -> bool {
        self.has(CellAttribute::Dim)
    }

    pub const fn italic(self) -> bool {
        self.has(CellAttribute::Italic)
    }

    pub const fn inverse(self) -> bool {
        self.has(CellAttribute::Inverse)
    }

    pub const fn invisible(self) -> bool {
        self.has(CellAttribute::Invisible)
    }

    pub const fn strikethrough(self) -> bool {
        self.has(CellAttribute::Strikethrough)
    }

    pub const fn overline(self) -> bool {
        self.has(CellAttribute::Overline)
    }

    pub const fn underline(self) -> UnderlineStyle {
        self.underline
    }
}

impl std::fmt::Debug for CellFlags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CellFlags")
            .field("bold", &self.bold())
            .field("dim", &self.dim())
            .field("italic", &self.italic())
            .field("inverse", &self.inverse())
            .field("invisible", &self.invisible())
            .field("strikethrough", &self.strikethrough())
            .field("overline", &self.overline())
            .field("underline", &self.underline)
            .finish()
    }
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
    pub underline_color: Color,
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
    pub dirty_rows: Arc<[bool]>,
    pub cursor: Cursor,
    pub selection: Option<SelectionRange>,
    pub cols: usize,
    pub rows: usize,
    pub display_offset: usize,
    pub history_size: usize,
    pub mouse_shape: MouseShape,
    pub row_identities: Arc<[RowIdentity]>,
    pub row_shift: isize,
    pub revision: u64,
    pub overscan: Overscan,
    pub overscan_rows: Arc<[OverscanRow]>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RowId([u64; 2]);

impl RowId {
    #[cfg(ghostty_native)]
    pub(crate) fn from_bits(bits: [u64; 2]) -> Self {
        Self(bits)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowIdentity {
    pub viewport_y: i32,
    pub id: RowId,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Overscan {
    pub above: u16,
    pub below: u16,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalMemoryUsage {
    pub compression_supported: bool,
    pub primary_pages: u64,
    pub primary_virtual_bytes: u64,
    pub primary_resident_bytes: u64,
    pub primary_compressed_pages: u64,
    pub primary_compressed_bytes: u64,
    pub primary_image_bytes: u64,
    pub alternate_pages: u64,
    pub alternate_virtual_bytes: u64,
    pub alternate_resident_bytes: u64,
    pub alternate_compressed_pages: u64,
    pub alternate_compressed_bytes: u64,
    pub alternate_image_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverscanRow {
    pub identity: RowIdentity,
    pub cells: Arc<[Cell]>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseShape {
    Default,
    ContextMenu,
    Help,
    Pointer,
    Progress,
    Wait,
    Cell,
    Crosshair,
    #[default]
    Text,
    VerticalText,
    Alias,
    Copy,
    Move,
    NoDrop,
    NotAllowed,
    Grab,
    Grabbing,
    AllScroll,
    ColResize,
    RowResize,
    NResize,
    EResize,
    SResize,
    WResize,
    NeResize,
    NwResize,
    SeResize,
    SwResize,
    EwResize,
    NsResize,
    NeswResize,
    NwseResize,
    ZoomIn,
    ZoomOut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderHold {
    pub started_at: Instant,
    pub generation: u64,
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
    Bottom,
    Delta(i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressState {
    Remove,
    Set,
    Error,
    Indeterminate,
    Pause,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgressReport {
    pub state: ProgressState,
    pub percent: Option<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramStatusState {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
    Clear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramStatusKind {
    Permission,
    Question,
    Auth,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramStatusReport {
    pub state: ProgramStatusState,
    pub kind: Option<ProgramStatusKind>,
    pub progress: Option<u8>,
    pub id: String,
    pub app: String,
    pub title: String,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticPromptKind {
    PromptStart,
    InputStart,
    OutputStart,
    CommandEnd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    Primary,
    Right,
    Continuation,
    Secondary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OscTerminator {
    St,
    Bel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnknownSequenceKind {
    Apc,
    Osc(OscTerminator),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendEvent {
    WritePty(Vec<u8>),
    ClipboardStore(String),
    Bell,
    Title(String),
    WorkingDirectory(String),
    Progress(ProgressReport),
    ProgramStatus(ProgramStatusReport),
    SemanticPrompt {
        kind: SemanticPromptKind,
        prompt_kind: PromptKind,
        exit_code: Option<i32>,
    },
    Reset,
    DesktopNotification {
        title: String,
        body: String,
    },
    UnknownSequence {
        kind: UnknownSequenceKind,
        content: String,
        truncated: bool,
    },
    CallbackPanicked,
    InputDropped {
        bytes: usize,
    },
    EffectsOverflow {
        dropped_events: usize,
        dropped_bytes: usize,
    },
    ProgramStatusOverflow {
        dropped_events: usize,
        dropped_bytes: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_cell_stays_within_seven_machine_words() {
        assert!(
            std::mem::size_of::<Cell>() <= 7 * std::mem::size_of::<usize>(),
            "every publish walks the whole cached screen, so a Cell of {} bytes slows each frame",
            std::mem::size_of::<Cell>()
        );
    }
}
