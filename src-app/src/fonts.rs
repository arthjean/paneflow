#[cfg(windows)]
pub fn load_mono_fonts() -> Vec<String> {
    use std::collections::BTreeSet;

    use windows_sys::Win32::Foundation::LPARAM;
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DEFAULT_CHARSET, DeleteDC, EnumFontFamiliesExW, LOGFONTW, TEXTMETRICW,
        TMPF_FIXED_PITCH,
    };

    unsafe extern "system" fn collect_fixed_pitch_family(
        log_font: *const LOGFONTW,
        text_metric: *const TEXTMETRICW,
        _font_type: u32,
        families_ptr: LPARAM,
    ) -> i32 {
        if log_font.is_null() || text_metric.is_null() || families_ptr == 0 {
            return 1;
        }

        if unsafe { (*text_metric).tmPitchAndFamily } & TMPF_FIXED_PITCH != 0 {
            return 1;
        }

        let face = unsafe { &(*log_font).lfFaceName };
        let len = face
            .iter()
            .position(|code_unit| *code_unit == 0)
            .unwrap_or(face.len());
        let family = String::from_utf16_lossy(&face[..len]).trim().to_string();
        if !family.is_empty() && !family.starts_with('@') {
            unsafe {
                (&mut *(families_ptr as *mut BTreeSet<String>)).insert(family);
            }
        }

        1
    }

    let mut families = BTreeSet::from([
        "JetBrainsMono Nerd Font Mono".to_string(),
        "Geist Mono".to_string(),
        "IBM Plex Mono".to_string(),
        "Lilex".to_string(),
    ]);

    let mut filter: LOGFONTW = unsafe { std::mem::zeroed() };
    filter.lfCharSet = DEFAULT_CHARSET;

    let hdc = unsafe { CreateCompatibleDC(std::ptr::null_mut()) };
    if hdc.is_null() {
        log::warn!("fonts: CreateCompatibleDC failed; showing embedded fonts only");
        return families.into_iter().collect();
    }

    let result = unsafe {
        EnumFontFamiliesExW(
            hdc,
            &filter,
            Some(collect_fixed_pitch_family),
            (&mut families as *mut BTreeSet<String>) as LPARAM,
            0,
        )
    };
    unsafe {
        DeleteDC(hdc);
    }

    if result == 0 {
        log::warn!("fonts: EnumFontFamiliesExW failed; list may contain embedded fonts only");
    }

    families.into_iter().collect()
}

#[cfg(target_os = "macos")]
pub fn load_mono_fonts() -> Vec<String> {
    use std::collections::BTreeSet;

    use core_text::font as ct_font;
    use core_text::font_collection;
    use core_text::font_descriptor::SymbolicTraitAccessors;

    let collection = font_collection::create_for_all_families();
    let Some(descriptors) = collection.get_descriptors() else {
        log::warn!("Core Text font enumeration failed: no descriptors returned");
        return Vec::new();
    };

    let mut families: BTreeSet<String> = BTreeSet::new();
    for desc in descriptors.iter() {
        let font = ct_font::new_from_descriptor(&desc, 0.0);
        if font.symbolic_traits().is_monospace()
            && let Some(name) = lenient_font_attributes::family_name(&desc)
        {
            families.insert(name);
        }
    }

    families.into_iter().collect()
}

#[cfg(target_os = "macos")]
mod lenient_font_attributes {
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::string::CFString;
    use core_text::font_descriptor::{
        CTFontDescriptor, CTFontDescriptorCopyAttribute, kCTFontFamilyNameAttribute,
    };

    pub(super) fn family_name(descriptor: &CTFontDescriptor) -> Option<String> {
        unsafe {
            let value = CTFontDescriptorCopyAttribute(
                descriptor.as_concrete_TypeRef(),
                kCTFontFamilyNameAttribute,
            );
            if value.is_null() {
                return None;
            }
            CFType::wrap_under_create_rule(value)
                .downcast::<CFString>()
                .map(|s| s.to_string())
        }
    }
}

#[cfg(all(not(target_os = "macos"), not(windows)))]
pub fn load_mono_fonts() -> Vec<String> {
    use std::collections::BTreeSet;

    let output = match std::process::Command::new("fc-list")
        .args([":spacing=mono", "family"])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            log::warn!("fonts: fc-list failed: {e}");
            return Vec::new();
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut families = BTreeSet::new();

    for line in stdout.lines() {
        for part in line.split(',') {
            let name = part.trim();
            if !name.is_empty() {
                families.insert(name.to_string());
            }
        }
    }

    families.into_iter().collect()
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::*;

    #[test]
    fn core_text_returns_at_least_one_monospace_family() {
        let families = load_mono_fonts();
        assert!(
            !families.is_empty(),
            "expected at least one monospace family from Core Text, got none"
        );
    }

    #[test]
    fn core_text_includes_at_least_one_canonical_mono_family() {
        let families = load_mono_fonts();
        let canonical = ["Menlo", "Monaco", "Courier", "Courier New", "SF Mono"];
        let hit = canonical
            .iter()
            .find(|name| families.iter().any(|f| f == *name));
        assert!(
            hit.is_some(),
            "expected at least one of {:?} in enumerated families {:?}",
            canonical,
            families
        );
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn gdi_returns_embedded_and_system_monospace_families() {
        let families = load_mono_fonts();

        assert!(
            families
                .iter()
                .any(|family| family == "JetBrainsMono Nerd Font Mono")
        );
        assert!(families.iter().any(|family| family == "Geist Mono"));
        assert!(families.iter().any(|family| family == "IBM Plex Mono"));
        assert!(families.iter().any(|family| family == "Lilex"));
        assert!(
            families.iter().any(|family| matches!(
                family.as_str(),
                "Cascadia Mono" | "Consolas" | "Courier New"
            )),
            "expected a canonical Windows monospace family, got {families:?}"
        );
    }
}
