use paneflow_libghostty_sys as sys;

use crate::handles::check;
use crate::{GhosttyError, Result, Rgb, UnderlineStyle, WideCell};

const MAX_GRAPHEME_CODEPOINTS: usize = 1024;
const INLINE_GRAPHEME_CODEPOINTS: usize = 16;

mod sealed {
    pub trait Sealed {}
}

pub(crate) trait TerminalField: sealed::Sealed {
    type Value: Default;
    const DATA: sys::GhosttyTerminalData;
}

pub(crate) trait RenderField: sealed::Sealed {
    type Value: Default;
    const DATA: sys::GhosttyRenderStateData;
}

macro_rules! terminal_fields {
    ($($field:ident: $value:ty = $data:expr),+ $(,)?) => {
        $(
            pub(crate) struct $field;
            impl sealed::Sealed for $field {}
            impl TerminalField for $field {
                type Value = $value;
                const DATA: sys::GhosttyTerminalData = $data;
            }
        )+
    };
}

macro_rules! render_fields {
    ($($field:ident: $value:ty = $data:expr),+ $(,)?) => {
        $(
            pub(crate) struct $field;
            impl sealed::Sealed for $field {}
            impl RenderField for $field {
                type Value = $value;
                const DATA: sys::GhosttyRenderStateData = $data;
            }
        )+
    };
}

terminal_fields! {
    TerminalKittyKeyboardFlags: u8 = sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS,
    TerminalCursorX: u16 = sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_CURSOR_X,
    TerminalCursorY: u16 = sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_CURSOR_Y,
    TerminalTotalRows: usize = sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_TOTAL_ROWS,
    TerminalScrollbackRows: usize = sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS,
    TerminalCols: u16 = sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_COLS,
}

render_fields! {
    RenderCols: u16 = sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_COLS,
    RenderRows: u16 = sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_ROWS,
    RenderDirty: sys::GhosttyRenderStateDirty = sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_DIRTY,
    RenderCursorVisible: bool = sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VISIBLE,
    RenderCursorBlinking: bool = sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_BLINKING,
    RenderCursorViewportHasValue: bool = sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_HAS_VALUE,
    RenderCursorVisualStyle: sys::GhosttyRenderStateCursorVisualStyle = sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VISUAL_STYLE,
    RenderCursorViewportX: u16 = sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_X,
    RenderCursorViewportY: u16 = sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_Y,
    RenderCursorViewportWideTail: bool = sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_WIDE_TAIL,
}

pub(crate) struct RenderRowData {
    pub(crate) dirty: bool,
    pub(crate) cells: sys::GhosttyRenderStateRowCells,
    pub(crate) selection: Option<sys::GhosttyRenderStateRowSelection>,
}

pub(crate) struct RenderCellData {
    pub(crate) raw: sys::GhosttyCell,
    pub(crate) style: sys::GhosttyStyle,
}

pub(crate) struct RawCellData {
    pub(crate) codepoint: u32,
    pub(crate) wide: sys::GhosttyCellWide,
    pub(crate) has_hyperlink: bool,
    pub(crate) content_tag: sys::GhosttyCellContentTag,
}

pub(crate) fn ghostty_point(
    tag: sys::GhosttyPointTag,
    row: usize,
    column: usize,
) -> Result<sys::GhosttyPoint> {
    let x = u16::try_from(column).map_err(|_| GhosttyError::InvalidDimensions {
        cols: column,
        rows: row,
        max: u16::MAX,
    })?;
    let y = u32::try_from(row).map_err(|_| GhosttyError::LimitExceeded {
        resource: "grid row",
        limit: u32::MAX as usize,
    })?;
    Ok(sys::GhosttyPoint {
        tag,
        value: sys::GhosttyPointValue {
            coordinate: sys::GhosttyPointCoordinate { x, y },
        },
    })
}

