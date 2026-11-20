use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::{self, Cursor};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageError, ImageFormat, ImageReader, Limits};
use paneflow_config::schema::{SpaceColor, SpaceId};

pub(crate) const AVATAR_PIXELS: u32 = 96;
const MAX_SOURCE_BYTES: u64 = 25 * 1024 * 1024;
const MAX_SOURCE_SIDE: u32 = 12_000;
const MAX_DECODE_BYTES: u64 = 768 * 1024 * 1024;
const PRESCALE_FACTOR: u32 = 4;
const UNREADABLE: &str =
    "Unable to read this image. Choose a PNG, JPEG, WebP, GIF, BMP, or TIFF file.";
#[derive(Debug, Clone, Copy, PartialEq)]
struct Plate {
    ground: [u8; 3],
    ink: [u8; 3],
    angle: f32,
    seed: u32,
}

const DEFAULT_PLATE: Plate = Plate {
    ground: [0x1d, 0x1a, 0x22],
    ink: [0xdd, 0xd5, 0xc8],
    angle: 0.55,
    seed: 11,
};
const COLOR_PLATES: [(SpaceColor, Plate); 6] = [
    (
        SpaceColor::Red,
        Plate {
            ground: [0x22, 0x10, 0x1c],
            ink: [0xe0, 0x7a, 0x52],
            angle: 0.62,
            seed: 42,
        },
    ),
    (
        SpaceColor::Green,
        Plate {
            ground: [0x14, 0x2e, 0x25],
            ink: [0xa8, 0xc9, 0x8f],
            angle: 0.48,
            seed: 73,
        },
    ),
    (
        SpaceColor::Yellow,
        Plate {
            ground: [0x2a, 0x18, 0x0b],
            ink: [0xe6, 0xa8, 0x5a],
            angle: 0.58,
            seed: 104,
        },
    ),
    (
        SpaceColor::Blue,
        Plate {
            ground: [0x12, 0x1c, 0x40],
            ink: [0x8f, 0xa6, 0xf0],
            angle: 0.5,
            seed: 135,
        },
    ),
    (
        SpaceColor::Magenta,
        Plate {
            ground: [0x22, 0x10, 0x30],
            ink: [0xd9, 0xa0, 0xe8],
            angle: 0.66,
            seed: 166,
        },
    ),
    (
        SpaceColor::Cyan,
        Plate {
            ground: [0x0f, 0x26, 0x33],
            ink: [0x9f, 0xd6, 0xe0],
            angle: 0.52,
            seed: 197,
        },
    ),
];
const STREAK_OFFSET: f32 = 0.04;
const STREAK_WIDTH: f32 = 0.17;
const ECHO_OFFSET: f32 = -0.27;
const ECHO_WIDTH: f32 = 0.07;
const ECHO_STRENGTH: f32 = 0.55;
const HALO_OFFSET: f32 = 0.02;
const HALO_WIDTH: f32 = 0.42;
const HALO_STRENGTH: f32 = 0.22;
const SMEAR_SCALE: (f32, f32) = (1.3, 7.0);
const BREAK_SCALE: (f32, f32) = (2.6, 3.0);
const BREAK_SEED_OFFSET: u32 = 51;
const INK_CONTRAST: f32 = 1.15;
const GRAIN: f32 = 0.022;
const FALLBACK_INK_SHARE: f32 = 0.45;
const MAX_NOISE_OCTAVES: u32 = 4;
const PLATE_DENSITY: f32 = 2.0;
const PLATE_MIN_PIXELS: u32 = 16;
const PLATE_MAX_PIXELS: u32 = 256;
const PLATE_SUBSAMPLES: u32 = 2;

pub(crate) fn default_space_image(size: f32) -> Arc<gpui::Image> {
    plate_image(None, size)
}

pub(crate) fn space_color_image(color: SpaceColor, size: f32) -> Arc<gpui::Image> {
    plate_image(Some(color_plate_index(color)), size)
}

