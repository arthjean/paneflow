use paneflow_libghostty_sys as sys;

use crate::{GhosttyError, Result};

pub(crate) struct KeyContract {
    pub(crate) name: &'static str,
    pub(crate) value: i64,
    pub(crate) json_type: &'static str,
    pub(crate) rust_size: usize,
}

macro_rules! keys {
    ($($name:ident => $json:literal: $rust:ty),+ $(,)?) => {
        &[$(KeyContract {
            name: stringify!($name),
            value: sys::$name as i64,
            json_type: $json,
            rust_size: std::mem::size_of::<$rust>(),
        }),+]
    };
}

type Callback = Option<unsafe extern "C" fn()>;

pub(crate) const KEY_CONTRACTS: &[KeyContract] = keys![
    GhosttyBuildInfo_GHOSTTY_BUILD_INFO_FEATURE_COLOR => "bool": bool,
    GhosttyBuildInfo_GHOSTTY_BUILD_INFO_FEATURE_FORMATTER => "bool": bool,
    GhosttyBuildInfo_GHOSTTY_BUILD_INFO_FEATURE_GRID_INTROSPECTION => "bool": bool,
    GhosttyBuildInfo_GHOSTTY_BUILD_INFO_FEATURE_INPUT_ENCODE => "bool": bool,
    GhosttyBuildInfo_GHOSTTY_BUILD_INFO_FEATURE_RENDER_STATE => "bool": bool,
    GhosttyBuildInfo_GHOSTTY_BUILD_INFO_FEATURE_SEARCH => "bool": bool,
    GhosttyBuildInfo_GHOSTTY_BUILD_INFO_FEATURE_SELECTION => "bool": bool,
    GhosttyBuildInfo_GHOSTTY_BUILD_INFO_FEATURE_SNAPSHOT => "bool": bool,
    GhosttyBuildInfo_GHOSTTY_BUILD_INFO_KITTY_GRAPHICS => "bool": bool,
    GhosttyBuildInfo_GHOSTTY_BUILD_INFO_OPTIMIZE => "GhosttyOptimizeMode": sys::GhosttyOptimizeMode,
    GhosttyBuildInfo_GHOSTTY_BUILD_INFO_SIMD => "bool": bool,
    GhosttyBuildInfo_GHOSTTY_BUILD_INFO_VERSION_MAJOR => "usize": usize,
    GhosttyBuildInfo_GHOSTTY_BUILD_INFO_VERSION_MINOR => "usize": usize,
    GhosttyBuildInfo_GHOSTTY_BUILD_INFO_VERSION_PATCH => "usize": usize,
    GhosttyBuildInfo_GHOSTTY_BUILD_INFO_VERSION_STRING => "GhosttyString": sys::GhosttyString,
    GhosttyCellData_GHOSTTY_CELL_DATA_CODEPOINT => "u32": u32,
    GhosttyCellData_GHOSTTY_CELL_DATA_COLOR_PALETTE => "GhosttyColorPaletteIndex": u8,
    GhosttyCellData_GHOSTTY_CELL_DATA_COLOR_RGB => "GhosttyColorRgb": sys::GhosttyColorRgb,
    GhosttyCellData_GHOSTTY_CELL_DATA_CONTENT_TAG => "GhosttyCellContentTag": sys::GhosttyCellContentTag,
    GhosttyCellData_GHOSTTY_CELL_DATA_HAS_HYPERLINK => "bool": bool,
    GhosttyCellData_GHOSTTY_CELL_DATA_HAS_STYLING => "bool": bool,
    GhosttyCellData_GHOSTTY_CELL_DATA_WIDE => "GhosttyCellWide": sys::GhosttyCellWide,
    GhosttyFormatterOption_GHOSTTY_FORMATTER_OPT_EMIT => "GhosttyFormatterFormat": sys::GhosttyFormatterFormat,
    GhosttyFormatterOption_GHOSTTY_FORMATTER_OPT_EXTRAS => "GhosttyFormatterExtras": sys::GhosttyFormatterExtras,
    GhosttyFormatterOption_GHOSTTY_FORMATTER_OPT_SELECTION => "GhosttySelection": sys::GhosttySelection,
    GhosttyFormatterOption_GHOSTTY_FORMATTER_OPT_TRIM => "bool": bool,
    GhosttyFormatterOption_GHOSTTY_FORMATTER_OPT_UNWRAP => "bool": bool,
    GhosttyKeyEncoderOption_GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT => "GhosttyOptionAsAlt": sys::GhosttyOptionAsAlt,
    GhosttyKittyGraphicsData_GHOSTTY_KITTY_GRAPHICS_DATA_GENERATION => "u64": u64,
    GhosttyKittyGraphicsData_GHOSTTY_KITTY_GRAPHICS_DATA_PLACEMENT_ITERATOR => "GhosttyKittyGraphicsPlacementIterator": sys::GhosttyKittyGraphicsPlacementIterator,
    GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_COMPRESSION => "GhosttyKittyImageCompression": sys::GhosttyKittyImageCompression,
    GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_EXPECTED_LEN => "usize": usize,
    GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_FORMAT => "GhosttyKittyImageFormat": sys::GhosttyKittyImageFormat,
    GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_GENERATION => "u64": u64,
    GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_HEIGHT => "u32": u32,
    GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_ID => "u32": u32,
    GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_NUMBER => "u32": u32,
    GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_PIXELS => "GhosttyString": sys::GhosttyString,
    GhosttyKittyGraphicsImageData_GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_WIDTH => "u32": u32,
    GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_COLUMNS => "u32": u32,
    GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_IMAGE_ID => "u32": u32,
    GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_IS_VIRTUAL => "bool": bool,
    GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_PLACEMENT_ID => "u32": u32,
    GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_ROWS => "u32": u32,
    GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_SOURCE_HEIGHT => "u32": u32,
    GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_SOURCE_WIDTH => "u32": u32,
    GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_SOURCE_X => "u32": u32,
    GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_SOURCE_Y => "u32": u32,
    GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_X_OFFSET => "u32": u32,
    GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_Y_OFFSET => "u32": u32,
    GhosttyKittyGraphicsPlacementData_GHOSTTY_KITTY_GRAPHICS_PLACEMENT_DATA_Z => "i32": i32,
    GhosttyMouseEncoderOption_GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED => "bool": bool,
    GhosttyMouseEncoderOption_GHOSTTY_MOUSE_ENCODER_OPT_SIZE => "GhosttyMouseEncoderSize": sys::GhosttyMouseEncoderSize,
    GhosttyMouseEncoderOption_GHOSTTY_MOUSE_ENCODER_OPT_TRACK_LAST_CELL => "bool": bool,
    GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_COLS => "u16": u16,
    GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_BLINKING => "bool": bool,
    GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_HAS_VALUE => "bool": bool,
    GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_WIDE_TAIL => "bool": bool,
    GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_X => "u16": u16,
    GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_Y => "u16": u16,
    GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VISIBLE => "bool": bool,
    GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VISUAL_STYLE => "GhosttyCursorStyle": sys::GhosttyCursorStyle,
    GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_DIRTY => "GhosttyRenderStateDirty": sys::GhosttyRenderStateDirty,
    GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_OVERSCAN => "GhosttyRenderStateOverscan": sys::GhosttyRenderStateOverscan,
    GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_OVERSCAN_REQUEST => "GhosttyRenderStateOverscan": sys::GhosttyRenderStateOverscan,
    GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_ROWS => "u16": u16,
    GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR => "GhosttyRenderStateRowIterator": sys::GhosttyRenderStateRowIterator,
    GhosttyRenderStateOption_GHOSTTY_RENDER_STATE_OPT_DIRTY => "GhosttyRenderStateDirty": sys::GhosttyRenderStateDirty,
    GhosttyRenderStateOption_GHOSTTY_RENDER_STATE_OPT_OVERSCAN => "GhosttyRenderStateOverscan": sys::GhosttyRenderStateOverscan,
    GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF => "GhosttyCodepointBuffer": sys::GhosttyCodepointBuffer,
    GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW => "GhosttyCell": sys::GhosttyCell,
    GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE => "GhosttyStyle": sys::GhosttyStyle,
    GhosttyRenderStateRowData_GHOSTTY_RENDER_STATE_ROW_DATA_CELLS => "GhosttyRenderStateRowCells": sys::GhosttyRenderStateRowCells,
    GhosttyRenderStateRowData_GHOSTTY_RENDER_STATE_ROW_DATA_CELLS_RAW => "GhosttyCellsView": sys::GhosttyCellsView,
    GhosttyRenderStateRowData_GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY => "bool": bool,
    GhosttyRenderStateRowData_GHOSTTY_RENDER_STATE_ROW_DATA_ID => "GhosttyRenderStateRowId": sys::GhosttyRenderStateRowId,
    GhosttyRenderStateRowData_GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION => "GhosttyRenderStateRowSelection": sys::GhosttyRenderStateRowSelection,
    GhosttyRenderStateRowData_GHOSTTY_RENDER_STATE_ROW_DATA_VIEWPORT_Y => "i32": i32,
    GhosttyRenderStateRowOption_GHOSTTY_RENDER_STATE_ROW_OPT_DIRTY => "bool": bool,
    GhosttyRowData_GHOSTTY_ROW_DATA_KITTY_VIRTUAL_PLACEHOLDER => "bool": bool,
    GhosttySearchData_GHOSTTY_SEARCH_DATA_MATCHES => "GhosttySelectionBuffer": sys::GhosttySelectionBuffer,
    GhosttySearchData_GHOSTTY_SEARCH_DATA_SELECTED_INDEX => "usize": usize,
    GhosttySearchData_GHOSTTY_SEARCH_DATA_SELECTED_MATCH => "GhosttySelection": sys::GhosttySelection,
    GhosttySearchData_GHOSTTY_SEARCH_DATA_STATUS => "GhosttySearchStatus": sys::GhosttySearchStatus,
    GhosttySearchData_GHOSTTY_SEARCH_DATA_TOTAL_MATCHES => "usize": usize,
    GhosttySearchData_GHOSTTY_SEARCH_DATA_VIEWPORT_MATCHES => "GhosttySelectionBuffer": sys::GhosttySelectionBuffer,
    GhosttySearchOption_GHOSTTY_SEARCH_OPT_NEEDLE => "GhosttyString": sys::GhosttyString,
    GhosttySelectionGestureData_GHOSTTY_SELECTION_GESTURE_DATA_ANCHOR => "GhosttyGridRef": sys::GhosttyGridRef,
    GhosttySelectionGestureData_GHOSTTY_SELECTION_GESTURE_DATA_AUTOSCROLL => "GhosttySelectionGestureAutoscroll": sys::GhosttySelectionGestureAutoscroll,
    GhosttySelectionGestureData_GHOSTTY_SELECTION_GESTURE_DATA_BEHAVIOR => "GhosttySelectionGestureBehavior": sys::GhosttySelectionGestureBehavior,
    GhosttySelectionGestureData_GHOSTTY_SELECTION_GESTURE_DATA_CLICK_COUNT => "u8": u8,
    GhosttySelectionGestureData_GHOSTTY_SELECTION_GESTURE_DATA_DRAGGED => "bool": bool,
    GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_BEHAVIORS => "GhosttySelectionGestureBehaviors": sys::GhosttySelectionGestureBehaviors,
    GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_GEOMETRY => "GhosttySelectionGestureGeometry": sys::GhosttySelectionGestureGeometry,
    GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_POSITION => "GhosttySurfacePosition": sys::GhosttySurfacePosition,
    GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_RECTANGLE => "bool": bool,
    GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF => "GhosttyGridRef": sys::GhosttyGridRef,
    GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REPEAT_DISTANCE => "f64": f64,
    GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REPEAT_INTERVAL_NS => "u64": u64,
    GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_TIME_NS => "u64": u64,
    GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_VIEWPORT => "GhosttyPointCoordinate": sys::GhosttyPointCoordinate,
    GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_WORD_BOUNDARY_CODEPOINTS => "GhosttyCodepoints": sys::GhosttyCodepoints,
    GhosttySnapshotDecoderOption_GHOSTTY_SNAPSHOT_DECODER_OPT_COMPRESS_HISTORY => "bool": bool,
    GhosttySnapshotDecoderOption_GHOSTTY_SNAPSHOT_DECODER_OPT_MAX_CONTINUATION_BYTES => "usize": usize,
    GhosttySnapshotDecoderOption_GHOSTTY_SNAPSHOT_DECODER_OPT_RETAIN_CONTINUATION => "bool": bool,
    GhosttySysOption_GHOSTTY_SYS_OPT_DECODE_PNG => "GhosttySysDecodePngFn": Callback,
    GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_COLS => "u16": u16,
    GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_CURSOR_X => "u16": u16,
    GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_CURSOR_Y => "u16": u16,
    GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS => "GhosttyKittyGraphics": sys::GhosttyKittyGraphics,
    GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS => "GhosttyKittyKeyFlags": u8,
    GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_MEMORY_USAGE => "GhosttyTerminalMemoryUsage": sys::GhosttyTerminalMemoryUsage,
    GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_MODE => "GhosttyTerminalModeConfig": sys::GhosttyTerminalModeConfig,
    GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_MOUSE_SHAPE => "GhosttyMouseShape": sys::GhosttyMouseShape,
    GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_PWD => "GhosttyString": sys::GhosttyString,
    GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_ROWS => "u16": u16,
    GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS => "usize": usize,
    GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_SCROLLBAR => "GhosttyTerminalScrollbar": sys::GhosttyTerminalScrollbar,
    GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_SELECTION => "GhosttySelection": sys::GhosttySelection,
    GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_TITLE => "GhosttyString": sys::GhosttyString,
    GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_TOTAL_ROWS => "usize": usize,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_APC_MAX_BYTES => "usize": usize,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_APC_MAX_BYTES_KITTY => "usize": usize,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_BELL => "GhosttyTerminalBellFn": Callback,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_CLIPBOARD_READ => "GhosttyTerminalClipboardReadFn": Callback,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE => "GhosttyTerminalClipboardWriteFn": Callback,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE_MAX_BYTES => "usize": usize,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND => "GhosttyColorRgb": sys::GhosttyColorRgb,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_CURSOR => "GhosttyColorRgb": sys::GhosttyColorRgb,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND => "GhosttyColorRgb": sys::GhosttyColorRgb,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_PALETTE => "array": [sys::GhosttyColorRgb; 256],
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_SCHEME => "GhosttyTerminalColorSchemeFn": Callback,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_CONTINUATION_MAX_BYTES => "usize": usize,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_BLINK => "bool": bool,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_STYLE => "GhosttyCursorStyle": sys::GhosttyCursorStyle,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_DESKTOP_NOTIFICATION => "GhosttyTerminalDesktopNotificationFn": Callback,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES => "GhosttyTerminalDeviceAttributesFn": Callback,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_ENQUIRY => "GhosttyTerminalEnquiryFn": Callback,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_GLYPH_PROTOCOL => "bool": bool,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_MEDIUM_FILE => "bool": bool,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_MEDIUM_SHARED_MEM => "bool": bool,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_MEDIUM_TEMP_FILE => "GhosttyString": sys::GhosttyString,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT => "u64": u64,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_MODE => "GhosttyTerminalModeConfig": sys::GhosttyTerminalModeConfig,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_PROGRAM_STATUS => "GhosttyTerminalProgramStatusFn": Callback,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_PROGRESS_REPORT => "GhosttyTerminalProgressReportFn": Callback,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_PWD_CHANGED => "GhosttyTerminalPwdChangedFn": Callback,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_RENDER_HOLD => "GhosttyTerminalRenderHoldFn": Callback,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_RESET => "GhosttyTerminalResetFn": Callback,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_RESIZE_PULL_SCROLLBACK => "bool": bool,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES => "usize": usize,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES => "usize": usize,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_SELECTION => "GhosttySelection": sys::GhosttySelection,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_SEMANTIC_PROMPT => "GhosttyTerminalSemanticPromptFn": Callback,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_SIZE => "GhosttyTerminalSizeFn": Callback,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_TERMINFO_NAME => "GhosttyString": sys::GhosttyString,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_TITLE_CHANGED => "GhosttyTerminalTitleChangedFn": Callback,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_UNKNOWN_MAX_BYTES => "usize": usize,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_UNKNOWN_SEQUENCE => "GhosttyTerminalUnknownSequenceFn": Callback,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_USERDATA => "opaque": *mut std::ffi::c_void,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_WRITE_PTY => "GhosttyTerminalWritePtyFn": Callback,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_XTVERSION => "GhosttyTerminalXtversionFn": Callback,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_XT_CHECKSUM_EXTENSION => "u8": u8,
    GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_XT_CHECKSUM_REPORT => "bool": bool,
];

