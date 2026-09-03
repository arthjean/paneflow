use paneflow_libghostty_sys as sys;

use crate::engine::DisplayTerminal;
use crate::snapshot_ffi::{
    cell_grapheme, cell_grapheme_len, raw_cell_data, raw_cell_palette, raw_cell_rgb,
    render_cell_data, underline, wide_cell,
};
use crate::style::style_color;
use crate::{Cell, CellFlags, Color, GhosttyError, Point, Result};

impl DisplayTerminal {
    pub(crate) fn copy_cell(
        &self,
        row_cells: sys::GhosttyRenderStateRowCells,
        row: usize,
        column: usize,
        selected: bool,
    ) -> Result<Cell> {
        let rendered = render_cell_data(row_cells)?;
        let style = rendered.style;
        let foreground = style_color(style.fg_color)?;
        let raw = raw_cell_data(rendered.raw)?;
        let (character, zerowidth) = match raw.content_tag {
            sys::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_CODEPOINT => {
                (decode_codepoint(raw.codepoint), None)
            }
            sys::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_CODEPOINT_GRAPHEME => {
                cell_grapheme(row_cells, cell_grapheme_len(row_cells)?)?
            }
            sys::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_BG_COLOR_PALETTE
            | sys::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_BG_COLOR_RGB => (' ', None),
            value => {
                return Err(GhosttyError::AbiMismatch(format!(
                    "unknown Ghostty cell content tag {value}"
                )));
            }
        };
        let background = cell_background(rendered.raw, raw.content_tag, style.bg_color)?;
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
            wide: wide_cell(raw.wide)?,
            selected,
            hyperlink: raw.has_hyperlink,
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

fn cell_background(
    cell: sys::GhosttyCell,
    content_tag: sys::GhosttyCellContentTag,
    style: sys::GhosttyStyleColor,
) -> Result<Color> {
    match content_tag {
        sys::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_BG_COLOR_PALETTE => {
            Ok(Color::Palette(raw_cell_palette(cell)?))
        }
        sys::GhosttyCellContentTag_GHOSTTY_CELL_CONTENT_BG_COLOR_RGB => {
            Ok(Color::Rgb(raw_cell_rgb(cell)?.into()))
        }
        _ => style_color(style),
    }
}
