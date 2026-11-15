use paneflow_libghostty_sys as sys;

use crate::{GhosttyError, Result};

type Field<'a> = (&'a str, usize, usize);

macro_rules! check {
    ($layouts:expr, $ty:ty, $name:literal, { $($field:ident: $field_ty:ty),+ $(,)? }) => {
        check_layout::<$ty>(
            $layouts,
            $name,
            &[$((
                stringify!($field),
                std::mem::offset_of!($ty, $field),
                std::mem::size_of::<$field_ty>(),
            )),+],
        )?;
    };
}

pub(crate) fn validate(layouts: &serde_json::Value) -> Result<()> {
    check!(layouts, sys::GhosttyColorRgb, "GhosttyColorRgb", {
        r: u8, g: u8, b: u8
    });
    check!(layouts, sys::GhosttyString, "GhosttyString", {
        ptr: *const u8, len: usize
    });
    check!(layouts, sys::GhosttyGridRef, "GhosttyGridRef", {
        node: *mut std::ffi::c_void, x: u16, y: u16
    });
    check!(layouts, sys::GhosttySelection, "GhosttySelection", {
        start: sys::GhosttyGridRef, end: sys::GhosttyGridRef, rectangle: bool
    });
    check!(layouts, sys::GhosttySelectionBuffer, "GhosttySelectionBuffer", {
        ptr: *mut sys::GhosttySelection, cap: usize, len: usize
    });
    check!(layouts, sys::GhosttyCodepointBuffer, "GhosttyCodepointBuffer", {
        ptr: *mut u32, cap: usize, len: usize
    });
    check!(layouts, sys::GhosttyPointCoordinate, "GhosttyPointCoordinate", {
        x: u16, y: u32
    });
    check!(layouts, sys::GhosttyPoint, "GhosttyPoint", {
        tag: sys::GhosttyPointTag, value: sys::GhosttyPointValue
    });
    check!(layouts, sys::GhosttyStyleColor, "GhosttyStyleColor", {
        tag: u8, value: sys::GhosttyStyleColorValue
    });
    check!(layouts, sys::GhosttyStyle, "GhosttyStyle", {
        fg_color: sys::GhosttyStyleColor,
        bg_color: sys::GhosttyStyleColor,
        underline_color: sys::GhosttyStyleColor,
        flags: sys::GhosttyStyleFlags,
        reserved: u16
    });
    crate::style::validate_flag_bits(layouts)?;
    check!(layouts, sys::GhosttyRenderStateColors, "GhosttyRenderStateColors", {
        size: usize,
        background: sys::GhosttyColorRgb,
        foreground: sys::GhosttyColorRgb,
        cursor: sys::GhosttyColorRgb,
        cursor_has_value: bool,
        palette: [sys::GhosttyColorRgb; 256]
    });
    check!(layouts, sys::GhosttyMousePosition, "GhosttyMousePosition", {
        x: f32, y: f32
    });
    check!(layouts, sys::GhosttyMouseEncoderSize, "GhosttyMouseEncoderSize", {
        size: usize,
        screen_width: u32,
        screen_height: u32,
        cell_width: u32,
        cell_height: u32,
        padding_top: u32,
        padding_bottom: u32,
        padding_right: u32,
        padding_left: u32
    });
    check!(layouts, sys::GhosttySizeReportSize, "GhosttySizeReportSize", {
        rows: u16, columns: u16, cell_width: u32, cell_height: u32
    });
    check!(layouts, sys::GhosttyTerminalScrollbar, "GhosttyTerminalScrollbar", {
        total: u64, offset: u64, len: u64
    });
    check!(layouts, sys::GhosttyTerminalScrollViewport, "GhosttyTerminalScrollViewport", {
        tag: sys::GhosttyTerminalScrollViewportTag,
        value: sys::GhosttyTerminalScrollViewportValue
    });
    check!(layouts, sys::GhosttyDeviceAttributesPrimary, "GhosttyDeviceAttributesPrimary", {
        conformance_level: u16, features: [u16; 64], num_features: usize
    });
    check!(layouts, sys::GhosttyDeviceAttributesSecondary, "GhosttyDeviceAttributesSecondary", {
        device_type: u16, firmware_version: u16, rom_cartridge: u16
    });
    check!(layouts, sys::GhosttyDeviceAttributesTertiary, "GhosttyDeviceAttributesTertiary", {
        unit_id: u32
    });
    check!(layouts, sys::GhosttyDeviceAttributes, "GhosttyDeviceAttributes", {
        primary: sys::GhosttyDeviceAttributesPrimary,
        secondary: sys::GhosttyDeviceAttributesSecondary,
        tertiary: sys::GhosttyDeviceAttributesTertiary
    });
    check!(layouts, sys::GhosttyTerminalProgramStatus, "GhosttyTerminalProgramStatus", {
        state: sys::GhosttyProgramStatusState,
        kind: sys::GhosttyProgramStatusKind,
        progress: i8,
        id: sys::GhosttyString,
        app: sys::GhosttyString,
        title: sys::GhosttyString,
        message: sys::GhosttyString
    });
    check!(layouts, sys::GhosttyTerminalSemanticPrompt, "GhosttyTerminalSemanticPrompt", {
        kind: sys::GhosttySemanticPromptKind,
        prompt_kind: sys::GhosttySemanticPromptPromptKind,
        has_exit_code: bool,
        exit_code: i32,
        command: sys::GhosttyString,
        error: sys::GhosttyString
    });
    check!(layouts, sys::GhosttyTerminalMemoryUsage, "GhosttyTerminalMemoryUsage", {
        size: usize,
        compression_supported: bool,
        primary_pages: u64,
        primary_virtual_bytes: u64,
        primary_resident_bytes: u64,
        primary_compressed_pages: u64,
        primary_compressed_bytes: u64,
        primary_image_bytes: u64,
        alternate_pages: u64,
        alternate_virtual_bytes: u64,
        alternate_resident_bytes: u64,
        alternate_compressed_pages: u64,
        alternate_compressed_bytes: u64,
        alternate_image_bytes: u64
    });
    check!(layouts, sys::GhosttyTerminalUnknownOscSequence, "GhosttyTerminalUnknownOscSequence", {
        truncated: bool,
        content: sys::GhosttyString,
        terminator: sys::GhosttyOscTerminator
    });
    check!(layouts, sys::GhosttyTerminalUnknownStringSequence, "GhosttyTerminalUnknownStringSequence", {
        truncated: bool,
        content: sys::GhosttyString
    });
    check!(layouts, sys::GhosttyTerminalUnknownSequenceValue, "GhosttyTerminalUnknownSequenceValue", {
        apc: sys::GhosttyTerminalUnknownStringSequence,
        osc: sys::GhosttyTerminalUnknownOscSequence
    });
    check!(layouts, sys::GhosttyTerminalUnknownSequence, "GhosttyTerminalUnknownSequence", {
        tag: sys::GhosttyTerminalUnknownSequenceTag,
        value: sys::GhosttyTerminalUnknownSequenceValue
    });
    check!(layouts, sys::GhosttyTerminalWriteStop, "GhosttyTerminalWriteStop", {
        size: usize,
        consumed: usize,
        reached: u32,
        kind: sys::GhosttyTerminalStringKind,
        introducer: sys::GhosttyTerminalStringIntroducer,
        end: sys::GhosttyTerminalStringEnd,
        byte: u8
    });
    check!(layouts, sys::GhosttyTerminalModeConfig, "GhosttyTerminalModeConfig", {
        mode: sys::GhosttyMode,
        value: bool
    });
    check!(layouts, sys::GhosttyRenderStateOverscan, "GhosttyRenderStateOverscan", {
        above: u16, below: u16
    });
    check!(layouts, sys::GhosttyRenderStateRowId, "GhosttyRenderStateRowId", {
        bits: [u64; 2]
    });
    check!(layouts, sys::GhosttyTerminalProgressReport, "GhosttyTerminalProgressReport", {
        state: sys::GhosttyTerminalProgressState, progress: i8
    });
    check!(layouts, sys::GhosttyTerminalDesktopNotification, "GhosttyTerminalDesktopNotification", {
        title: sys::GhosttyString, body: sys::GhosttyString
    });
    check!(layouts, sys::GhosttyClipboardContent, "GhosttyClipboardContent", {
        mime: sys::GhosttyString, data: sys::GhosttyString
    });
    check!(layouts, sys::GhosttyClipboardWrite, "GhosttyClipboardWrite", {
        location: sys::GhosttyClipboardLocation,
        contents: *const sys::GhosttyClipboardContent,
        contents_len: usize,
        name: sys::GhosttyString,
        granted: bool,
        can_remember: bool,
        ctx: *const std::ffi::c_void,
        reply: sys::GhosttyClipboardWriteReplyFn
    });
    check!(layouts, sys::GhosttyClipboardRead, "GhosttyClipboardRead", {
        location: sys::GhosttyClipboardLocation,
        mimes: *const sys::GhosttyString,
        mimes_len: usize,
        list: bool,
        name: sys::GhosttyString,
        granted: bool,
        can_remember: bool,
        ctx: *const std::ffi::c_void,
        reply: sys::GhosttyClipboardReadReplyFn
    });
    check!(layouts, sys::GhosttyKittyGraphicsPlacementRenderInfo, "GhosttyKittyGraphicsPlacementRenderInfo", {
        size: usize,
        pixel_width: u32,
        pixel_height: u32,
        grid_cols: u32,
        grid_rows: u32,
        viewport_col: i32,
        viewport_row: i32,
        viewport_visible: bool,
        source_x: u32,
        source_y: u32,
        source_width: u32,
        source_height: u32
    });
    Ok(())
}

