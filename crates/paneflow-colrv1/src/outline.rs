use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlineGlyphCollection, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};
use tiny_skia::{Path, PathBuilder};

pub trait OutlineSource {
    fn glyph_path(&self, glyph_id: GlyphId) -> Option<Path>;
}

pub struct FontOutlines<'a> {
    glyphs: OutlineGlyphCollection<'a>,
}

impl<'a> FontOutlines<'a> {
    pub fn new(font: &FontRef<'a>) -> Self {
        Self {
            glyphs: font.outline_glyphs(),
        }
    }
}

impl OutlineSource for FontOutlines<'_> {
    fn glyph_path(&self, glyph_id: GlyphId) -> Option<Path> {
        let glyph = self.glyphs.get(glyph_id)?;
        let mut pen = PathPen(PathBuilder::new());
        glyph
            .draw(
                DrawSettings::unhinted(Size::unscaled(), LocationRef::default()),
                &mut pen,
            )
            .ok()?;
        pen.0.finish()
    }
}

struct PathPen(PathBuilder);

impl OutlinePen for PathPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to(x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to(x, y);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.0.quad_to(cx0, cy0, x, y);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.0.cubic_to(cx0, cy0, cx1, cy1, x, y);
    }

    fn close(&mut self) {
        self.0.close();
    }
}
