use std::f32::consts::TAU;
use std::hash::{Hash, Hasher};
use std::io::{self, Cursor};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

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
type MeshColors = [[u8; 3]; 5];
const MESH_ANCHORS: [(f32, f32); 5] = [
    (0.12, 0.18),
    (0.88, 0.12),
    (0.18, 0.88),
    (0.86, 0.82),
    (0.62, 0.40),
];
const DEFAULT_MESH: MeshColors = [
    [0x5b, 0x8c, 0xff],
    [0xf0, 0xa6, 0xca],
    [0x5e, 0xea, 0xd4],
    [0xa7, 0x8b, 0xfa],
    [0xfd, 0xe6, 0x8a],
];
const COLOR_MESHES: [(SpaceColor, MeshColors); 6] = [
    (
        SpaceColor::Red,
        [
            [0xff, 0x5f, 0x6d],
            [0xff, 0x9a, 0x62],
            [0xe2, 0x3b, 0x6b],
            [0xff, 0x7a, 0xa2],
            [0xff, 0xc9, 0xa3],
        ],
    ),
    (
        SpaceColor::Green,
        [
            [0x2f, 0xcf, 0x8f],
            [0xa3, 0xe0, 0x4a],
            [0x0f, 0xa4, 0x7a],
            [0x5f, 0xe0, 0xc4],
            [0xd4, 0xf5, 0xa0],
        ],
    ),
    (
        SpaceColor::Yellow,
        [
            [0xf8, 0xb6, 0x2c],
            [0xff, 0xe0, 0x5a],
            [0xf0, 0x8c, 0x1f],
            [0xff, 0xb6, 0x48],
            [0xff, 0xf1, 0xb8],
        ],
    ),
    (
        SpaceColor::Blue,
        [
            [0x3d, 0x7c, 0xf5],
            [0x64, 0xb0, 0xff],
            [0x5b, 0x5f, 0xf0],
            [0x3c, 0xc2, 0xf5],
            [0xbc, 0xd8, 0xff],
        ],
    ),
    (
        SpaceColor::Magenta,
        [
            [0xb8, 0x6b, 0xff],
            [0xf4, 0x6f, 0xbd],
            [0x8f, 0x4d, 0xf5],
            [0xe5, 0x7c, 0xf2],
            [0xf8, 0xc8, 0xec],
        ],
    ),
    (
        SpaceColor::Cyan,
        [
            [0x1e, 0xc8, 0xe6],
            [0x6f, 0xe6, 0xf7],
            [0x13, 0xa8, 0xa0],
            [0x3a, 0xa8, 0xf0],
            [0xc8, 0xf7, 0xfb],
        ],
    ),
];
const MESH_FALLOFF: f32 = 2.4;
const MESH_WARP: f32 = 0.07;

pub(crate) fn default_space_image() -> Arc<gpui::Image> {
    static IMAGE: OnceLock<Arc<gpui::Image>> = OnceLock::new();
    mesh_image(&IMAGE, DEFAULT_MESH)
}

pub(crate) fn space_color_image(color: SpaceColor) -> Arc<gpui::Image> {
    static IMAGES: [OnceLock<Arc<gpui::Image>>; COLOR_MESHES.len()] =
        [const { OnceLock::new() }; COLOR_MESHES.len()];
    let index = color_mesh_index(color);
    mesh_image(&IMAGES[index], COLOR_MESHES[index].1)
}

pub(crate) fn space_color_fill(color: SpaceColor) -> gpui::Hsla {
    let [r, g, b] = COLOR_MESHES[color_mesh_index(color)].1[0];
    gpui::rgb(u32::from_be_bytes([0, r, g, b])).into()
}

fn color_mesh_index(color: SpaceColor) -> usize {
    COLOR_MESHES
        .iter()
        .position(|(candidate, _)| *candidate == color)
        .unwrap_or_default()
}

fn mesh_image(slot: &OnceLock<Arc<gpui::Image>>, colors: MeshColors) -> Arc<gpui::Image> {
    slot.get_or_init(|| {
        Arc::new(gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            mesh_gradient_png(colors),
        ))
    })
    .clone()
}

fn mesh_gradient_png(colors: MeshColors) -> Vec<u8> {
    let anchors: [(f32, f32, [f32; 3]); 5] = std::array::from_fn(|index| {
        let (x, y) = MESH_ANCHORS[index];
        (x, y, srgb_to_oklab(colors[index]))
    });
    let size = AVATAR_PIXELS as f32;
    let mesh = image::RgbImage::from_fn(AVATAR_PIXELS, AVATAR_PIXELS, |column, row| {
        let (u, v) = ((column as f32 + 0.5) / size, (row as f32 + 0.5) / size);
        let (u, v) = (
            u + MESH_WARP * (TAU * (v * 1.2 + 0.15)).sin(),
            v + MESH_WARP * (TAU * (u * 0.9 + 0.6)).sin(),
        );
        let mut blend = [0.0_f32; 3];
        let mut total = 0.0;
        for (x, y, lab) in anchors {
            let distance = (u - x).powi(2) + (v - y).powi(2);
            let weight = 1.0 / (distance.powf(MESH_FALLOFF / 2.0) + 1e-4);
            total += weight;
            for (channel, value) in blend.iter_mut().zip(lab) {
                *channel += weight * value;
            }
        }
        image::Rgb(oklab_to_srgb(blend.map(|channel| channel / total)))
    });
    let mut png = Vec::new();
    if let Err(error) = mesh.write_to(&mut Cursor::new(&mut png), ImageFormat::Png) {
        log::warn!("spaces: unable to encode a space mesh image: {error}");
    }
    png
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
    fn the_default_space_image_is_a_stable_mesh_from_its_anchor_colors() {
        let png = mesh_gradient_png(DEFAULT_MESH);
        assert_eq!(png, mesh_gradient_png(DEFAULT_MESH));
        let mesh = image::load_from_memory_with_format(&png, ImageFormat::Png)
            .unwrap()
            .to_rgb8();
        assert_eq!(mesh.dimensions(), (AVATAR_PIXELS, AVATAR_PIXELS));
        let [r, g, b] = mesh.get_pixel(0, 0).0;
        assert!(b > r && b > g, "the top left leans on the blue anchor");
        let [r, g, b] = mesh.get_pixel(0, AVATAR_PIXELS - 1).0;
        assert!(g > r && g > b, "the bottom left leans on the mint anchor");
    }

    #[test]
    fn every_space_color_has_its_own_mesh_led_by_its_hue() {
        let colors = crate::settings::tabs::spaces::SPACE_COLORS;
        assert_eq!(colors.len(), COLOR_MESHES.len());
        for color in colors {
            let (owner, mesh) = COLOR_MESHES[color_mesh_index(color)];
            assert_eq!(owner, color);
            assert_ne!(mesh, DEFAULT_MESH);
        }
        let [r, g, b] = COLOR_MESHES[color_mesh_index(SpaceColor::Red)].1[0];
        assert!(r > g && r > b);
        let [r, g, b] = COLOR_MESHES[color_mesh_index(SpaceColor::Blue)].1[0];
        assert!(b > r && b > g);
    }

    #[test]
    fn oklab_round_trips_each_anchor_color() {
        for rgb in COLOR_MESHES
            .iter()
            .flat_map(|(_, mesh)| *mesh)
            .chain(DEFAULT_MESH)
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
