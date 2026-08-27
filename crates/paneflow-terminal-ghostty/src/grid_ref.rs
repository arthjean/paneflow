//! Per-cell and per-row metadata read through a grid reference.
//!
//! The render snapshot carries what the renderer draws. This is everything
//! else the grid knows about a position: soft-wrap state, OSC 133 semantic
//! prompt regions, protected cells, style identity. Selection correctness and
//! "select the output of this command" both depend on it.

use paneflow_libghostty_sys as sys;

use crate::batch::{Slot, get_multi};
use crate::engine::DisplayTerminal;
use crate::handles::check;
use crate::style::Style;
use crate::{GhosttyError, Point, Result, WideCell};

/// Where a row sits in an OSC 133 shell prompt.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SemanticPrompt {
    /// Not part of a prompt, or the shell never reported one.
    #[default]
    None,
    /// The first row of a prompt.
    Prompt,
    /// A continuation row of a multi-line prompt.
    PromptContinuation,
}

/// What a cell holds in an OSC 133 command cycle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SemanticContent {
    /// Command output.
    #[default]
    Output,
    /// Prompt text.
    Prompt,
    /// Text the user typed at the prompt.
    Input,
}

/// Row-level grid metadata.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RowInfo {
    /// The row soft-wraps into the next one.
    pub wrap: bool,
    /// The row continues a soft-wrapped row above it.
    pub wrap_continuation: bool,
    /// Some cell in the row carries a grapheme cluster.
    pub grapheme: bool,
    /// Some cell in the row is styled. May report a false positive.
    pub styled: bool,
    /// Some cell in the row carries a hyperlink. May report a false positive.
    pub hyperlink: bool,
    /// The row's prompt state, from OSC 133.
    pub semantic_prompt: SemanticPrompt,
    /// The row holds a Kitty virtual image placeholder.
    pub kitty_virtual_placeholder: bool,
    /// The row changed since the renderer last cleared its dirty bit.
    pub dirty: bool,
}

/// What kind of content a cell holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CellContent {
    /// A single codepoint.
    #[default]
    Codepoint,
    /// A codepoint with an attached grapheme cluster.
    Grapheme,
    /// No text: a palette-indexed background color only.
    BackgroundPalette,
    /// No text: a direct RGB background color only.
    BackgroundRgb,
}

/// Cell-level grid metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellInfo {
    /// What the cell holds.
    pub content: CellContent,
    /// The codepoint, or zero when the cell has no text.
    pub codepoint: u32,
    /// The cell's contribution to a wide character.
    pub wide: WideCell,
    /// The cell has text to render.
    pub has_text: bool,
    /// The cell carries non-default styling.
    pub has_styling: bool,
    /// Identifier for the cell's style, stable within a screen.
    pub style_id: u16,
    /// The cell carries a hyperlink.
    pub has_hyperlink: bool,
    /// The cell is protected from selective erase (DECSCA).
    pub protected: bool,
    /// The cell's role in an OSC 133 command cycle.
    pub semantic_content: SemanticContent,
}