fn usize_name() -> String {
    format!("u{}", usize::BITS)
}

fn primitive_size(name: &str) -> Option<u64> {
    Some(match name {
        "u8" | "i8" | "bool" => 1,
        "u16" | "i16" => 2,
        "u32" | "i32" | "f32" => 4,
        "u64" | "i64" | "f64" => 8,
        "opaque" | "pointer" => std::mem::size_of::<usize>() as u64,
        _ => return None,
    })
}

fn type_size(types: &serde_json::Value, entry: &serde_json::Value) -> Option<u64> {
    let name = entry.get("type")?.as_str()?;
    if name == "array" {
        let elem = entry.get("elem")?.as_str()?;
        let count = entry.get("count")?.as_u64()?;
        let elem_size = primitive_size(elem)
            .or_else(|| types.get(elem)?.get("size")?.as_u64())?;
        return elem_size.checked_mul(count);
    }
    primitive_size(name).or_else(|| types.get(name)?.get("size")?.as_u64())
}

fn split_key(types: &serde_json::Value, constant: &str) -> Option<(String, String)> {
    let (enum_name, rest) = constant.split_once('_')?;
    let prefix = types.get(enum_name)?.get("prefix")?.as_str()?;
    let key = rest.strip_prefix(prefix)?;
    Some((enum_name.to_owned(), key.to_owned()))
}