pub(crate) fn copy_buffer(
    resource: &'static str,
    cap: usize,
    mut read: impl FnMut(*mut u8, usize, *mut usize) -> sys::GhosttyResult,
) -> Result<Option<Vec<u8>>> {
    let mut required = 0usize;
    let result = read(std::ptr::null_mut(), 0, &mut required);
    if result == sys::GhosttyResult_GHOSTTY_SUCCESS && required == 0 {
        return Ok(None);
    }
    if result != sys::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
        check("buffer_size_query", result)?;
    }
    if required > cap {
        return Err(GhosttyError::LimitExceeded {
            resource,
            limit: cap,
        });
    }
    let mut output = vec![0u8; required];
    let result = read(output.as_mut_ptr(), output.len(), &mut required);
    check("buffer_copy", result)?;
    if required > output.len() {
        return Err(GhosttyError::AbiMismatch(format!(
            "{resource} reported {required} bytes after receiving a {}-byte buffer",
            output.len()
        )));
    }
    output.truncate(required);
    Ok(Some(output))
}

pub(crate) fn cell_grapheme(
    cells: sys::GhosttyRenderStateRowCells,
    len: u32,
) -> Result<(char, Option<Box<[char]>>)> {
    let len = usize::try_from(len)
        .map_err(|_| GhosttyError::AbiMismatch("grapheme length overflow".into()))?;
    if len > MAX_GRAPHEME_CODEPOINTS {
        return Err(GhosttyError::LimitExceeded {
            resource: "cell grapheme",
            limit: MAX_GRAPHEME_CODEPOINTS,
        });
    }
    if len == 0 {
        return Ok((' ', None));
    }
    let mut inline = [0u32; INLINE_GRAPHEME_CODEPOINTS];
    let mut heap = Vec::new();
    let codepoints = if len <= inline.len() {
        &mut inline[..len]
    } else {
        heap.resize(len, 0);
        heap.as_mut_slice()
    };
    let result = unsafe {
        sys::ghostty_render_state_row_cells_get(
            cells,
            sys::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF,
            codepoints.as_mut_ptr().cast(),
        )
    };
    check("render_state_row_cells_get_graphemes", result)?;
    let mut characters = codepoints
        .iter()
        .copied()
        .map(|value| char::from_u32(value).unwrap_or(char::REPLACEMENT_CHARACTER));
    let character = characters.next().unwrap_or(' ');
    let zerowidth = (len > 1).then(|| characters.collect::<Vec<_>>().into_boxed_slice());
    Ok((character, zerowidth))
}

pub(crate) fn terminal_get<F: TerminalField>(terminal: sys::GhosttyTerminal) -> Result<F::Value> {
    let mut value = F::Value::default();
    let result = unsafe {
        sys::ghostty_terminal_get(terminal, F::DATA, (&mut value as *mut F::Value).cast())
    };
    check("terminal_get", result)?;
    Ok(value)
}

pub(crate) fn terminal_scrollbar(
    terminal: sys::GhosttyTerminal,
) -> Result<sys::GhosttyTerminalScrollbar> {
    let mut value: sys::GhosttyTerminalScrollbar = unsafe { std::mem::zeroed() };
    let result = unsafe {
        sys::ghostty_terminal_get(
            terminal,
            sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_SCROLLBAR,
            (&mut value as *mut sys::GhosttyTerminalScrollbar).cast(),
        )
    };
    check("terminal_get_scrollbar", result)?;
    Ok(value)
}

pub(crate) fn terminal_selection_rectangle(terminal: sys::GhosttyTerminal) -> Result<Option<bool>> {
    let mut selection: sys::GhosttySelection = unsafe { std::mem::zeroed() };
    selection.size = std::mem::size_of::<sys::GhosttySelection>();
    let result = unsafe {
        sys::ghostty_terminal_get(
            terminal,
            sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_SELECTION,
            (&mut selection as *mut sys::GhosttySelection).cast(),
        )
    };
    if result == sys::GhosttyResult_GHOSTTY_NO_VALUE {
        Ok(None)
    } else {
        check("terminal_get_selection", result)?;
        Ok(Some(selection.rectangle))
    }
}

pub(crate) fn render_get<F: RenderField>(state: sys::GhosttyRenderState) -> Result<F::Value> {
    let mut value = F::Value::default();
    let result = unsafe {
        sys::ghostty_render_state_get(state, F::DATA, (&mut value as *mut F::Value).cast())
    };
    check("render_state_get", result)?;
    Ok(value)
}

pub(crate) fn render_row_iterator(
    state: sys::GhosttyRenderState,
    mut iterator: sys::GhosttyRenderStateRowIterator,
) -> Result<sys::GhosttyRenderStateRowIterator> {
    let result = unsafe {
        sys::ghostty_render_state_get(
            state,
            sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
            (&mut iterator as *mut sys::GhosttyRenderStateRowIterator).cast(),
        )
    };
    check("render_state_get_row_iterator", result)?;
    Ok(iterator)
}