pub(crate) fn space_color_fill(color: SpaceColor) -> gpui::Hsla {
    let plate = COLOR_PLATES[color_plate_index(color)].1;
    let [r, g, b] = oklab_to_srgb(mix_lab(
        srgb_to_oklab(plate.ground),
        srgb_to_oklab(plate.ink),
        FALLBACK_INK_SHARE,
    ));
    gpui::rgb(u32::from_be_bytes([0, r, g, b])).into()
}

fn color_plate_index(color: SpaceColor) -> usize {
    COLOR_PLATES
        .iter()
        .position(|(candidate, _)| *candidate == color)
        .unwrap_or_default()
}

fn plate_image(index: Option<usize>, size: f32) -> Arc<gpui::Image> {
    type Cache = Mutex<HashMap<(Option<usize>, u32), Arc<gpui::Image>>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let pixels = plate_pixels(size);
    let cache = CACHE.get_or_init(Cache::default);
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache
        .entry((index, pixels))
        .or_insert_with(|| {
            let plate = index.map_or(DEFAULT_PLATE, |index| COLOR_PLATES[index].1);
            Arc::new(gpui::Image::from_bytes(
                gpui::ImageFormat::Png,
                plate_png(plate, pixels),
            ))
        })
        .clone()
}

fn plate_pixels(size: f32) -> u32 {
    ((size * PLATE_DENSITY).ceil() as u32).clamp(PLATE_MIN_PIXELS, PLATE_MAX_PIXELS)
}

fn plate_png(plate: Plate, pixels: u32) -> Vec<u8> {
    let ground = srgb_to_oklab(plate.ground);
    let ink = srgb_to_oklab(plate.ink);
    let (sin, cos) = plate.angle.sin_cos();
    let octaves = noise_octaves(pixels);
    let size = pixels as f32;
    let subsample = 1.0 / (PLATE_SUBSAMPLES as f32 * size);
    let image = image::RgbImage::from_fn(pixels, pixels, |column, row| {
        let mut light = 0.0;
        for sub_y in 0..PLATE_SUBSAMPLES {
            for sub_x in 0..PLATE_SUBSAMPLES {
                let u = column as f32 / size + (sub_x as f32 + 0.5) * subsample - 0.5;
                let v = row as f32 / size + (sub_y as f32 + 0.5) * subsample - 0.5;
                let along = u * cos + v * sin;
                let across = v * cos - u * sin;
                light += streak_light(plate.seed, along, across, octaves);
            }
        }
        let light = light / (PLATE_SUBSAMPLES * PLATE_SUBSAMPLES) as f32;
        let mut lab = mix_lab(ground, ink, light);
        lab[0] += GRAIN * grain(plate.seed, column, row);
        image::Rgb(oklab_to_srgb(lab))
    });
    let mut png = Vec::new();
    if let Err(error) = image.write_to(&mut Cursor::new(&mut png), ImageFormat::Png) {
        log::warn!("spaces: unable to encode a space plate image: {error}");
    }
    png
}

fn noise_octaves(pixels: u32) -> u32 {
    let nyquist = pixels as f32 / 2.0;
    (0..MAX_NOISE_OCTAVES)
        .take_while(|octave| SMEAR_SCALE.1 * 2f32.powi(*octave as i32) < nyquist)
        .count()
        .max(1) as u32
}