pub(crate) fn validate(types: &serde_json::Value) -> Result<()> {
    for contract in KEY_CONTRACTS {
        validate_contract(types, contract)?;
    }
    Ok(())
}

fn validate_contract(types: &serde_json::Value, contract: &KeyContract) -> Result<()> {
    let mismatch = |detail: String| {
        Err(GhosttyError::AbiMismatch(format!(
            "{} value type {detail}",
            contract.name
        )))
    };
    let Some((enum_name, key)) = split_key(types, contract.name) else {
        return mismatch("has no key prefix in the type manifest".into());
    };
    let published_value = types
        .get(&enum_name)
        .and_then(|value| value.get("values"))
        .and_then(|values| values.get(&key))
        .and_then(serde_json::Value::as_i64);
    if published_value != Some(contract.value) {
        return Err(GhosttyError::AbiMismatch(format!(
            "{} discriminant expected {}, published {published_value:?}",
            contract.name, contract.value
        )));
    }
    let Some(entry) = types
        .get(&enum_name)
        .and_then(|value| value.get("keys"))
        .and_then(|keys| keys.get(&key))
    else {
        return mismatch(format!("is missing from {enum_name}.keys"));
    };
    let published = entry.get("type").and_then(serde_json::Value::as_str);
    let expected = if contract.json_type == "usize" {
        usize_name()
    } else {
        contract.json_type.to_owned()
    };
    if published != Some(expected.as_str()) {
        return mismatch(format!("expected {expected}, published {published:?}"));
    }
    let size = type_size(types, entry);
    if size != Some(contract.rust_size as u64) {
        return mismatch(format!(
            "size expected {}, published {size:?}",
            contract.rust_size
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::ffi::CStr;

    use super::*;

    fn pinned_types() -> serde_json::Value {
        let json = unsafe { CStr::from_ptr(sys::ghostty_type_json()) }
            .to_str()
            .expect("type manifest is UTF-8");
        serde_json::from_str::<serde_json::Value>(json).expect("type manifest must parse")["types"]
            .clone()
    }

    #[test]
    fn every_key_the_crate_passes_has_the_published_value_type() {
        validate(&pinned_types()).expect("every key contract must match the pinned manifest");
    }

    #[test]
    fn a_retyped_key_is_named_in_the_mismatch() {
        let mut types = pinned_types();
        types["GhosttyRenderStateRowCellsData"]["keys"]["GRAPHEMES_BUF"]["type"] =
            serde_json::Value::from("u32");
        let error = validate(&types).expect_err("a retyped key must not validate");
        assert!(
            matches!(&error, GhosttyError::AbiMismatch(message)
                if message.starts_with("GhosttyRenderStateRowCellsData_GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF value type expected GhosttyCodepointBuffer")),
            "{error:?}"
        );
    }

    #[test]
    fn a_resized_value_type_is_named_in_the_mismatch() {
        let mut types = pinned_types();
        types["GhosttySelection"]["size"] = serde_json::Value::from(48);
        let error = validate(&types).expect_err("a resized value type must not validate");
        assert!(
            matches!(&error, GhosttyError::AbiMismatch(message) if message.contains("size expected 40")),
            "{error:?}"
        );
    }

    #[test]
    fn every_key_the_crate_reads_or_writes_has_a_contract() {
        let types = pinned_types();
        let contracts: BTreeSet<&str> = KEY_CONTRACTS.iter().map(|contract| contract.name).collect();
        let source_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut missing = BTreeSet::new();
        for entry in std::fs::read_dir(&source_dir).expect("source directory") {
            let path = entry.expect("source entry").path();
            let Some(file) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !file.ends_with(".rs") || file == "abi_keys.rs" || file == "abi_discriminants.rs" {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("source file");
            for token in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
                if !token.starts_with("Ghostty") || !token.contains("_GHOSTTY_") {
                    continue;
                }
                let Some((enum_name, _)) = split_key(&types, token) else {
                    continue;
                };
                if types[&enum_name].get("keys").is_some() && !contracts.contains(token) {
                    missing.insert(format!("{file}: {token}"));
                }
            }
        }
        assert!(missing.is_empty(), "keys without a value contract: {missing:#?}");
    }
}
