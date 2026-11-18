mod canvas;
mod color_line;
mod extent;
mod geometry;
mod outline;

use skrifa::color::ColorGlyphFormat;
use skrifa::instance::{LocationRef, Size};
use skrifa::raw::TableProvider;
use skrifa::{FontRef, GlyphId, MetadataProvider};
use tiny_skia::{Color, Transform};

use crate::canvas::Canvas;
use crate::extent::ExtentProbe;
use crate::outline::FontOutlines;

const MAX_SIDE: u32 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
}

impl Placement {
    fn enclosing(
        x_min: f32,
        y_min: f32,
        x_max: f32,
        y_max: f32,
        offset: (f32, f32),
    ) -> Option<Self> {
        let left = (x_min + offset.0).floor();
        let right = (x_max + offset.0).ceil();
        let bottom = (y_min + offset.1).floor();
        let top = (y_max + offset.1).ceil();
        if ![left, right, bottom, top]
            .iter()
            .all(|edge| edge.is_finite())
        {
            return None;
        }
        let width = right - left;
        let height = top - bottom;
        if width < 1.0 || height < 1.0 || width > MAX_SIDE as f32 || height > MAX_SIDE as f32 {
            return None;
        }
        Some(Self {
            left: left as i32,
            top: top as i32,
            width: width as u32,
            height: height as u32,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorGlyphImage {
    pub placement: Placement,
    pub rgba: Vec<u8>,
}

pub fn render_colr_v1_glyph(
    font_data: &[u8],
    face_index: u32,
    glyph_id: u32,
    pixel_size: f32,
    offset: (f32, f32),
) -> Option<ColorGlyphImage> {
    if !(pixel_size.is_finite() && pixel_size > 0.0) {
        return None;
    }
    let font = FontRef::from_index(font_data, face_index).ok()?;
    let glyph = font
        .color_glyphs()
        .get_with_format(GlyphId::new(glyph_id), ColorGlyphFormat::ColrV1)?;
    let units_per_em = font.head().ok()?.units_per_em();
    if units_per_em == 0 {
        return None;
    }
    let scale = pixel_size / f32::from(units_per_em);
    let outlines = FontOutlines::new(&font);

    let placement = match glyph.bounding_box(LocationRef::default(), Size::new(pixel_size)) {
        Some(clip) => Placement::enclosing(clip.x_min, clip.y_min, clip.x_max, clip.y_max, offset)?,
        None => {
            let mut probe = ExtentProbe::new(Transform::from_scale(scale, scale), &outlines);
            glyph.paint(LocationRef::default(), &mut probe).ok()?;
            let painted = probe.painted()?;
            Placement::enclosing(
                painted.left(),
                painted.top(),
                painted.right(),
                painted.bottom(),
                offset,
            )?
        }
    };

    let palette: Vec<Color> = font
        .color_palettes()
        .get(0)
        .map(|palette| {
            palette
                .colors()
                .iter()
                .map(|record| {
                    Color::from_rgba8(record.red, record.green, record.blue, record.alpha)
                })
                .collect()
        })
        .unwrap_or_default();
    let root = Transform::from_row(
        scale,
        0.0,
        0.0,
        -scale,
        offset.0 - placement.left as f32,
        placement.top as f32 - offset.1,
    );
    let mut canvas = Canvas::new(
        placement.width,
        placement.height,
        root,
        &palette,
        Color::BLACK,
        &outlines,
    )?;
    glyph.paint(LocationRef::default(), &mut canvas).ok()?;
    let pixmap = canvas.into_pixmap();
    let rgba = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let straight = pixel.demultiply();
            [
                straight.red(),
                straight.green(),
                straight.blue(),
                straight.alpha(),
            ]
        })
        .collect();
    Some(ColorGlyphImage { placement, rgba })
}