fn streak_light(seed: u32, along: f32, across: f32, octaves: u32) -> f32 {
    let band = |offset: f32, width: f32| (-((across - offset) / width).powi(2)).exp();
    let main = band(STREAK_OFFSET, STREAK_WIDTH);
    let echo = ECHO_STRENGTH * band(ECHO_OFFSET, ECHO_WIDTH);
    let halo = HALO_STRENGTH * band(HALO_OFFSET, HALO_WIDTH);
    let smear = fbm(
        seed,
        along * SMEAR_SCALE.0 + 5.0,
        across * SMEAR_SCALE.1 + 5.0,
        octaves,
    );
    let breaks = fbm(
        seed + BREAK_SEED_OFFSET,
        along * BREAK_SCALE.0 + 2.0,
        across * BREAK_SCALE.1 + 1.0,
        octaves,
    );
    let light = halo + (main + echo) * (0.25 + 1.1 * smear) * (0.55 + 0.7 * breaks);
    light.clamp(0.0, 1.0).powf(INK_CONTRAST)
}

fn mix_lab(from: [f32; 3], to: [f32; 3], share: f32) -> [f32; 3] {
    std::array::from_fn(|channel| from[channel] + (to[channel] - from[channel]) * share)
}

fn fbm(seed: u32, x: f32, y: f32, octaves: u32) -> f32 {
    let (mut total, mut amplitude, mut norm, mut frequency) = (0.0, 1.0, 0.0, 1.0);
    for octave in 0..octaves {
        total += amplitude * value_noise(seed + octave * 17, x * frequency, y * frequency);
        norm += amplitude;
        amplitude *= 0.5;
        frequency *= 2.0;
    }
    total / norm
}

fn value_noise(seed: u32, x: f32, y: f32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
    let (tx, ty) = (smooth(x - x0), smooth(y - y0));
    let (cx, cy) = (x0 as i32, y0 as i32);
    let corner = |dx: i32, dy: i32| lattice(seed, cx + dx, cy + dy);
    let top = corner(0, 0) + (corner(1, 0) - corner(0, 0)) * tx;
    let bottom = corner(0, 1) + (corner(1, 1) - corner(0, 1)) * tx;
    top + (bottom - top) * ty
}

fn lattice(seed: u32, x: i32, y: i32) -> f32 {
    (mix_bits(seed, x as u32, y as u32) >> 8) as f32 / (1u32 << 24) as f32
}

fn grain(seed: u32, column: u32, row: u32) -> f32 {
    let bits = mix_bits(seed ^ 0x9e37_79b9, column, row);
    let first = (bits & 0xffff) as f32 / 65_535.0;
    let second = (bits >> 16) as f32 / 65_535.0;
    first + second - 1.0
}

fn mix_bits(seed: u32, x: u32, y: u32) -> u32 {
    let mut hash =
        seed.wrapping_mul(0x27d4_eb2d) ^ x.wrapping_mul(0x85eb_ca6b) ^ y.wrapping_mul(0xc2b2_ae35);
    hash ^= hash >> 15;
    hash = hash.wrapping_mul(0x2c1b_3c6d);
    hash ^= hash >> 12;
    hash = hash.wrapping_mul(0x297a_2d39);
    hash ^ (hash >> 15)
}