pub(crate) fn cell_grapheme_len(cells: sys::GhosttyRenderStateRowCells) -> Result<u32> {
    let mut len = 0u32;
    let result = unsafe {
        sys::ghostty_render_state_row_cells_get(
            cells,
            sys::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN,
            (&mut len as *mut u32).cast(),
        )
    };
    check("render_state_row_cells_get_grapheme_len", result)?;
    Ok(len)
}

pub(crate) fn render_row_data(
    iterator: sys::GhosttyRenderStateRowIterator,
    cells: sys::GhosttyRenderStateRowCells,
) -> Result<RenderRowData> {
    let mut dirty = false;
    let result = unsafe {
        sys::ghostty_render_state_row_get(
            iterator,
            sys::GhosttyRenderStateRowData_GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY,
            (&mut dirty as *mut bool).cast(),
        )
    };
    check("render_state_row_get_dirty", result)?;

    let mut cells = cells;
    let result = unsafe {
        sys::ghostty_render_state_row_get(
            iterator,
            sys::GhosttyRenderStateRowData_GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
            (&mut cells as *mut sys::GhosttyRenderStateRowCells).cast(),
        )
    };
    check("render_state_row_get_cells", result)?;

    let mut selection: sys::GhosttyRenderStateRowSelection = unsafe { std::mem::zeroed() };
    selection.size = std::mem::size_of::<sys::GhosttyRenderStateRowSelection>();
    let result = unsafe {
        sys::ghostty_render_state_row_get(
            iterator,
            sys::GhosttyRenderStateRowData_GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION,
            (&mut selection as *mut sys::GhosttyRenderStateRowSelection).cast(),
        )
    };
    let selection = if result == sys::GhosttyResult_GHOSTTY_NO_VALUE {
        None
    } else {
        check("render_state_row_get_selection", result)?;
        Some(selection)
    };
    Ok(RenderRowData {
        dirty,
        cells,
        selection,
    })
}

pub(crate) fn render_cell_data(cells: sys::GhosttyRenderStateRowCells) -> Result<RenderCellData> {
    let mut style: sys::GhosttyStyle = unsafe { std::mem::zeroed() };
    style.size = std::mem::size_of::<sys::GhosttyStyle>();
    let mut raw = 0u64;
    let result = unsafe {
        sys::ghostty_render_state_row_cells_get(
            cells,
            sys::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW,
            (&mut raw as *mut sys::GhosttyCell).cast(),
        )
    };
    check("render_state_row_cells_get_raw", result)?;
    let result = unsafe {
        sys::ghostty_render_state_row_cells_get(
            cells,
            sys::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
            (&mut style as *mut sys::GhosttyStyle).cast(),
        )
    };
    check("render_state_row_cells_get_style", result)?;
    Ok(RenderCellData { raw, style })
}

pub(crate) fn raw_cell_data(cell: sys::GhosttyCell) -> Result<RawCellData> {
    let mut codepoint = 0u32;
    let result = unsafe {
        sys::ghostty_cell_get(
            cell,
            sys::GhosttyCellData_GHOSTTY_CELL_DATA_CODEPOINT,
            (&mut codepoint as *mut u32).cast(),
        )
    };
    check("cell_get_codepoint", result)?;

    let wide = raw_cell_wide(cell)?;
    let mut has_hyperlink = false;
    let result = unsafe {
        sys::ghostty_cell_get(
            cell,
            sys::GhosttyCellData_GHOSTTY_CELL_DATA_HAS_HYPERLINK,
            (&mut has_hyperlink as *mut bool).cast(),
        )
    };
    check("cell_get_has_hyperlink", result)?;

    let mut content_tag = sys::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_CODEPOINT;
    let result = unsafe {
        sys::ghostty_cell_get(
            cell,
            sys::GhosttyCellData_GHOSTTY_CELL_DATA_CONTENT_TAG,
            (&mut content_tag as *mut sys::GhosttyCellContentTag).cast(),
        )
    };
    check("cell_get_content_tag", result)?;
    Ok(RawCellData {
        codepoint,
        wide,
        has_hyperlink,
        content_tag,
    })
}

