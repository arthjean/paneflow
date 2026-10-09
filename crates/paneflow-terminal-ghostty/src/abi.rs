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
    let document = type_json()?;
    let types = layout_types(&document)?;
    validate_discriminants(types, crate::abi_discriminants::READ_DISCRIMINANTS)?;
    crate::abi_layout::validate(types)
}

fn type_json() -> Result<serde_json::Value> {
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
    serde_json::from_str(json)
        .map_err(|error| GhosttyError::AbiMismatch(format!("invalid layout JSON: {error}")))
}

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

fn validate_discriminants(types: &serde_json::Value, discriminants: &[(&str, i64)]) -> Result<()> {
    for &(binding, actual) in discriminants {
        let (enum_name, value_name) = binding
            .find("_GHOSTTY_")
            .map(|index| (&binding[..index], &binding[index + 1..]))
            .ok_or_else(|| {
                GhosttyError::AbiMismatch(format!("{binding} is not an enum value binding"))
            })?;
        let descriptor = types
            .get(enum_name)
            .ok_or_else(|| GhosttyError::AbiMismatch(format!("type JSON has no enum {enum_name}")))?;
        let key = descriptor
            .get("prefix")
            .and_then(serde_json::Value::as_str)
            .and_then(|prefix| value_name.strip_prefix(prefix))
            .ok_or_else(|| {
                GhosttyError::AbiMismatch(format!(
                    "{enum_name} prefix in the type JSON does not lead {value_name}"
                ))
            })?;
        let expected = descriptor
            .get("values")
            .and_then(|values| values.get(key))
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| {
                GhosttyError::AbiMismatch(format!("{enum_name} has no value {value_name} in the type JSON"))
            })?;
        if actual != expected {
            return Err(GhosttyError::AbiMismatch(format!(
                "{enum_name} value {value_name} expected {expected} from the type JSON, got {actual}"
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

#[cfg(test)]
mod tests {
    use super::*;

    fn pinned_types() -> serde_json::Value {
        let document = type_json().expect("the pinned library has a type JSON");
        layout_types(&document).expect("type JSON has a types map").clone()
    }

    #[test]
    fn every_discriminant_paneflow_reads_matches_the_type_json() {
        validate_discriminants(&pinned_types(), crate::abi_discriminants::READ_DISCRIMINANTS)
            .expect("the bindings must match the linked library");
        validate().expect("the pinned library must validate");
    }

    #[test]
    fn a_renumbered_enum_value_is_an_abi_mismatch_naming_the_enum_and_value() {
        for (enum_name, key, binding) in [
            (
                "GhosttyProgramStatusKind",
                "PERMISSION",
                "GHOSTTY_PROGRAM_STATUS_KIND_PERMISSION",
            ),
            ("GhosttyOscTerminator", "ST", "GHOSTTY_OSC_TERMINATOR_ST"),
            (
                "GhosttySemanticPromptPromptKind",
                "RIGHT",
                "GHOSTTY_SEMANTIC_PROMPT_PROMPT_RIGHT",
            ),
            (
                "GhosttyRenderStateData",
                "OVERSCAN_REQUEST",
                "GHOSTTY_RENDER_STATE_DATA_OVERSCAN_REQUEST",
            ),
        ] {
            let mut types = pinned_types();
            let value = &mut types[enum_name]["values"][key];
            let renumbered = value.as_i64().expect("enum value") + 100;
            *value = serde_json::Value::from(renumbered);

            let error = validate_discriminants(&types, crate::abi_discriminants::READ_DISCRIMINANTS)
                .expect_err("a renumbered value must not validate");
            let expected = format!("{enum_name} value {binding} expected {renumbered}");
            assert!(
                matches!(&error, GhosttyError::AbiMismatch(message) if message.starts_with(&expected)),
                "{error:?}"
            );
        }
    }

    #[test]
    fn a_missing_enum_value_is_an_abi_mismatch() {
        let mut types = pinned_types();
        types["GhosttyOscTerminator"]["values"]
            .as_object_mut()
            .expect("enum values")
            .remove("BEL");

        let error = validate_discriminants(&types, crate::abi_discriminants::READ_DISCRIMINANTS)
            .expect_err("a missing value must not validate");
        assert!(
            matches!(
                &error,
                GhosttyError::AbiMismatch(message)
                    if message == "GhosttyOscTerminator has no value GHOSTTY_OSC_TERMINATOR_BEL in the type JSON"
            ),
            "{error:?}"
        );
    }

    #[test]
    fn every_discriminant_the_crate_reads_is_validated() {
        let binding = regex::Regex::new(r"Ghostty[A-Za-z]+_GHOSTTY_[A-Z0-9_]+").expect("regex");
        let validated: std::collections::BTreeSet<&str> = crate::abi_discriminants::READ_DISCRIMINANTS
            .iter()
            .map(|&(name, _)| name)
            .collect();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut pending = vec![root.join("src"), root.join("tests"), root.join("benches")];
        let mut unvalidated = std::collections::BTreeSet::new();
        while let Some(path) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&path) else {
                continue;
            };
            for entry in entries {
                let path = entry.expect("directory entry").path();
                if path.is_dir() {
                    pending.push(path);
                    continue;
                }
                if path.extension().is_none_or(|extension| extension != "rs")
                    || path.file_name().is_some_and(|name| name == "abi_discriminants.rs")
                {
                    continue;
                }
                let source = std::fs::read_to_string(&path).expect("source file");
                for found in binding.find_iter(&source) {
                    if !validated.contains(found.as_str()) {
                        unvalidated.insert(format!("{} in {}", found.as_str(), path.display()));
                    }
                }
            }
        }
        assert!(
            unvalidated.is_empty(),
            "add these discriminants to abi_discriminants.rs: {unvalidated:#?}"
        );
    }
}
