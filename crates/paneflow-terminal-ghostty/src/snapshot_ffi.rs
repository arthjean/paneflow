use paneflow_libghostty_sys as sys;

use crate::handles::check;
use crate::{GhosttyError, Result, Rgb, UnderlineStyle, WideCell};

const MAX_GRAPHEME_CODEPOINTS: usize = 1024;
const INLINE_GRAPHEME_CODEPOINTS: usize = 16;

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
    // SAFETY: `cells` is the live row-cells iterator handle passed through
    // `copy_cell`; GRAPHEMES_BUF writes `len` u32 values, and `codepoints` has
    // exactly that many initialized, writable elements.
    unsafe {
        cell_get(
            cells,
            sys::GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF,
            &mut codepoints[0],
        )?;
    }
    let mut characters = codepoints
        .iter()
        .copied()
        .map(|value| char::from_u32(value).unwrap_or(char::REPLACEMENT_CHARACTER));
    let character = characters.next().unwrap_or(' ');
    let zerowidth = (len > 1).then(|| characters.collect::<Vec<_>>().into_boxed_slice());
    Ok((character, zerowidth))
}

/// Read a field from a libghostty render-state handle.
///
/// # Safety
///
/// `state` must be a live `GhosttyRenderState`. `data` must select a field
/// whose ABI output type is exactly `T`, including size, alignment, and valid
/// Rust bit patterns. The selected operation must be allowed to initialize
/// `out` for the duration of this call.
pub(crate) unsafe fn render_get<T>(
    state: sys::GhosttyRenderState,
    data: sys::GhosttyRenderStateData,
    out: &mut T,
) -> Result<()> {
    let result = unsafe { sys::ghostty_render_state_get(state, data, (out as *mut T).cast()) };
    check("render_state_get", result)
}

/// Read a field from a live libghostty row-cells iterator.
///
/// # Safety
///
/// `cells` must be a live `GhosttyRenderStateRowCells`. `data` must select a
/// field whose ABI output type is exactly `T`, including size, alignment, and
/// valid Rust bit patterns. For buffer selectors, `out` must point to the first
/// element of a writable allocation large enough for the complete FFI write.
pub(crate) unsafe fn cell_get<T>(
    cells: sys::GhosttyRenderStateRowCells,
    data: sys::GhosttyRenderStateRowCellsData,
    out: &mut T,
) -> Result<()> {
    let result =
        unsafe { sys::ghostty_render_state_row_cells_get(cells, data, (out as *mut T).cast()) };
    check("render_state_row_cells_get", result)
}

/// Read multiple fields from a live libghostty row-cells iterator.
///
/// # Safety
///
/// `cells` must be a live `GhosttyRenderStateRowCells`. Each `values[i]` must
/// be non-null, aligned, writable, and point to the exact ABI output type for
/// `keys[i]`; all output regions must remain valid and non-overlapping for the
/// call, and every written value must have a valid Rust representation.
pub(crate) unsafe fn cell_get_multi<const N: usize>(
    cells: sys::GhosttyRenderStateRowCells,
    keys: [sys::GhosttyRenderStateRowCellsData; N],
    mut values: [*mut std::ffi::c_void; N],
) -> Result<()> {
    let mut written = 0usize;
    let result = unsafe {
        sys::ghostty_render_state_row_cells_get_multi(
            cells,
            N,
            keys.as_ptr(),
            values.as_mut_ptr(),
            &mut written,
        )
    };
    check("render_state_row_cells_get_multi", result)?;
    if written != N {
        return Err(GhosttyError::AbiMismatch(format!(
            "render cell multi-get wrote {written} fields, expected {N}"
        )));
    }
    Ok(())
}

/// Read a field from a libghostty cell value.
///
/// # Safety
///
/// `cell` must be a valid `GhosttyCell` produced by libghostty. `data` must
/// select a field whose ABI output type is exactly `T`, including size,
/// alignment, and valid Rust bit patterns.
pub(crate) unsafe fn raw_cell_get<T>(
    cell: sys::GhosttyCell,
    data: sys::GhosttyCellData,
    out: &mut T,
) -> Result<()> {
    let result = unsafe { sys::ghostty_cell_get(cell, data, (out as *mut T).cast()) };
    check("cell_get", result)
}

/// Read multiple fields from a libghostty cell value.
///
/// # Safety
///
/// `cell` must be a valid `GhosttyCell` produced by libghostty. Each
/// `values[i]` must be non-null, aligned, writable, and point to the exact ABI
/// output type for `keys[i]`; all output regions must remain valid and
/// non-overlapping for the call, and every written value must have a valid
/// Rust representation.
pub(crate) unsafe fn raw_cell_get_multi<const N: usize>(
    cell: sys::GhosttyCell,
    keys: [sys::GhosttyCellData; N],
    mut values: [*mut std::ffi::c_void; N],
) -> Result<()> {
    let mut written = 0usize;
    let result = unsafe {
        sys::ghostty_cell_get_multi(cell, N, keys.as_ptr(), values.as_mut_ptr(), &mut written)
    };
    check("cell_get_multi", result)?;
    if written != N {
        return Err(GhosttyError::AbiMismatch(format!(
            "raw cell multi-get wrote {written} fields, expected {N}"
        )));
    }
    Ok(())
}

impl From<sys::GhosttyColorRgb> for Rgb {
    fn from(value: sys::GhosttyColorRgb) -> Self {
        Self {
            r: value.r,
            g: value.g,
            b: value.b,
        }
    }
}

pub(crate) fn wide_cell(value: sys::GhosttyCellWide) -> WideCell {
    match value {
        sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_WIDE => WideCell::Wide,
        sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_SPACER_TAIL => WideCell::SpacerTail,
        sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_SPACER_HEAD => WideCell::SpacerHead,
        _ => WideCell::Narrow,
    }
}

pub(crate) fn underline(value: i32) -> UnderlineStyle {
    match value {
        1 => UnderlineStyle::Single,
        2 => UnderlineStyle::Double,
        3 => UnderlineStyle::Curly,
        4 => UnderlineStyle::Dotted,
        5 => UnderlineStyle::Dashed,
        _ => UnderlineStyle::None,
    }
}

pub(crate) fn cursor_shape(value: sys::GhosttyRenderStateCursorVisualStyle) -> crate::CursorShape {
    match value {
        sys::GhosttyRenderStateCursorVisualStyle_GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BAR => {
            crate::CursorShape::Bar
        }
        sys::GhosttyRenderStateCursorVisualStyle_GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE => {
            crate::CursorShape::Underline
        }
        sys::GhosttyRenderStateCursorVisualStyle_GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK_HOLLOW => {
            crate::CursorShape::HollowBlock
        }
        _ => crate::CursorShape::Block,
    }
}
