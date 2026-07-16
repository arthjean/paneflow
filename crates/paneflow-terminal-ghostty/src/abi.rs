use std::ffi::CStr;

use paneflow_libghostty_sys as sys;

use crate::handles::check;
use crate::{GhosttyError, Result};

pub(crate) fn validate() -> Result<()> {
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
    let layouts: serde_json::Value = serde_json::from_str(json)
        .map_err(|error| GhosttyError::AbiMismatch(format!("invalid layout JSON: {error}")))?;
    crate::abi_layout::validate(&layouts)
}

fn build_info_u32(kind: sys::GhosttyBuildInfo) -> Result<u32> {
    let mut value = 0u32;
    let result = unsafe { sys::ghostty_build_info(kind, (&mut value as *mut u32).cast()) };
    check("build_info", result)?;
    Ok(value)
}
