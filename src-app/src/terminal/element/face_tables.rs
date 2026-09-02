//! Font tables read straight from the embedded faces.
//!
//! GPUI's `TextSystem` exposes ascent, descent, x-height, cap height, and
//! advances, but not the line gap, the `post` underline metrics, the `OS/2`
//! strikeout metrics, or a glyph's ink bounds. Ghostty sizes its grid and
//! constrains its Nerd Font icons from exactly those tables
//! (`src/font/Metrics.zig`, `src/font/Glyph.zig`), so the terminal parses the
//! bundled `.ttf` files itself. A system font the user configured has no
//! bytes here and falls back to the estimators in `font.rs`.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use ttf_parser::{Face, name_id};

struct EmbeddedFace {
    family: String,
    /// Upright, regular-weight face: the one the grid is measured on.
    regular: bool,
    data: Cow<'static, [u8]>,
}

static EMBEDDED_FACES: LazyLock<Vec<EmbeddedFace>> = LazyLock::new(load_embedded_faces);

fn load_embedded_faces() -> Vec<EmbeddedFace> {
    let mut faces = Vec::new();
    for path in crate::assets::Assets::iter() {
        let lower = path.to_lowercase();
        if !(lower.ends_with(".ttf") || lower.ends_with(".otf")) {
            continue;
        }
        let Some(file) = crate::assets::Assets::get(&path) else {
            continue;
        };
        let data = file.data;
        let Ok(face) = Face::parse(&data, 0) else {
            log::warn!("face_tables: embedded font {path} did not parse; skipping");
            continue;
        };
        let Some(family) = family_name(&face) else {
            log::warn!("face_tables: embedded font {path} has no family name; skipping");
            continue;
        };
        let regular =
            !face.is_italic() && !face.is_bold() && face.weight() == ttf_parser::Weight::Normal;
        faces.push(EmbeddedFace {
            family,
            regular,
            data,
        });
    }
    faces
}

/// Typographic family (name ID 16) when present, else the legacy family
/// (name ID 1): the same precedence GPUI's font databases use to register the
/// face, so the name the renderer resolves is the name looked up here.
fn family_name(face: &Face<'_>) -> Option<String> {
    let mut legacy = None;
    for name in face.names() {
        match name.name_id {
            name_id::TYPOGRAPHIC_FAMILY => {
                if let Some(value) = name.to_string() {
                    return Some(value);
                }
            }
            name_id::FAMILY if legacy.is_none() => {
                legacy = name.to_string();
            }
            _ => {}
        }
    }
    legacy
}

fn embedded_face(family: &str) -> Option<&'static EmbeddedFace> {
    let faces = &*EMBEDDED_FACES;
    faces
        .iter()
        .find(|face| face.regular && face.family == family)
        .or_else(|| faces.iter().find(|face| face.family == family))
}

/// Vertical and horizontal metrics of a face, in font units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FaceTables {
    pub units_per_em: f32,
    pub ascent: f32,
    /// Positive magnitude below the baseline.
    pub descent: f32,
    pub line_gap: f32,
    /// Widest advance among the printable ASCII glyphs.
    pub advance: f32,
    /// Top of the underline relative to the baseline, negative below it.
    /// Zero when the `post` table carries nothing usable.
    pub underline_position: f32,
    pub underline_thickness: f32,
    /// Top of the strikeout stroke above the baseline, zero when absent.
    pub strikethrough_position: f32,
    pub strikethrough_thickness: f32,
    pub x_height: f32,
    pub cap_height: f32,
}

fn read_face_tables(face: &Face<'_>) -> FaceTables {
    let advance = (0x20u32..0x7f)
        .filter_map(char::from_u32)
        .filter_map(|ch| face.glyph_index(ch))
        .filter_map(|glyph| face.glyph_hor_advance(glyph))
        .map(f32::from)
        .fold(0.0, f32::max);
    let underline = face.underline_metrics();
    let strikeout = face.strikeout_metrics();
    FaceTables {
        units_per_em: f32::from(face.units_per_em()),
        ascent: f32::from(face.ascender()),
        descent: f32::from(face.descender()).abs(),
        line_gap: f32::from(face.line_gap()),
        advance,
        underline_position: underline.map_or(0.0, |m| f32::from(m.position)),
        underline_thickness: underline.map_or(0.0, |m| f32::from(m.thickness)),
        strikethrough_position: strikeout.map_or(0.0, |m| f32::from(m.position)),
        strikethrough_thickness: strikeout.map_or(0.0, |m| f32::from(m.thickness)),
        x_height: face.x_height().map_or(0.0, f32::from),
        cap_height: face.capital_height().map_or(0.0, f32::from),
    }
}

static FACE_TABLES: LazyLock<Mutex<HashMap<String, Option<FaceTables>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Tables of the embedded `family`, or `None` when the family is not bundled.
pub(crate) fn embedded_face_tables(family: &str) -> Option<FaceTables> {
    let mut cache = FACE_TABLES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = cache.get(family) {
        return *entry;
    }
    let tables = embedded_face(family)
        .and_then(|face| Face::parse(&face.data, 0).ok())
        .map(|face| read_face_tables(&face));
    cache.insert(family.to_owned(), tables);
    tables
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::element::font::EMBEDDED_MONO_FAMILY;

    #[test]
    fn embedded_mono_family_is_measurable() {
        let tables = embedded_face_tables(EMBEDDED_MONO_FAMILY).expect("bundled family parses");
        assert_eq!(tables.units_per_em, 1000.0);
        // JetBrains Mono: 0.6 em advance, 1.32 em face (1020 + 300 + 0).
        assert_eq!(tables.advance, 600.0);
        assert_eq!(tables.ascent, 1020.0);
        assert_eq!(tables.descent, 300.0);
        assert_eq!(tables.line_gap, 0.0);
        assert!(tables.underline_thickness > 0.0);
        assert!(tables.underline_position < 0.0);
        assert!(tables.strikethrough_position > 0.0);
        assert!(tables.x_height > 0.0 && tables.cap_height > tables.x_height);
    }

    #[test]
    fn unknown_family_has_no_tables() {
        assert!(embedded_face_tables("Definitely Not Bundled").is_none());
    }
}