fn check_layout<T>(layouts: &serde_json::Value, name: &str, fields: &[Field<'_>]) -> Result<()> {
    let layout = layouts.get(name);
    let actual_size = number(layout, "size");
    let actual_align = number(layout, "align");
    let expected_size = std::mem::size_of::<T>() as u64;
    let expected_align = std::mem::align_of::<T>() as u64;
    if (actual_size, actual_align) != (Some(expected_size), Some(expected_align)) {
        return Err(GhosttyError::AbiMismatch(format!(
            "{name} size/alignment expected {expected_size}/{expected_align}, got {actual_size:?}/{actual_align:?}"
        )));
    }
    for &(field, expected_offset, expected_field_size) in fields {
        let metadata = layout
            .and_then(|value| value.get("fields"))
            .and_then(|value| value.get(field));
        let actual_offset = number(metadata, "offset");
        let actual_field_size = number(metadata, "size");
        if (actual_offset, actual_field_size)
            != (
                Some(expected_offset as u64),
                Some(expected_field_size as u64),
            )
        {
            return Err(GhosttyError::AbiMismatch(format!(
                "{name}.{field} offset/size expected {expected_offset}/{expected_field_size}, got {actual_offset:?}/{actual_field_size:?}"
            )));
        }
    }
    Ok(())
}

fn number(value: Option<&serde_json::Value>, key: &str) -> Option<u64> {
    value
        .and_then(|value| value.get(key))
        .and_then(serde_json::Value::as_u64)
}

#[cfg(test)]
mod tests {
    use std::ffi::CStr;

    use super::*;

    const UNKNOWN_SEQUENCE_SIZE_AT_0C2A290D: usize = 136;

    fn pinned_layouts() -> serde_json::Value {
        let json = unsafe { CStr::from_ptr(sys::ghostty_type_json()) }
            .to_str()
            .expect("layout JSON is UTF-8");
        let document: serde_json::Value =
            serde_json::from_str(json).expect("layout JSON must parse");
        document["types"].clone()
    }

    #[test]
    fn the_pinned_layouts_validate() {
        validate(&pinned_layouts()).expect("pinned layouts must match the bindings");
    }

    #[test]
    fn the_unknown_sequence_keeps_its_size_across_the_re_pin() {
        assert_eq!(
            std::mem::size_of::<sys::GhosttyTerminalUnknownSequence>(),
            UNKNOWN_SEQUENCE_SIZE_AT_0C2A290D
        );
    }

    #[test]
    fn a_shifted_program_status_field_is_named_in_the_mismatch() {
        let mut layouts = pinned_layouts();
        let offset = &mut layouts["GhosttyTerminalProgramStatus"]["fields"]["state"]["offset"];
        let shifted = offset.as_u64().expect("state offset") + 4;
        *offset = serde_json::Value::from(shifted);

        let error = validate(&layouts).expect_err("a shifted field must not validate");
        assert!(
            matches!(
                &error,
                GhosttyError::AbiMismatch(message)
                    if message.starts_with("GhosttyTerminalProgramStatus.state offset/size")
            ),
            "{error:?}"
        );
    }

    #[test]
    fn a_shifted_apc_arm_mode_config_or_write_stop_field_is_named_in_the_mismatch() {
        for (layout, field) in [
            ("GhosttyTerminalUnknownStringSequence", "content"),
            ("GhosttyTerminalUnknownSequenceValue", "apc"),
            ("GhosttyTerminalModeConfig", "value"),
            ("GhosttyTerminalWriteStop", "consumed"),
        ] {
            let mut layouts = pinned_layouts();
            let offset = &mut layouts[layout]["fields"][field]["offset"];
            let shifted = offset.as_u64().expect("field offset") + 1;
            *offset = serde_json::Value::from(shifted);

            let error = validate(&layouts).expect_err("a shifted field must not validate");
            let expected = format!("{layout}.{field} offset/size");
            assert!(
                matches!(&error, GhosttyError::AbiMismatch(message) if message.starts_with(&expected)),
                "{error:?}"
            );
        }
    }
}