impl DisplayTerminal {
    /// Row metadata for the logical line at `line`, where zero is the first
    /// viewport row and negative values address scrollback.
    pub fn row_info(&self, line: i32) -> Result<RowInfo> {
        let reference = self.grid_ref(Point::new(line, 0))?;
        let mut row: sys::GhosttyRow = 0;
        // SAFETY: `reference` is a live grid ref and `row` is valid writable
        // storage for the out-parameter.
        let result = unsafe { sys::ghostty_grid_ref_row(&raw const reference, &mut row) };
        check("grid_ref_row", result)?;

        let mut info = RowInfo::default();
        let mut semantic = sys::GhosttyRowSemanticPrompt_GHOSTTY_ROW_SEMANTIC_NONE;
        use sys as s;
        // SAFETY: every destination matches the output type screen.h
        // documents for its key, and all of them outlive the call.
        unsafe {
            get_multi(
                "row_get_multi",
                row,
                sys::ghostty_row_get_multi,
                [
                    Slot::new(s::GhosttyRowData_GHOSTTY_ROW_DATA_WRAP, &mut info.wrap),
                    Slot::new(
                        s::GhosttyRowData_GHOSTTY_ROW_DATA_WRAP_CONTINUATION,
                        &mut info.wrap_continuation,
                    ),
                    Slot::new(
                        s::GhosttyRowData_GHOSTTY_ROW_DATA_GRAPHEME,
                        &mut info.grapheme,
                    ),
                    Slot::new(s::GhosttyRowData_GHOSTTY_ROW_DATA_STYLED, &mut info.styled),
                    Slot::new(
                        s::GhosttyRowData_GHOSTTY_ROW_DATA_HYPERLINK,
                        &mut info.hyperlink,
                    ),
                    Slot::new(
                        s::GhosttyRowData_GHOSTTY_ROW_DATA_SEMANTIC_PROMPT,
                        &mut semantic,
                    ),
                    Slot::new(
                        s::GhosttyRowData_GHOSTTY_ROW_DATA_KITTY_VIRTUAL_PLACEHOLDER,
                        &mut info.kitty_virtual_placeholder,
                    ),
                    Slot::new(s::GhosttyRowData_GHOSTTY_ROW_DATA_DIRTY, &mut info.dirty),
                ],
            )?;
        }
        info.semantic_prompt = semantic_prompt(semantic)?;
        Ok(info)
    }

    /// Read one row field on its own, for callers that need a single bit
    /// without paying for the full [`RowInfo`] batch.
    pub fn row_wraps(&self, line: i32) -> Result<bool> {
        let reference = self.grid_ref(Point::new(line, 0))?;
        let mut row: sys::GhosttyRow = 0;
        // SAFETY: `reference` is live and `row` is valid writable storage.
        let result = unsafe { sys::ghostty_grid_ref_row(&raw const reference, &mut row) };
        check("grid_ref_row", result)?;
        let mut wrap = false;
        // SAFETY: `GHOSTTY_ROW_DATA_WRAP` writes a `bool`, which is what
        // `wrap` provides, and `row` is live.
        let result = unsafe {
            sys::ghostty_row_get(
                row,
                sys::GhosttyRowData_GHOSTTY_ROW_DATA_WRAP,
                (&raw mut wrap).cast(),
            )
        };
        check("row_get", result)?;
        Ok(wrap)
    }

    /// Cell metadata at `point`.
    pub fn cell_info(&self, point: Point) -> Result<CellInfo> {
        let reference = self.grid_ref(point)?;
        let mut cell: sys::GhosttyCell = 0;
        // SAFETY: `reference` is live and `cell` is valid writable storage.
        let result = unsafe { sys::ghostty_grid_ref_cell(&raw const reference, &mut cell) };
        check("grid_ref_cell", result)?;

        let mut content_tag = sys::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_CODEPOINT;
        let mut wide = sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_NARROW;
        let mut semantic = sys::GhosttyCellSemanticContent_GHOSTTY_CELL_SEMANTIC_OUTPUT;
        let mut codepoint = 0u32;
        let mut has_text = false;
        let mut has_styling = false;
        let mut style_id = 0u16;
        let mut has_hyperlink = false;
        let mut protected = false;
        use sys as s;
        // SAFETY: every destination matches the output type screen.h
        // documents for its key, and all of them outlive the call.
        unsafe {
            get_multi(
                "cell_get_multi",
                cell,
                sys::ghostty_cell_get_multi,
                [
                    Slot::new(
                        s::GhosttyCellData_GHOSTTY_CELL_DATA_CONTENT_TAG,
                        &mut content_tag,
                    ),
                    Slot::new(
                        s::GhosttyCellData_GHOSTTY_CELL_DATA_CODEPOINT,
                        &mut codepoint,
                    ),
                    Slot::new(s::GhosttyCellData_GHOSTTY_CELL_DATA_WIDE, &mut wide),
                    Slot::new(s::GhosttyCellData_GHOSTTY_CELL_DATA_HAS_TEXT, &mut has_text),
                    Slot::new(
                        s::GhosttyCellData_GHOSTTY_CELL_DATA_HAS_STYLING,
                        &mut has_styling,
                    ),
                    Slot::new(s::GhosttyCellData_GHOSTTY_CELL_DATA_STYLE_ID, &mut style_id),
                    Slot::new(
                        s::GhosttyCellData_GHOSTTY_CELL_DATA_HAS_HYPERLINK,
                        &mut has_hyperlink,
                    ),
                    Slot::new(
                        s::GhosttyCellData_GHOSTTY_CELL_DATA_PROTECTED,
                        &mut protected,
                    ),
                    Slot::new(
                        s::GhosttyCellData_GHOSTTY_CELL_DATA_SEMANTIC_CONTENT,
                        &mut semantic,
                    ),
                ],
            )?;
        }
        Ok(CellInfo {
            content: cell_content(content_tag)?,
            codepoint,
            wide: crate::snapshot_ffi::wide_cell(wide)?,
            has_text,
            has_styling,
            style_id,
            has_hyperlink,
            protected,
            semantic_content: semantic_content(semantic)?,
        })
    }

