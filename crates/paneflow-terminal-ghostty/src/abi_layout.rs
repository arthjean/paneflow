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
    check!(layouts, sys::GhosttyTerminalOptions, "GhosttyTerminalOptions", {
        cols: u16, rows: u16, max_scrollback: usize
    });
    check!(layouts, sys::GhosttyGridRef, "GhosttyGridRef", {
        size: usize, node: *mut std::ffi::c_void, x: u16, y: u16
    });
    check!(layouts, sys::GhosttyPointCoordinate, "GhosttyPointCoordinate", {
        x: u16, y: u32
    });
    check!(layouts, sys::GhosttyPoint, "GhosttyPoint", {
        tag: sys::GhosttyPointTag, value: sys::GhosttyPointValue
    });
    check!(layouts, sys::GhosttyStyleColor, "GhosttyStyleColor", {
        tag: sys::GhosttyStyleColorTag, value: sys::GhosttyStyleColorValue
    });
    check!(layouts, sys::GhosttyStyle, "GhosttyStyle", {
        size: usize,
        fg_color: sys::GhosttyStyleColor,
        bg_color: sys::GhosttyStyleColor,
        underline_color: sys::GhosttyStyleColor,
        bold: bool,
        italic: bool,
        faint: bool,
        blink: bool,
        inverse: bool,
        invisible: bool,
        strikethrough: bool,
        overline: bool,
        underline: std::ffi::c_int
    });
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
