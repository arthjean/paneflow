#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use paneflow_colrv1::{render_colr_v1_glyph, ColorGlyphImage};
use skrifa::color::ColorGlyphFormat;
use skrifa::instance::{LocationRef, Size};
use skrifa::{FontRef, GlyphId, MetadataProvider};

const NOTO_COLRV1: &str = "/usr/share/fonts/google-noto-color-emoji-fonts/Noto-COLRv1.ttf";

fn noto_colrv1() -> Option<Vec<u8>> {
    match std::fs::read(NOTO_COLRV1) {
        Ok(data) => Some(data),
        Err(error) => {
            eprintln!(
                "skipped: {NOTO_COLRV1} is not readable ({error}); install the Noto COLRv1 \
                 emoji font (Fedora: google-noto-color-emoji-fonts) to run this test"
            );
            None
        }
    }
}

fn glyph_id(data: &[u8], character: char) -> u32 {
    FontRef::new(data)
        .expect("the Noto COLRv1 font parses")
        .charmap()
        .map(character)
        .expect("the Noto COLRv1 font maps the emoji")
        .to_u32()
}

fn render(data: &[u8], character: char, pixel_size: f32) -> ColorGlyphImage {
    render_colr_v1_glyph(data, 0, glyph_id(data, character), pixel_size, (0.0, 0.0))
        .expect("the emoji has a COLRv1 paint graph")
}

fn opaque_pixels(image: &ColorGlyphImage) -> Vec<[u8; 3]> {
    image
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|[_, _, _, alpha]| *alpha >= 200)
        .map(|[red, green, blue, _]| [*red, *green, *blue])
        .collect()
}

fn share(pixels: &[[u8; 3]], predicate: impl Fn(&[u8; 3]) -> bool) -> f32 {
    pixels.iter().filter(|pixel| predicate(pixel)).count() as f32 / pixels.len().max(1) as f32
}

fn assert_consistent(image: &ColorGlyphImage) {
    let placement = image.placement;
    assert!(placement.width > 0 && placement.height > 0, "{placement:?}");
    assert_eq!(
        image.rgba.len(),
        placement.width as usize * placement.height as usize * 4,
        "{placement:?}"
    );
}

#[test]
fn thumbs_up_is_painted_mostly_yellow() {
    let Some(data) = noto_colrv1() else {
        return;
    };
    let image = render(&data, '\u{1F44D}', 64.0);
    assert_consistent(&image);
    let opaque = opaque_pixels(&image);
    let coverage = opaque.len() as f32 / (image.rgba.len() / 4) as f32;
    assert!(coverage > 0.3, "only {coverage} of the bitmap is opaque");
    let yellow = share(&opaque, |[r, g, b]| *r >= 200 && *g >= 140 && *b <= 100);
    assert!(
        yellow > 0.6,
        "only {yellow} of the opaque pixels are yellow"
    );
}

#[test]
fn large_red_circle_is_painted_mostly_red() {
    let Some(data) = noto_colrv1() else {
        return;
    };
    let image = render(&data, '\u{1F534}', 64.0);
    assert_consistent(&image);
    let opaque = opaque_pixels(&image);
    let red = share(&opaque, |[r, g, b]| *r >= 180 && *g <= 90 && *b <= 90);
    assert!(red > 0.8, "only {red} of the opaque pixels are red");
    let center = image.placement.width as usize / 2
        + image.placement.width as usize * (image.placement.height as usize / 2);
    assert_eq!(image.rgba[center * 4 + 3], 255);
}

#[test]
fn the_placement_is_the_clip_box_rounded_outward_after_the_offset() {
    let Some(data) = noto_colrv1() else {
        return;
    };
    let font = FontRef::new(&data).expect("the Noto COLRv1 font parses");
    let id = glyph_id(&data, '\u{1F600}');
    let clip = font
        .color_glyphs()
        .get_with_format(GlyphId::new(id), ColorGlyphFormat::ColrV1)
        .and_then(|glyph| glyph.bounding_box(LocationRef::default(), Size::new(40.0)))
        .expect("Noto COLRv1 glyphs carry a clip box");
    for offset in [(0.0, 0.0), (0.25, 0.5), (0.75, -0.25)] {
        let image = render_colr_v1_glyph(&data, 0, id, 40.0, offset)
            .expect("the emoji has a COLRv1 paint graph");
        assert_consistent(&image);
        let placement = image.placement;
        assert_eq!(placement.left, (clip.x_min + offset.0).floor() as i32);
        assert_eq!(placement.top, (clip.y_max + offset.1).ceil() as i32);
        assert_eq!(
            placement.left + placement.width as i32,
            (clip.x_max + offset.0).ceil() as i32
        );
        assert_eq!(
            placement.top - placement.height as i32,
            (clip.y_min + offset.1).floor() as i32
        );
    }
}

#[test]
fn the_bitmap_scales_with_the_pixel_size() {
    let Some(data) = noto_colrv1() else {
        return;
    };
    let small = render(&data, '\u{1F600}', 16.0).placement;
    let large = render(&data, '\u{1F600}', 64.0).placement;
    let ratio = large.width as f32 / small.width as f32;
    assert!((3.5..=4.5).contains(&ratio), "{small:?} {large:?}");
}

#[test]
fn a_glyph_without_a_colrv1_paint_graph_is_left_to_other_sources() {
    let Some(data) = noto_colrv1() else {
        return;
    };
    assert_eq!(render_colr_v1_glyph(&data, 0, 0, 32.0, (0.0, 0.0)), None);
    assert_eq!(
        render_colr_v1_glyph(&data, 0, glyph_id(&data, '\u{1F600}'), 0.0, (0.0, 0.0)),
        None
    );
}
