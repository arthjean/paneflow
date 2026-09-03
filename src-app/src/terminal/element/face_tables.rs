use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use ttf_parser::{Face, name_id};

struct EmbeddedFace {
    family: String,
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FaceTables {
    pub units_per_em: f32,
    pub ascent: f32,
    pub descent: f32,
    pub line_gap: f32,
    pub advance: f32,
    pub underline_position: f32,
    pub underline_thickness: f32,
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GlyphInk {
    pub units_per_em: f32,
    pub x_min: f32,
    pub y_min: f32,
    pub x_max: f32,
    pub y_max: f32,
}

impl GlyphInk {
    pub fn width(&self) -> f32 {
        self.x_max - self.x_min
    }

    pub fn height(&self) -> f32 {
        self.y_max - self.y_min
    }
}

type GlyphInkCache = HashMap<(String, char), Option<GlyphInk>>;

static GLYPH_INK: LazyLock<Mutex<GlyphInkCache>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub(crate) fn embedded_glyph_ink(family: &str, ch: char) -> Option<GlyphInk> {
    let mut cache = GLYPH_INK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = cache.get(&(family.to_owned(), ch)) {
        return *entry;
    }
    let ink = embedded_face(family)
        .and_then(|face| Face::parse(&face.data, 0).ok())
        .and_then(|face| {
            let glyph = face.glyph_index(ch)?;
            let rect = face.glyph_bounding_box(glyph)?;
            Some(GlyphInk {
                units_per_em: f32::from(face.units_per_em()),
                x_min: f32::from(rect.x_min),
                y_min: f32::from(rect.y_min),
                x_max: f32::from(rect.x_max),
                y_max: f32::from(rect.y_max),
            })
        });
    cache.insert((family.to_owned(), ch), ink);
    ink
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::element::font::EMBEDDED_MONO_FAMILY;

    #[test]
    fn embedded_mono_family_is_measurable() {
        let tables = embedded_face_tables(EMBEDDED_MONO_FAMILY).expect("bundled family parses");
        assert_eq!(tables.units_per_em, 1000.0);
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
        assert!(embedded_glyph_ink("Definitely Not Bundled", 'a').is_none());
    }

    #[test]
    fn nerd_font_icon_ink_overflows_its_advance() {
        let ink = embedded_glyph_ink(EMBEDDED_MONO_FAMILY, '\u{f09b}').expect("icon outline");
        assert!(
            ink.width() > 600.0,
            "icon width {} should exceed the advance",
            ink.width()
        );
        let latin = embedded_glyph_ink(EMBEDDED_MONO_FAMILY, 'm').expect("latin outline");
        assert!(latin.width() <= 600.0);
    }
}