pub(crate) fn raw_cell_wide(cell: sys::GhosttyCell) -> Result<sys::GhosttyCellWide> {
    let mut value = sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_NARROW;
    let result = unsafe {
        sys::ghostty_cell_get(
            cell,
            sys::GhosttyCellData_GHOSTTY_CELL_DATA_WIDE,
            (&mut value as *mut sys::GhosttyCellWide).cast(),
        )
    };
    check("cell_get_wide", result)?;
    Ok(value)
}

pub(crate) fn raw_cell_palette(cell: sys::GhosttyCell) -> Result<u8> {
    let mut value = 0u8;
    let result = unsafe {
        sys::ghostty_cell_get(
            cell,
            sys::GhosttyCellData_GHOSTTY_CELL_DATA_COLOR_PALETTE,
            (&mut value as *mut u8).cast(),
        )
    };
    check("cell_get_palette", result)?;
    Ok(value)
}

pub(crate) fn raw_cell_rgb(cell: sys::GhosttyCell) -> Result<sys::GhosttyColorRgb> {
    let mut value = sys::GhosttyColorRgb { r: 0, g: 0, b: 0 };
    let result = unsafe {
        sys::ghostty_cell_get(
            cell,
            sys::GhosttyCellData_GHOSTTY_CELL_DATA_COLOR_RGB,
            (&mut value as *mut sys::GhosttyColorRgb).cast(),
        )
    };
    check("cell_get", result)?;
    Ok(value)
}

impl From<sys::GhosttyColorRgb> for Rgb {
    fn from(value: sys::GhosttyColorRgb) -> Self {
        // Read the channels back through libghostty's accessor so a future
        // layout change stays its problem rather than a silent field swap.
        let (mut r, mut g, mut b) = (0u8, 0u8, 0u8);
        // SAFETY: `value` is a live struct and the three out-parameters are
        // valid writable storage.
        unsafe { sys::ghostty_color_rgb_get(&raw const value, &mut r, &mut g, &mut b) };
        Self { r, g, b }
    }
}

pub(crate) fn wide_cell(value: sys::GhosttyCellWide) -> Result<WideCell> {
    match value {
        sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_NARROW => Ok(WideCell::Narrow),
        sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_WIDE => Ok(WideCell::Wide),
        sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_SPACER_TAIL => Ok(WideCell::SpacerTail),
        sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_SPACER_HEAD => Ok(WideCell::SpacerHead),
        _ => Err(GhosttyError::AbiMismatch(format!(
            "unknown Ghostty cell width discriminant {value}"
        ))),
    }
}

pub(crate) fn underline(value: i32) -> Result<UnderlineStyle> {
    match value {
        0 => Ok(UnderlineStyle::None),
        1 => Ok(UnderlineStyle::Single),
        2 => Ok(UnderlineStyle::Double),
        3 => Ok(UnderlineStyle::Curly),
        4 => Ok(UnderlineStyle::Dotted),
        5 => Ok(UnderlineStyle::Dashed),
        _ => Err(GhosttyError::AbiMismatch(format!(
            "unknown Ghostty underline discriminant {value}"
        ))),
    }
}

pub(crate) fn cursor_shape(
    value: sys::GhosttyRenderStateCursorVisualStyle,
) -> Result<crate::CursorShape> {
    match value {
        sys::GhosttyRenderStateCursorVisualStyle_GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BAR => {
            Ok(crate::CursorShape::Bar)
        }
        sys::GhosttyRenderStateCursorVisualStyle_GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK => {
            Ok(crate::CursorShape::Block)
        }
        sys::GhosttyRenderStateCursorVisualStyle_GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE => {
            Ok(crate::CursorShape::Underline)
        }
        sys::GhosttyRenderStateCursorVisualStyle_GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK_HOLLOW => {
            Ok(crate::CursorShape::HollowBlock)
        }
        _ => Err(GhosttyError::AbiMismatch(format!(
            "unknown Ghostty cursor shape discriminant {value}"
        ))),
    }
}

#[cfg(test)]
mod discriminant_tests {
    use super::*;

    #[test]
    fn unknown_native_discriminants_are_rejected() {
        assert!(wide_cell(i32::MAX).is_err());
        assert!(underline(i32::MAX).is_err());
        assert!(cursor_shape(i32::MAX).is_err());
    }
}
