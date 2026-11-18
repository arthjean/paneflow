use std::sync::Arc;

use gpui::{
    Bounds, DevicePixels, FontId, FontRun, GlyphId, PlatformTextSystem, RenderGlyphParams, Size, px,
};

const FONT_SIZE: f32 = 16.0;
const SCALE_FACTOR: f32 = 2.0;

struct EmojiBitmap {
    bounds: Bounds<DevicePixels>,
    size: Size<DevicePixels>,
    bgra: Vec<u8>,
}

impl EmojiBitmap {
    fn opaque_rgb(&self) -> Vec<[u8; 3]> {
        self.bgra
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|[_, _, _, alpha]| *alpha >= 200)
            .map(|[blue, green, red, _]| [*red, *green, *blue])
            .collect()
    }

    fn rgba_at(&self, x: i32, y: i32) -> [u8; 4] {
        let index = (y * self.size.width.0 + x) as usize * 4;
        [
            self.bgra[index + 2],
            self.bgra[index + 1],
            self.bgra[index],
            self.bgra[index + 3],
        ]
    }
}

fn share(pixels: &[[u8; 3]], predicate: impl Fn(&[u8; 3]) -> bool) -> f32 {
    pixels.iter().filter(|pixel| predicate(pixel)).count() as f32 / pixels.len().max(1) as f32
}

fn platform_text_system() -> Option<Arc<dyn PlatformTextSystem>> {
    let text_system =
        std::panic::catch_unwind(|| gpui_platform::current_platform(true).text_system()).ok()?;
    let fonts = crate::assets::Assets
        .embedded_fonts()
        .expect("the embedded fonts load from the binary");
    text_system
        .add_fonts(fonts)
        .expect("the platform text system registers the embedded fonts");
    Some(text_system)
}

fn emoji_glyphs(text_system: &dyn PlatformTextSystem, text: &str) -> Vec<(FontId, GlyphId)> {
    let font_id = text_system
        .font_id(&gpui::font("IBM Plex Mono"))
        .expect("IBM Plex Mono is embedded");
    let layout = text_system.layout_line(
        text,
        px(FONT_SIZE),
        &[FontRun {
            len: text.len(),
            font_id,
        }],
    );
    layout
        .runs
        .iter()
        .flat_map(|run| {
            run.glyphs
                .iter()
                .filter(|glyph| glyph.is_emoji)
                .map(move |glyph| (run.font_id, glyph.id))
        })
        .collect()
}

fn rasterize(
    text_system: &dyn PlatformTextSystem,
    font_id: FontId,
    glyph_id: GlyphId,
) -> EmojiBitmap {
    let params = RenderGlyphParams {
        font_id,
        glyph_id,
        font_size: px(FONT_SIZE),
        subpixel_variant: Default::default(),
        scale_factor: SCALE_FACTOR,
        is_emoji: true,
        subpixel_rendering: false,
        dilation: 0,
    };
    let bounds = text_system
        .glyph_raster_bounds(&params)
        .expect("an emoji glyph has raster bounds");
    assert!(
        bounds.size.width.0 > 0 && bounds.size.height.0 > 0,
        "the emoji glyph rasterizes to an empty cell: {bounds:?}"
    );
    let (size, bgra) = text_system
        .rasterize_glyph(&params, bounds)
        .expect("an emoji glyph with bounds rasterizes");
    EmojiBitmap { bounds, size, bgra }
}

fn single_emoji(text_system: &dyn PlatformTextSystem, text: &str) -> Option<EmojiBitmap> {
    let glyphs = emoji_glyphs(text_system, text);
    if glyphs.is_empty() {
        eprintln!("skipped: no color emoji font on this system covers {text:?}");
        return None;
    }
    assert_eq!(glyphs.len(), 1, "{text:?} shapes to one emoji glyph");
    let (font_id, glyph_id) = glyphs[0];
    Some(rasterize(text_system, font_id, glyph_id))
}

fn assert_cell_sized(bitmap: &EmojiBitmap) {
    let device_em = FONT_SIZE * SCALE_FACTOR;
    assert_eq!(bitmap.size, bitmap.bounds.size);
    assert_eq!(
        bitmap.bgra.len(),
        (bitmap.size.width.0 * bitmap.size.height.0 * 4) as usize,
        "the polychrome atlas expects four bytes per pixel"
    );
    assert!(
        (bitmap.size.width.0 as f32) < device_em * 1.5
            && (bitmap.size.height.0 as f32) < device_em * 1.5
            && (bitmap.size.height.0 as f32) > device_em * 0.75,
        "the emoji is not scaled to the font size: {:?}",
        bitmap.bounds
    );
    assert!(
        bitmap.bounds.origin.y.0 < 0 && bitmap.bounds.origin.y.0 + bitmap.size.height.0 > 0,
        "the emoji does not straddle the baseline: {:?}",
        bitmap.bounds
    );
}

#[test]
fn thumbs_up_rasterizes_as_a_yellow_color_glyph() {
    let Some(text_system) = platform_text_system() else {
        eprintln!("skipped: no platform text system");
        return;
    };
    let Some(bitmap) = single_emoji(text_system.as_ref(), "\u{1F44D}") else {
        return;
    };
    assert_cell_sized(&bitmap);
    let opaque = bitmap.opaque_rgb();
    let pixels = (bitmap.size.width.0 * bitmap.size.height.0) as f32;
    assert!(
        opaque.len() as f32 / pixels > 0.3,
        "the thumb is mostly transparent"
    );
    let yellow = share(&opaque, |[r, g, b]| *r >= 200 && *g >= 140 && *b <= 100);
    assert!(yellow > 0.6, "only {yellow} of the thumb is yellow");
}

#[test]
fn large_red_circle_rasterizes_red() {
    let Some(text_system) = platform_text_system() else {
        eprintln!("skipped: no platform text system");
        return;
    };
    let Some(bitmap) = single_emoji(text_system.as_ref(), "\u{1F534}") else {
        return;
    };
    assert_cell_sized(&bitmap);
    let red = share(&bitmap.opaque_rgb(), |[r, g, b]| {
        *r >= 180 && *g <= 90 && *b <= 90
    });
    assert!(red > 0.8, "only {red} of the circle is red");
}

#[test]
fn flag_of_japan_is_a_red_disc_on_white() {
    let Some(text_system) = platform_text_system() else {
        eprintln!("skipped: no platform text system");
        return;
    };
    let glyphs = emoji_glyphs(text_system.as_ref(), "\u{1F1EF}\u{1F1F5}");
    if glyphs.len() != 1 {
        eprintln!(
            "skipped: the emoji font shapes the Japan flag to {} glyphs, not one ligature",
            glyphs.len()
        );
        return;
    }
    let (font_id, glyph_id) = glyphs[0];
    let bitmap = rasterize(text_system.as_ref(), font_id, glyph_id);
    assert_cell_sized(&bitmap);
    let opaque = bitmap.opaque_rgb();
    let white = share(&opaque, |pixel| pixel.iter().all(|&channel| channel >= 200));
    let red = share(&opaque, |[r, g, b]| *r >= 160 && *g <= 90 && *b <= 90);
    assert!(white > 0.4, "only {white} of the flag is white");
    assert!(red > 0.05, "only {red} of the flag is red");
    let [r, g, b, a] = bitmap.rgba_at(bitmap.size.width.0 / 2, bitmap.size.height.0 / 2);
    assert!(
        a >= 200 && r >= 160 && g <= 90 && b <= 90,
        "the middle of the flag is not the red disc: {:?}",
        [r, g, b, a]
    );
}