fn srgb_to_oklab(rgb: [u8; 3]) -> [f32; 3] {
    let [r, g, b] = rgb.map(|channel| {
        let channel = f32::from(channel) / 255.0;
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    });
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

fn oklab_to_srgb([lightness, a, b]: [f32; 3]) -> [u8; 3] {
    let l = (lightness + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m = (lightness - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s = (lightness - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
    .map(|channel| {
        let channel = channel.clamp(0.0, 1.0);
        let encoded = if channel <= 0.003_130_8 {
            channel * 12.92
        } else {
            1.055 * channel.powf(1.0 / 2.4) - 0.055
        };
        (encoded * 255.0).round() as u8
    })
}

pub(crate) fn avatar_path(file_name: &str) -> Option<PathBuf> {
    paneflow_home::space_avatars_dir().map(|dir| dir.join(file_name))
}

pub(crate) fn import_avatar(source: &Path, space: &SpaceId) -> Result<String, String> {
    let bytes =
        paneflow_home::read_regular_capped(source, MAX_SOURCE_BYTES).map_err(
            |error| match error.kind() {
                io::ErrorKind::FileTooLarge => {
                    "This file is larger than 25 MB. Choose a smaller image.".to_string()
                }
                _ => format!("Unable to open this file: {error}"),
            },
        )?;
    let png = avatar_png(&bytes)?;
    let file_name = avatar_file_name(space, &png);
    let dir = paneflow_home::space_avatars_dir()
        .ok_or_else(|| "Unable to save the image: no Paneflow home directory".to_string())?;
    paneflow_home::create_private_dir_all(&dir)
        .and_then(|()| paneflow_home::write_atomically(&dir.join(&file_name), &png))
        .map_err(|error| format!("Unable to save the image: {error}"))?;
    Ok(file_name)
}

pub(crate) fn discard_avatar(file_name: &str) {
    let Some(path) = avatar_path(file_name) else {
        return;
    };
    if let Err(error) = std::fs::remove_file(&path)
        && error.kind() != io::ErrorKind::NotFound
    {
        log::warn!("spaces: unable to remove {}: {error}", path.display());
    }
}

fn avatar_png(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let square = centered_square(decode_oriented(bytes)?);
    let side = square.width();
    let prescaled = if side >= AVATAR_PIXELS * PRESCALE_FACTOR {
        square.thumbnail_exact(AVATAR_PIXELS * 2, AVATAR_PIXELS * 2)
    } else {
        square
    };
    let avatar = prescaled
        .resize_exact(AVATAR_PIXELS, AVATAR_PIXELS, FilterType::Lanczos3)
        .to_rgba8();
    let mut png = Vec::new();
    avatar
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .map_err(|error| format!("Unable to save the image: {error}"))?;
    Ok(png)
}

fn decode_oriented(bytes: &[u8]) -> Result<DynamicImage, String> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| UNREADABLE.to_string())?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_SIDE);
    limits.max_image_height = Some(MAX_SOURCE_SIDE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(describe)?;
    let orientation = decoder.orientation().ok();
    let mut image = DynamicImage::from_decoder(decoder).map_err(describe)?;
    if let Some(orientation) = orientation {
        image.apply_orientation(orientation);
    }
    Ok(image)
}

fn centered_square(image: DynamicImage) -> DynamicImage {
    let (width, height) = (image.width(), image.height());
    let side = width.min(height);
    if width == height {
        return image;
    }
    image.crop_imm((width - side) / 2, (height - side) / 2, side, side)
}

fn describe(error: ImageError) -> String {
    match error {
        ImageError::Limits(_) => format!(
            "This image is too large. Choose one under {MAX_SOURCE_SIDE} pixels on each side."
        ),
        _ => UNREADABLE.to_string(),
    }
}

fn avatar_file_name(space: &SpaceId, png: &[u8]) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    png.hash(&mut hasher);
    format!("{space}-{:016x}.png", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
        let image = image::RgbaImage::from_fn(width, height, |x, _| {
            if x < width / 2 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 255])
            }
        });
        let mut bytes = Vec::new();
        DynamicImage::ImageRgba8(image)
            .write_to(&mut Cursor::new(&mut bytes), format)
            .unwrap();
        bytes
    }

    #[test]
    fn the_default_space_plate_is_a_stable_neutral_duotone() {
        let pixels = plate_pixels(56.0);
        let png = plate_png(DEFAULT_PLATE, pixels);
        assert_eq!(png, plate_png(DEFAULT_PLATE, pixels));
        let plate = image::load_from_memory_with_format(&png, ImageFormat::Png)
            .unwrap()
            .to_rgb8();
        assert_eq!(plate.dimensions(), (112, 112));
        let mut darkest = u8::MAX;
        let mut brightest = 0;
        for pixel in plate.pixels() {
            let [r, g, b] = pixel.0;
            let (low, high) = (r.min(g).min(b), r.max(g).max(b));
            assert!(high - low <= 24, "the default plate stays neutral");
            darkest = darkest.min(high);
            brightest = brightest.max(high);
        }
        assert!(darkest < 60, "the ground stays dark");
        assert!(brightest > 150, "the streak lights the plate");
    }

    #[test]
    fn a_plate_is_drawn_at_twice_its_size_and_drops_the_octaves_it_cannot_resolve() {
        assert_eq!(plate_pixels(16.0), 32);
        assert_eq!(plate_pixels(56.0), 112);
        assert_eq!(plate_pixels(4.0), PLATE_MIN_PIXELS);
        assert_eq!(plate_pixels(400.0), PLATE_MAX_PIXELS);
        assert_eq!(noise_octaves(32), 2);
        assert_eq!(noise_octaves(112), 3);
        assert_eq!(noise_octaves(PLATE_MIN_PIXELS), 1);
        assert_eq!(noise_octaves(PLATE_MAX_PIXELS), MAX_NOISE_OCTAVES);
    }

    #[test]
    fn every_space_color_has_its_own_plate_whose_ink_carries_its_hue() {
        let colors = crate::settings::tabs::spaces::SPACE_COLORS;
        assert_eq!(colors.len(), COLOR_PLATES.len());
        for color in colors {
            let (owner, plate) = COLOR_PLATES[color_plate_index(color)];
            assert_eq!(owner, color);
            assert_ne!(plate, DEFAULT_PLATE);
        }
        let ink = |color| COLOR_PLATES[color_plate_index(color)].1.ink;
        let [r, g, b] = ink(SpaceColor::Red);
        assert!(r > g && r > b);
        let [r, g, b] = ink(SpaceColor::Green);
        assert!(g > r && g > b);
        let [r, g, b] = ink(SpaceColor::Blue);
        assert!(b > r && b > g);
    }

    #[test]
    fn oklab_round_trips_each_plate_color() {
        for rgb in COLOR_PLATES
            .iter()
            .flat_map(|(_, plate)| [plate.ground, plate.ink])
            .chain([DEFAULT_PLATE.ground, DEFAULT_PLATE.ink])
        {
            assert_eq!(oklab_to_srgb(srgb_to_oklab(rgb)), rgb);
        }
    }

    #[test]
    fn an_avatar_is_a_centered_square_png_at_the_avatar_size() {
        for (width, height) in [(1200, 800), (60, 90), (512, 512)] {
            let png = avatar_png(&encoded(width, height, ImageFormat::Png)).unwrap();
            let avatar = image::load_from_memory_with_format(&png, ImageFormat::Png)
                .unwrap()
                .to_rgba8();
            assert_eq!(avatar.dimensions(), (AVATAR_PIXELS, AVATAR_PIXELS));
            assert_eq!(avatar.get_pixel(4, 48).0, [255, 0, 0, 255]);
            assert_eq!(avatar.get_pixel(91, 48).0, [0, 0, 255, 255]);
        }
    }

    #[test]
    fn an_avatar_reads_every_offered_format() {
        for format in [
            ImageFormat::Jpeg,
            ImageFormat::WebP,
            ImageFormat::Gif,
            ImageFormat::Bmp,
            ImageFormat::Tiff,
        ] {
            assert!(
                avatar_png(&encoded(40, 40, format)).is_ok(),
                "{format:?} is offered in the error message"
            );
        }
    }

    #[test]
    fn an_unreadable_file_names_the_formats_to_choose() {
        assert_eq!(avatar_png(b"not an image"), Err(UNREADABLE.to_string()));
    }

    #[test]
    fn an_avatar_file_name_is_a_valid_config_value_that_changes_with_the_image() {
        let space = SpaceId::new();
        let red = avatar_file_name(&space, b"red");
        assert!(paneflow_config::schema::is_valid_space_avatar(&red));
        assert_eq!(red, avatar_file_name(&space, b"red"));
        assert_ne!(red, avatar_file_name(&space, b"blue"));
    }
}