    /// The full style at `point`.
    pub fn cell_style(&self, point: Point) -> Result<Style> {
        let reference = self.grid_ref(point)?;
        let mut style = std::mem::MaybeUninit::<sys::GhosttyStyle>::uninit();
        // SAFETY: `reference` is live and the callee fully initializes the
        // style it is handed, including its leading `size` field.
        let style = unsafe {
            let result = sys::ghostty_grid_ref_style(&raw const reference, style.as_mut_ptr());
            check("grid_ref_style", result)?;
            style.assume_init()
        };
        Ok(Style::from_raw(style))
    }
}

fn cell_content(value: sys::GhosttyCellContentTag) -> Result<CellContent> {
    match value {
        sys::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_CODEPOINT => Ok(CellContent::Codepoint),
        sys::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_CODEPOINT_GRAPHEME => {
            Ok(CellContent::Grapheme)
        }
        sys::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_BG_COLOR_PALETTE => {
            Ok(CellContent::BackgroundPalette)
        }
        sys::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_BG_COLOR_RGB => {
            Ok(CellContent::BackgroundRgb)
        }
        other => Err(GhosttyError::AbiMismatch(format!(
            "unknown Ghostty cell content tag {other}"
        ))),
    }
}

fn semantic_prompt(value: sys::GhosttyRowSemanticPrompt) -> Result<SemanticPrompt> {
    match value {
        sys::GhosttyRowSemanticPrompt_GHOSTTY_ROW_SEMANTIC_NONE => Ok(SemanticPrompt::None),
        sys::GhosttyRowSemanticPrompt_GHOSTTY_ROW_SEMANTIC_PROMPT => Ok(SemanticPrompt::Prompt),
        sys::GhosttyRowSemanticPrompt_GHOSTTY_ROW_SEMANTIC_PROMPT_CONTINUATION => {
            Ok(SemanticPrompt::PromptContinuation)
        }
        other => Err(GhosttyError::AbiMismatch(format!(
            "unknown Ghostty row semantic prompt {other}"
        ))),
    }
}

