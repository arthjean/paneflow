use paneflow_libghostty_sys as sys;

use crate::engine::DisplayTerminal;
use crate::snapshot_ffi::{
    cell_get, cell_get_multi, cell_grapheme, raw_cell_get, raw_cell_get_multi, underline, wide_cell,
};
use crate::{Cell, CellFlags, Color, GhosttyError, Point, Result};

impl DisplayTerminal {
    pub(crate) fn copy_cell(
        &self,
        row_cells: sys::GhosttyRenderStateRowCells,
        row: usize,
        column: usize,
        selected: bool,
    ) -> Result<Cell> {
        let mut style: sys::GhosttyStyle = unsafe { std::mem::zeroed() };
        style.size = std::mem::size_of::<sys::GhosttyStyle>();
        let mut raw_cell = 0u64;
        // SAFETY: `row_cells` is the live iterator handle returned for the
        // current render row. RAW writes `GhosttyCell`, STYLE writes
        // `GhosttyStyle`, and both pointers target distinct writable locals.
        unsafe {
            cell_get_multi(
                row_cells,
                [
                    sys::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW,
                    sys::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
                ],
                [
                    (&mut raw_cell as *mut sys::GhosttyCell).cast(),
                    (&mut style as *mut sys::GhosttyStyle).cast(),
                ],
            )?;
        }
        let foreground = style_color(style.fg_color)?;
        let mut codepoint = 0u32;
        let mut wide = sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_NARROW;
        let mut has_hyperlink = false;
        let mut content_tag = sys::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_CODEPOINT;
        // SAFETY: `raw_cell` was produced by the RAW selector above. The four
        // selectors write u32, GhosttyCellWide, bool, and GhosttyCellContentTag
        // respectively into distinct, aligned, writable locals.
        unsafe {
            raw_cell_get_multi(
                raw_cell,
                [
                    sys::GhosttyCellData_GHOSTTY_CELL_DATA_CODEPOINT,
                    sys::GhosttyCellData_GHOSTTY_CELL_DATA_WIDE,
                    sys::GhosttyCellData_GHOSTTY_CELL_DATA_HAS_HYPERLINK,
                    sys::GhosttyCellData_GHOSTTY_CELL_DATA_CONTENT_TAG,
                ],
                [
                    (&mut codepoint as *mut u32).cast(),
                    (&mut wide as *mut sys::GhosttyCellWide).cast(),
                    (&mut has_hyperlink as *mut bool).cast(),
                    (&mut content_tag as *mut sys::GhosttyCellContentTag).cast(),
                ],
            )?;
        }
        let (character, zerowidth) = match content_tag {
            sys::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_CODEPOINT => {
                (decode_codepoint(codepoint), None)
            }
            sys::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_CODEPOINT_GRAPHEME => {
                let mut grapheme_len = 0u32;
                // SAFETY: `row_cells` is the current live row-cells iterator,
                // and GRAPHEMES_LEN writes exactly one u32.
                unsafe {
                    cell_get(
                        row_cells,
                        sys::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN,
                        &mut grapheme_len,
                    )?;
                }
                cell_grapheme(row_cells, grapheme_len)?
            }
            sys::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_BG_COLOR_PALETTE
            | sys::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_BG_COLOR_RGB => (' ', None),
            value => {
                return Err(GhosttyError::AbiMismatch(format!(
                    "unknown Ghostty cell content tag {value}"
                )));
            }
        };
        let background = cell_background(raw_cell, content_tag, style.bg_color)?;
        let row = i32::try_from(row)
            .map_err(|_| GhosttyError::AbiMismatch("snapshot row overflow".into()))?;
        Ok(Cell {
            point: Point::new(row, column),
            character,
            zerowidth,
            foreground,
            background,
            flags: CellFlags {
                bold: style.bold,
                dim: style.faint,
                italic: style.italic,
                inverse: style.inverse,
                invisible: style.invisible,
                strikethrough: style.strikethrough,
                overline: style.overline,
                underline: underline(style.underline)?,
            },
            wide: wide_cell(wide)?,
            selected,
            hyperlink: has_hyperlink,
        })
    }
}

fn decode_codepoint(value: u32) -> char {
    if value == 0 {
        ' '
    } else {
        char::from_u32(value).unwrap_or(char::REPLACEMENT_CHARACTER)
    }
}

/// Resolve the same background precedence as Ghostty's `Style.bg`: an erased
/// cell can carry its background in the cell content even when its style is
/// default. Keeping palette values indexed lets Paneflow apply its own theme.
fn cell_background(
    cell: sys::GhosttyCell,
    content_tag: sys::GhosttyCellContentTag,
    style: sys::GhosttyStyleColor,
) -> Result<Color> {
    match content_tag {
        sys::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_BG_COLOR_PALETTE => {
            let mut index = 0u8;
            // SAFETY: `cell` came from the current row's RAW selector, and the
            // COLOR_PALETTE selector writes exactly one u8.
            unsafe {
                raw_cell_get(
                    cell,
                    sys::GhosttyCellData_GHOSTTY_CELL_DATA_COLOR_PALETTE,
                    &mut index,
                )?;
            }
            Ok(Color::Palette(index))
        }
        sys::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_BG_COLOR_RGB => {
            let mut rgb = sys::GhosttyColorRgb { r: 0, g: 0, b: 0 };
            // SAFETY: `cell` came from the current row's RAW selector, and the
            // COLOR_RGB selector writes exactly one `GhosttyColorRgb`.
            unsafe {
                raw_cell_get(
                    cell,
                    sys::GhosttyCellData_GHOSTTY_CELL_DATA_COLOR_RGB,
                    &mut rgb,
                )?;
            }
            Ok(Color::Rgb(rgb.into()))
        }
        _ => style_color(style),
    }
}

fn style_color(color: sys::GhosttyStyleColor) -> Result<Color> {
    match color.tag {
        sys::GhosttyStyleColorTag_GHOSTTY_STYLE_COLOR_NONE => Ok(Color::Default),
        sys::GhosttyStyleColorTag_GHOSTTY_STYLE_COLOR_PALETTE => {
            Ok(Color::Palette(unsafe { color.value.palette }))
        }
        sys::GhosttyStyleColorTag_GHOSTTY_STYLE_COLOR_RGB => {
            Ok(Color::Rgb(unsafe { color.value.rgb }.into()))
        }
        _ => Err(GhosttyError::AbiMismatch(
            "unknown Ghostty style color tag".into(),
        )),
    }
}
