//! Monospace font family enumeration.
//!
//! Per-OS strategy:
//!
//! - **Linux / BSDs** → `fc-list :spacing=mono family` (fontconfig). Widely
//!   available, fast, and returns the deduplicated family list we need.
//! - **macOS** (US-012) → Core Text via the `core-text = "21"` crate. The
//!   previous shared `fc-list` branch worked only when Homebrew's
//!   fontconfig was installed - a fresh macOS lacks it entirely, leaving
//!   the settings font picker empty. Core Text is macOS-native, has no
//!   install requirement, and returns the same family strings the OS
//!   already knows about (SF Mono, Menlo, Monaco, Courier, etc.).
//! - **Windows** → GDI `EnumFontFamiliesExW`, filtering the callback's
//!   `TEXTMETRICW` to fixed-pitch families. GDI is used only for discovery;
//!   GPUI/DirectWrite still owns rendering.
//!
//! All three branches share the contract: `Ok`-shaped return even on
//! failure. An empty list + `log::warn!` keeps the settings picker
//! renderable; panicking here would cascade into the whole settings
//! window failing to open.

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

        // Win32's historical flag is inverted: set means variable pitch,
        // clear means fixed pitch.
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

    // Embedded families are available to GPUI even though GDI cannot enumerate
    // application-registered font bytes.
    let mut families = BTreeSet::from([
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

/// macOS implementation (US-012).
///
/// Calls `CTFontCollectionCreateFromAvailableFonts` under the hood via
/// `core_text::font_collection::create_for_all_families`, iterates the
/// returned descriptors, instantiates a zero-sized `CTFont` per descriptor
/// (cheap - no glyph tables loaded at size 0), checks the symbolic
/// `kCTFontMonoSpaceTrait` bit (`0x00000400`) via `SymbolicTraitAccessors::is_monospace`,
/// and collects unique family names into a `BTreeSet` for stable ordering.
///
/// **Failure mode** (US-012 AC7): if Core Text returns no descriptors
/// (sandbox restriction, framework not loaded, OS bug) we return
/// `Vec::new()` and log a warning - the picker shows empty rather than
/// panicking. Mirrors the Linux `fc-list` branch's fallback semantics.
///
/// **Performance**: Core Text caches the font collection internally, so
/// subsequent calls return in well under the PRD's 200 ms warm-cache
/// budget. Cold-start cost on a system with a few hundred fonts is
/// typically ~30 ms on Apple Silicon - comfortably inside budget.
#[cfg(target_os = "macos")]
pub fn load_mono_fonts() -> Vec<String> {
    use std::collections::BTreeSet;

    use core_text::font as ct_font;
    use core_text::font_collection;
    use core_text::font_descriptor::SymbolicTraitAccessors;

    let collection = font_collection::create_for_all_families();
    // NOTE: core-text 21's `get_descriptors()` has a documented one-shot leak -
    // it wraps the `CTFontCollectionCreateMatchingFontDescriptors` result under
    // the Get rule when Apple returns it under the Create rule, so the CFArray
    // is never released. Accepted here: this runs at most once per process
    // (memoized by the `INSTALLED_MONO_FONTS` `LazyLock`) and leaks only a few
    // KB. If it ever moves off the one-shot path, mirror Zed's direct FFI
    // (`extern "C"` + `wrap_under_create_rule`, gpui_macos/src/text_system.rs).
    let Some(descriptors) = collection.get_descriptors() else {
        log::warn!("Core Text font enumeration failed: no descriptors returned");
        return Vec::new();
    };

    let mut families: BTreeSet<String> = BTreeSet::new();
    for desc in descriptors.iter() {
        // `new_from_descriptor(&CTFontDescriptor, f64)` at size 0.0 is
        // the documented cheap-instantiation idiom - Core Text lazy-loads
        // tables only when the font is actually rendered. Dereferencing
        // `desc` (a `core_foundation::ItemRef`) yields `&CTFontDescriptor`
        // which matches the constructor's signature.
        let font = ct_font::new_from_descriptor(&desc, 0.0);
        // `desc.family_name()` (core-text) panics via an internal `.expect()`
        // on a font with no family-name attribute, which would poison the
        // `LazyLock` registry and trip `panic = "deny"`. Use the panic-free
        // reader and silently skip any descriptor without a usable name.
        if font.symbolic_traits().is_monospace()
            && let Some(name) = lenient_font_attributes::family_name(&desc)
        {
            families.insert(name);
        }
    }

    families.into_iter().collect()
}

/// Panic-free Core Text family-name read.
///
/// `core_text`'s `CTFontDescriptor::family_name` does `.expect(...)` on the
/// attribute and asserts it is a `CFString`, so a single installed font with an
/// absent or malformed family-name attribute panics - poisoning the `LazyLock`
/// registry (every later read re-panics) and tripping the workspace's
/// `panic = "deny"` lint. Mirrors Zed's `lenient_font_attributes`, but
/// `downcast`s instead of asserting so it never panics on a non-string value.
#[cfg(target_os = "macos")]
mod lenient_font_attributes {
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::string::CFString;
    use core_text::font_descriptor::{
        CTFontDescriptor, CTFontDescriptorCopyAttribute, kCTFontFamilyNameAttribute,
    };

    pub(super) fn family_name(descriptor: &CTFontDescriptor) -> Option<String> {
        // SAFETY: `CTFontDescriptorCopyAttribute` returns a +1 (Create-rule)
        // `CFTypeRef` or NULL. We null-check, take ownership under the create
        // rule, then `downcast` - never dereferencing NULL or a wrong-typed
        // object, and never leaking (the temporary `CFType` releases the +1 if
        // the downcast fails).
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

/// Linux / FreeBSD / OpenBSD / NetBSD / other unixes.
///
/// Keeps the `fc-list` path that was shared with macOS pre-US-012. Any
/// unix target that isn't macOS lands here; if fontconfig isn't present
/// the spawn fails cleanly and the picker renders empty (same
/// fallback-on-failure invariant as the macOS and Windows branches).
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
        // fc-list may output "Family1,Family2" on a single line
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

    /// Smoke test: Core Text on any real macOS install ships at least
    /// one monospace family (Menlo is bundled back to 10.6). An empty
    /// result means either Core Text is broken or the filter is wrong
    /// either way, a regression the CI `macos-check` leg catches.
    #[test]
    fn core_text_returns_at_least_one_monospace_family() {
        let families = load_mono_fonts();
        assert!(
            !families.is_empty(),
            "expected at least one monospace family from Core Text, got none"
        );
    }

    /// The canonical macOS monospace fonts that ship with every release
    /// back to 10.6 - if none of these are present, the enumeration is
    /// almost certainly broken (or the filter is mis-classifying).
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
