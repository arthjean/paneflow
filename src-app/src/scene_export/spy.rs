use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use gpui::{
    AtlasKey, AtlasTile, Bounds, DevicePixels, Font, FontId, FontMetrics, FontRun, GlyphId, Hsla,
    LineLayout, MissingGlyphSink, MonochromeSprite, PaintedPath, PlatformAtlas,
    PlatformHeadlessRenderer, PlatformTextSystem, PolychromeSprite, Quad, RenderGlyphParams,
    RenderImageParams, RenderSvgParams, Scene, Shadow, Size, SubpixelSprite, TextRenderingMode,
    Underline,
};
use parking_lot::Mutex;

#[derive(Clone, Debug)]
pub(crate) struct RecordedGlyph {
    pub font_id: usize,
    pub glyph_id: u32,
    pub x: f32,
    pub index: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct RecordedLayout {
    pub text: String,
    pub font_size: f32,
    pub glyphs: Vec<RecordedGlyph>,
}

pub(crate) struct SpyTextSystem {
    inner: gpui_wgpu::CosmicTextSystem,
    pub fonts: Mutex<HashMap<usize, Font>>,
    pub layouts: Mutex<Vec<RecordedLayout>>,
}

impl SpyTextSystem {
    pub fn new(inner: gpui_wgpu::CosmicTextSystem) -> Self {
        Self {
            inner,
            fonts: Mutex::default(),
            layouts: Mutex::default(),
        }
    }
}

type Inner = gpui_wgpu::CosmicTextSystem;

impl PlatformTextSystem for SpyTextSystem {
    fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
        <Inner as PlatformTextSystem>::add_fonts(&self.inner, fonts)
    }
    fn set_missing_glyph_sink(&self, sink: Option<Arc<dyn MissingGlyphSink>>) {
        <Inner as PlatformTextSystem>::set_missing_glyph_sink(&self.inner, sink)
    }
    fn all_font_names(&self) -> Vec<String> {
        <Inner as PlatformTextSystem>::all_font_names(&self.inner)
    }
    fn font_id(&self, descriptor: &Font) -> Result<FontId> {
        let id = <Inner as PlatformTextSystem>::font_id(&self.inner, descriptor)?;
        self.fonts.lock().insert(id.0, descriptor.clone());
        Ok(id)
    }
    fn prewarm_fonts(&self, font_ids: &[FontId]) {
        <Inner as PlatformTextSystem>::prewarm_fonts(&self.inner, font_ids)
    }
    fn font_metrics(&self, font_id: FontId) -> FontMetrics {
        <Inner as PlatformTextSystem>::font_metrics(&self.inner, font_id)
    }
    fn typographic_bounds(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Bounds<f32>> {
        <Inner as PlatformTextSystem>::typographic_bounds(&self.inner, font_id, glyph_id)
    }
    fn advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Size<f32>> {
        <Inner as PlatformTextSystem>::advance(&self.inner, font_id, glyph_id)
    }
    fn glyph_for_char(&self, font_id: FontId, ch: char) -> Option<GlyphId> {
        <Inner as PlatformTextSystem>::glyph_for_char(&self.inner, font_id, ch)
    }
    fn glyph_raster_bounds(&self, params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
        <Inner as PlatformTextSystem>::glyph_raster_bounds(&self.inner, params)
    }
    fn rasterize_glyph(
        &self,
        params: &RenderGlyphParams,
        raster_bounds: Bounds<DevicePixels>,
    ) -> Result<(Size<DevicePixels>, Vec<u8>)> {
        <Inner as PlatformTextSystem>::rasterize_glyph(&self.inner, params, raster_bounds)
    }
    fn layout_line(&self, text: &str, font_size: gpui::Pixels, runs: &[FontRun]) -> LineLayout {
        let layout = <Inner as PlatformTextSystem>::layout_line(&self.inner, text, font_size, runs);
        let glyphs = layout
            .runs
            .iter()
            .flat_map(|run| {
                run.glyphs.iter().map(move |glyph| RecordedGlyph {
                    font_id: run.font_id.0,
                    glyph_id: glyph.id.0,
                    x: glyph.position.x.as_f32(),
                    index: glyph.index,
                })
            })
            .collect();
        self.layouts.lock().push(RecordedLayout {
            text: text.to_string(),
            font_size: font_size.as_f32(),
            glyphs,
        });
        layout
    }
    fn recommended_rendering_mode(
        &self,
        font_id: FontId,
        font_size: gpui::Pixels,
    ) -> TextRenderingMode {
        <Inner as PlatformTextSystem>::recommended_rendering_mode(&self.inner, font_id, font_size)
    }
    fn glyph_dilation_for_color(&self, color: Hsla) -> u8 {
        <Inner as PlatformTextSystem>::glyph_dilation_for_color(&self.inner, color)
    }
}

pub(crate) type TileKey = (u32, u32, u32);

pub(crate) fn tile_key(tile: &AtlasTile) -> TileKey {
    (
        tile.texture_id.index,
        tile.texture_id.kind as u32,
        tile.tile_id.0,
    )
}

#[derive(Clone)]
pub(crate) enum TileSource {
    Glyph(RenderGlyphParams),
    Svg(RenderSvgParams),
    Image(RenderImageParams),
}

pub(crate) struct SpyAtlas {
    inner: Arc<dyn PlatformAtlas>,
    pub tiles: Mutex<HashMap<TileKey, TileSource>>,
}

impl PlatformAtlas for SpyAtlas {
    fn get_or_insert_with<'a>(
        &self,
        key: AtlasKey,
        build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> Result<Option<AtlasTile>> {
        let source = match &key {
            AtlasKey::Glyph(params) => TileSource::Glyph(params.clone()),
            AtlasKey::Svg(params) => TileSource::Svg(params.clone()),
            AtlasKey::Image(params) => TileSource::Image(params.clone()),
        };
        let tile = self.inner.get_or_insert_with(key, build)?;
        if let Some(tile) = &tile {
            self.tiles.lock().insert(tile_key(tile), source);
        }
        Ok(tile)
    }
    fn remove(&self, key: &AtlasKey) {
        self.inner.remove(key)
    }
    fn contains(&self, key: &AtlasKey) -> bool {
        self.inner.contains(key)
    }
}

#[derive(Clone, Default)]
pub(crate) struct CapturedScene {
    pub shadows: Vec<Shadow>,
    pub quads: Vec<Quad>,
    pub paths: Vec<PaintedPath>,
    pub underlines: Vec<Underline>,
    pub monochrome: Vec<MonochromeSprite>,
    pub subpixel: Vec<SubpixelSprite>,
    pub polychrome: Vec<PolychromeSprite>,
    pub surfaces: usize,
}

struct SpyRenderer {
    inner: gpui_wgpu::WgpuHeadlessRenderer,
    atlas: Arc<SpyAtlas>,
    sink: Arc<Mutex<Option<CapturedScene>>>,
    target: Size<DevicePixels>,
}

impl PlatformHeadlessRenderer for SpyRenderer {
    fn render_scene_to_image(
        &mut self,
        scene: &Scene,
        _size: Size<DevicePixels>,
    ) -> Result<image::RgbaImage> {
        *self.sink.lock() = Some(CapturedScene {
            shadows: scene.shadows.clone(),
            quads: scene.quads.clone(),
            paths: scene.paths.clone(),
            underlines: scene.underlines.clone(),
            monochrome: scene.monochrome_sprites.clone(),
            subpixel: scene.subpixel_sprites.clone(),
            polychrome: scene.polychrome_sprites.clone(),
            surfaces: scene.surfaces.len(),
        });
        self.inner.render_scene_to_image(scene, self.target)
    }
    fn render_scene(&mut self, scene: &Scene, _size: Size<DevicePixels>) -> Result<()> {
        self.inner.render_scene(scene, self.target)
    }
    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.atlas.clone()
    }
}

#[derive(Clone, Default)]
pub(crate) struct Recorder {
    pub scene: Arc<Mutex<Option<CapturedScene>>>,
    pub atlas: Arc<Mutex<Option<Arc<SpyAtlas>>>>,
}

impl Recorder {
    pub fn renderer_factory(
        &self,
        target: Size<DevicePixels>,
    ) -> impl Fn() -> Result<Option<Box<dyn PlatformHeadlessRenderer>>> + 'static {
        let recorder = self.clone();
        move || {
            let inner = gpui_wgpu::WgpuHeadlessRenderer::new()?;
            #[expect(
                clippy::arc_with_non_send_sync,
                reason = "PlatformHeadlessRenderer::sprite_atlas hands out an Arc"
            )]
            let atlas = Arc::new(SpyAtlas {
                inner: inner.sprite_atlas(),
                tiles: Mutex::default(),
            });
            *recorder.atlas.lock() = Some(atlas.clone());
            Ok(Some(Box::new(SpyRenderer {
                inner,
                atlas,
                sink: recorder.scene.clone(),
                target,
            }) as Box<dyn PlatformHeadlessRenderer>))
        }
    }
}