fn semantic_content(value: sys::GhosttyCellSemanticContent) -> Result<SemanticContent> {
    match value {
        sys::GhosttyCellSemanticContent_GHOSTTY_CELL_SEMANTIC_OUTPUT => Ok(SemanticContent::Output),
        sys::GhosttyCellSemanticContent_GHOSTTY_CELL_SEMANTIC_PROMPT => Ok(SemanticContent::Prompt),
        sys::GhosttyCellSemanticContent_GHOSTTY_CELL_SEMANTIC_INPUT => Ok(SemanticContent::Input),
        other => Err(GhosttyError::AbiMismatch(format!(
            "unknown Ghostty cell semantic content {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TerminalAppearance, WindowSize};

    fn terminal(cols: usize, rows: usize) -> DisplayTerminal {
        let size = WindowSize::new(cols, rows, 8, 16).expect("valid terminal size");
        DisplayTerminal::new(size, 100, TerminalAppearance::default())
            .expect("terminal must initialize")
    }

    #[test]
    fn a_soft_wrapped_row_reports_wrap_and_its_continuation() {
        let mut terminal = terminal(4, 4);
        terminal.feed(b"abcdef").expect("output must parse");

        let first = terminal.row_info(0).expect("first row");
        assert!(first.wrap);
        assert!(!first.wrap_continuation);
        assert!(terminal.row_wraps(0).expect("single-field read"));

        let second = terminal.row_info(1).expect("second row");
        assert!(!second.wrap);
        assert!(second.wrap_continuation);
        assert!(!terminal.row_wraps(1).expect("single-field read"));
    }

    #[test]
    fn cell_metadata_tracks_text_styling_and_width() {
        let mut terminal = terminal(10, 2);
        terminal.feed(b"a\x1b[1mb\x1b[0m").expect("output must parse");

        let plain = terminal.cell_info(Point::new(0, 0)).expect("plain cell");
        assert_eq!(plain.codepoint, u32::from('a'));
        assert!(plain.has_text);
        assert!(!plain.has_styling);
        assert_eq!(plain.wide, WideCell::Narrow);
        assert!(!plain.protected);

        let bold = terminal.cell_info(Point::new(0, 1)).expect("bold cell");
        assert_eq!(bold.codepoint, u32::from('b'));
        assert!(bold.has_styling);
        assert_ne!(bold.style_id, plain.style_id);
    }

    #[test]
    fn a_wide_character_marks_its_spacer_tail() {
        let mut terminal = terminal(10, 2);
        terminal.feed("世".as_bytes()).expect("output must parse");

        let head = terminal.cell_info(Point::new(0, 0)).expect("wide head");
        assert_eq!(head.wide, WideCell::Wide);
        let tail = terminal.cell_info(Point::new(0, 1)).expect("wide tail");
        assert_eq!(tail.wide, WideCell::SpacerTail);
    }

    #[test]
    fn styles_read_through_a_grid_ref_match_what_was_written() {
        let mut terminal = terminal(10, 2);
        terminal
            .feed(b"\x1b[1;3;38;2;255;0;0mx\x1b[0m")
            .expect("output must parse");

        let style = terminal.cell_style(Point::new(0, 0)).expect("styled cell");
        assert!(!style.is_default());
        let flags = style.flags().expect("flags");
        assert!(flags.bold);
        assert!(flags.italic);
        assert_eq!(
            style.foreground().expect("foreground"),
            crate::Color::Rgb(crate::Rgb { r: 255, g: 0, b: 0 })
        );

        let blank = terminal.cell_style(Point::new(0, 5)).expect("blank cell");
        assert!(blank.is_default());
    }

    #[test]
    fn osc133_marks_the_prompt_row_and_its_cells() {
        let mut terminal = terminal(20, 4);
        terminal
            .feed(b"\x1b]133;A\x07$ \x1b]133;B\x07ls\x1b]133;C\x07\r\nout")
            .expect("output must parse");

        let prompt = terminal.row_info(0).expect("prompt row");
        assert_eq!(prompt.semantic_prompt, SemanticPrompt::Prompt);
        assert_eq!(
            terminal
                .cell_info(Point::new(0, 0))
                .expect("prompt cell")
                .semantic_content,
            SemanticContent::Prompt
        );
        assert_eq!(
            terminal
                .cell_info(Point::new(0, 2))
                .expect("input cell")
                .semantic_content,
            SemanticContent::Input
        );

        let output = terminal.row_info(1).expect("output row");
        assert_eq!(output.semantic_prompt, SemanticPrompt::None);
    }
}
