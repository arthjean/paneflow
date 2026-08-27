use std::ffi::CStr;

use paneflow_libghostty_sys as sys;

use crate::handles::check;
use crate::{GhosttyError, Result};

type TerminalNewFn = unsafe extern "C" fn(
    *const sys::GhosttyAllocator,
    *mut sys::GhosttyTerminal,
    u16,
    u16,
) -> sys::GhosttyResult;
type TerminalResizeFn =
    unsafe extern "C" fn(sys::GhosttyTerminal, u16, u16, u32, u32) -> sys::GhosttyResult;
type TerminalWriteFn = unsafe extern "C" fn(sys::GhosttyTerminal, *const u8, usize);
type RenderUpdateFn =
    unsafe extern "C" fn(sys::GhosttyRenderState, sys::GhosttyTerminal) -> sys::GhosttyResult;
type KeyEncodeFn = unsafe extern "C" fn(
    sys::GhosttyKeyEncoder,
    sys::GhosttyKeyEvent,
    *mut std::ffi::c_char,
    usize,
    *mut usize,
) -> sys::GhosttyResult;

const _: TerminalNewFn = sys::ghostty_terminal_new;
const _: unsafe extern "C" fn(sys::GhosttyTerminal) = sys::ghostty_terminal_free;
const _: TerminalResizeFn = sys::ghostty_terminal_resize;
const _: TerminalWriteFn = sys::ghostty_terminal_vt_write;
const _: RenderUpdateFn = sys::ghostty_render_state_update;
const _: KeyEncodeFn = sys::ghostty_key_encoder_encode;
const _: unsafe extern "C" fn(*const sys::GhosttyAllocator, *mut u8, usize) = sys::ghostty_free;

pub(crate) fn validate() -> Result<()> {
    validate_discriminants()?;
    let actual = (
        build_info_u32(sys::GhosttyBuildInfo_GHOSTTY_BUILD_INFO_VERSION_MAJOR)?,
        build_info_u32(sys::GhosttyBuildInfo_GHOSTTY_BUILD_INFO_VERSION_MINOR)?,
        build_info_u32(sys::GhosttyBuildInfo_GHOSTTY_BUILD_INFO_VERSION_PATCH)?,
    );
    let actual = format!("{}.{}.{}", actual.0, actual.1, actual.2);
    if actual != sys::EXPECTED_API_VERSION {
        return Err(GhosttyError::AbiMismatch(format!(
            "expected {}, got {actual}",
            sys::EXPECTED_API_VERSION
        )));
    }
    let json = unsafe {
        let pointer = sys::ghostty_type_json();
        if pointer.is_null() {
            return Err(GhosttyError::AbiMismatch(
                "ghostty_type_json returned null".into(),
            ));
        }
        CStr::from_ptr(pointer)
            .to_str()
            .map_err(|_| GhosttyError::AbiMismatch("layout JSON is not UTF-8".into()))?
    };
    let document: serde_json::Value = serde_json::from_str(json)
        .map_err(|error| GhosttyError::AbiMismatch(format!("invalid layout JSON: {error}")))?;
    crate::abi_layout::validate(layout_types(&document)?)
}

/// Unwrap the type map from the ABI manifest returned by `ghostty_type_json`.
///
/// The manifest wraps the per-type descriptors in an envelope whose shape is
/// pinned by its `schema` number. Refusing an unknown number keeps a future
/// reshuffle a loud startup failure instead of a silent layout check that
/// matches nothing.
fn layout_types(document: &serde_json::Value) -> Result<&serde_json::Value> {
    const EXPECTED_SCHEMA: u64 = 1;

    let schema = document.get("schema").and_then(serde_json::Value::as_u64);
    if schema != Some(EXPECTED_SCHEMA) {
        return Err(GhosttyError::AbiMismatch(format!(
            "layout JSON schema expected {EXPECTED_SCHEMA}, got {schema:?}"
        )));
    }
    document
        .get("types")
        .ok_or_else(|| GhosttyError::AbiMismatch("layout JSON has no types map".into()))
}

fn validate_discriminants() -> Result<()> {
    for (name, actual, expected) in [
        (
            "GHOSTTY_SUCCESS",
            sys::GhosttyResult_GHOSTTY_SUCCESS as i64,
            0,
        ),
        (
            "GHOSTTY_INVALID_VALUE",
            sys::GhosttyResult_GHOSTTY_INVALID_VALUE as i64,
            -2,
        ),
        (
            "GHOSTTY_OPTIMIZE_RELEASE_FAST",
            sys::GhosttyOptimizeMode_GHOSTTY_OPTIMIZE_RELEASE_FAST as i64,
            3,
        ),
        (
            "GHOSTTY_BUILD_INFO_SIMD",
            sys::GhosttyBuildInfo_GHOSTTY_BUILD_INFO_SIMD as i64,
            1,
        ),
        (
            "GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES",
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES as i64,
            8,
        ),
        (
            "GHOSTTY_POINT_TAG_HISTORY",
            sys::GhosttyPointTag_GHOSTTY_POINT_TAG_HISTORY as i64,
            3,
        ),
        (
            "GHOSTTY_STYLE_COLOR_RGB",
            sys::GhosttyStyleColorTag_GHOSTTY_STYLE_COLOR_RGB as i64,
            2,
        ),
        (
            "GHOSTTY_CELL_WIDE_SPACER_TAIL",
            sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_SPACER_TAIL as i64,
            2,
        ),
    ] {
        if actual != expected {
            return Err(GhosttyError::AbiMismatch(format!(
                "{name} discriminant expected {expected}, got {actual}"
            )));
        }
    }
    Ok(())
}

fn build_info_u32(kind: sys::GhosttyBuildInfo) -> Result<u32> {
    let mut value = 0u32;
    let result = unsafe { sys::ghostty_build_info(kind, (&mut value as *mut u32).cast()) };
    check("build_info", result)?;
    Ok(value)
}
