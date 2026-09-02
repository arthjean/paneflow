use std::collections::HashMap;
use std::sync::{Arc, Once};

use gpui::RenderImage;
use paneflow_terminal_ghostty as ghostty;

const MAX_IMAGE_STORAGE_BYTES: u64 = 32 * 1024 * 1024;

const MAX_COMMAND_BYTES: usize = 8 * 1024 * 1024;

const MAX_IMAGE_PIXELS: u64 = 8192 * 8192;

#[derive(Clone)]
pub struct KittyPlacement {
    pub image: Arc<RenderImage>,
    pub col: i32,
    pub row: i32,
    pub width: u32,
    pub height: u32,
    pub source_x: u32,
    pub source_y: u32,
    pub source_width: u32,
    pub source_height: u32,
    pub z: i32,
}

impl std::fmt::Debug for KittyPlacement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KittyPlacement")
            .field("col", &self.col)
            .field("row", &self.row)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("z", &self.z)
            .finish_non_exhaustive()
    }
}

impl PartialEq for KittyPlacement {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.image, &other.image)
            && (self.col, self.row, self.width, self.height, self.z)
                == (other.col, other.row, other.width, other.height, other.z)
            && (
                self.source_x,
                self.source_y,
                self.source_width,
                self.source_height,
            ) == (
                other.source_x,
                other.source_y,
                other.source_width,
                other.source_height,
            )
    }
}

impl Eq for KittyPlacement {}

pub(super) fn install_png_decoder() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        if let Err(error) = ghostty::set_png_decoder(Some(decode_png)) {
            log::warn!(
                target: "paneflow::terminal::kitty",
                "PNG decoder could not be installed, Kitty PNG images will not render: {error}"
            );
        }
    });
}

fn decode_png(bytes: &[u8]) -> Option<ghostty::DecodedImage> {
    let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?;
    let width = decoded.width();
    let height = decoded.height();
    if u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS {
        log::debug!(
            target: "paneflow::terminal::kitty",
            "refused a {width}x{height} PNG: past the {MAX_IMAGE_PIXELS}-pixel cap"
        );
        return None;
    }
    Some(ghostty::DecodedImage {
        width,
        height,
        rgba: decoded.into_rgba8().into_raw(),
    })
}

fn to_bgra(info: &ghostty::ImageInfo, pixels: &[u8]) -> Option<Vec<u8>> {
    let stride = info.format.bytes_per_pixel()?;
    let count = usize::try_from(u64::from(info.width) * u64::from(info.height)).ok()?;
    if pixels.len() < count.checked_mul(stride)? {
        return None;
    }
    let mut bgra = Vec::with_capacity(count * 4);
    for pixel in pixels.chunks_exact(stride).take(count) {
        let (r, g, b, a) = match info.format {
            ghostty::ImageFormat::Rgb => (pixel[0], pixel[1], pixel[2], 0xff),
            ghostty::ImageFormat::Rgba => (pixel[0], pixel[1], pixel[2], pixel[3]),
            ghostty::ImageFormat::Gray => (pixel[0], pixel[0], pixel[0], 0xff),
            ghostty::ImageFormat::GrayAlpha => (pixel[0], pixel[0], pixel[0], pixel[1]),
            ghostty::ImageFormat::Png => return None,
        };
        bgra.extend_from_slice(&[b, g, r, a]);
    }
    Some(bgra)
}

#[derive(Default)]
pub(super) struct KittyImages {
    textures: HashMap<u32, (u64, Arc<RenderImage>)>,
    storage_generation: u64,
}

impl KittyImages {
    pub(super) fn collect(&mut self, terminal: &ghostty::DisplayTerminal) -> Vec<KittyPlacement> {
        match self.try_collect(terminal) {
            Ok(placements) => placements,
            Err(error) => {
                log::debug!(
                    target: "paneflow::terminal::kitty",
                    "Kitty graphics could not be read for this frame: {error}"
                );
                Vec::new()
            }
        }
    }

    fn try_collect(
        &mut self,
        terminal: &ghostty::DisplayTerminal,
    ) -> ghostty::Result<Vec<KittyPlacement>> {
        let Some(graphics) = terminal.kitty_graphics()? else {
            self.clear();
            return Ok(Vec::new());
        };
        let generation = graphics.generation()?;
        if generation == 0 {
            self.clear();
            return Ok(Vec::new());
        }
        let stored_changed = generation != self.storage_generation;
        self.storage_generation = generation;

        let mut placements = Vec::new();
        let mut live = Vec::new();
        let mut cursor = graphics.placements(ghostty::PlacementLayer::All)?;
        while cursor.advance() {
            let image_id = cursor.image_id()?;
            let Some(image) = graphics.image(image_id) else {
                continue;
            };
            let info = image.info()?;
            live.push(image_id);
            let texture = match self.texture(&info, &image)? {
                Some(texture) => texture,
                None => continue,
            };
            let render = cursor.render_info(&image)?;
            let Some((col, row)) = render.viewport else {
                continue;
            };
            let placement = cursor.read()?;
            placements.push(KittyPlacement {
                image: texture,
                col,
                row,
                width: render.pixel_width,
                height: render.pixel_height,
                source_x: render.source.x,
                source_y: render.source.y,
                source_width: render.source.width,
                source_height: render.source.height,
                z: placement.z,
            });
        }
        if stored_changed {
            self.textures.retain(|id, _| live.contains(id));
        }
        placements.sort_by_key(|placement| placement.z);
        Ok(placements)
    }

    fn texture(
        &mut self,
        info: &ghostty::ImageInfo,
        image: &ghostty::KittyImage<'_>,
    ) -> ghostty::Result<Option<Arc<RenderImage>>> {
        if let Some((generation, texture)) = self.textures.get(&info.id)
            && *generation == info.generation
        {
            return Ok(Some(texture.clone()));
        }
        let Some(pixels) = image.pixels()? else {
            return Ok(None);
        };
        let Some(texture) = build_texture(info, pixels) else {
            return Ok(None);
        };
        self.textures
            .insert(info.id, (info.generation, texture.clone()));
        Ok(Some(texture))
    }

    fn clear(&mut self) {
        self.storage_generation = 0;
        self.textures.clear();
    }
}

fn build_texture(info: &ghostty::ImageInfo, pixels: &[u8]) -> Option<Arc<RenderImage>> {
    if info.width == 0 || info.height == 0 {
        return None;
    }
    if u64::from(info.width) * u64::from(info.height) > MAX_IMAGE_PIXELS {
        log::debug!(
            target: "paneflow::terminal::kitty",
            "skipped a {}x{} image: past the {MAX_IMAGE_PIXELS}-pixel cap",
            info.width,
            info.height
        );
        return None;
    }
    let bgra = to_bgra(info, pixels)?;
    let buffer = image::RgbaImage::from_raw(info.width, info.height, bgra)?;
    Some(Arc::new(RenderImage::new([image::Frame::new(buffer)])))
}

pub(super) fn enable(terminal: &mut ghostty::DisplayTerminal) {
    install_png_decoder();
    if let Err(error) = terminal.enable_kitty_graphics(MAX_IMAGE_STORAGE_BYTES, MAX_COMMAND_BYTES) {
        log::warn!(
            target: "paneflow::terminal::kitty",
            "Kitty graphics could not be enabled: {error}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn red_image_command() -> Vec<u8> {
        let payload = "/wAA".repeat(16 * 32);
        format!("\x1b_Ga=T,f=24,s=16,v=32,q=2;{payload}\x1b\\").into_bytes()
    }

    fn terminal() -> ghostty::DisplayTerminal {
        let size = ghostty::WindowSize::new(40, 10, 8, 16).expect("valid size");
        ghostty::DisplayTerminal::new(size, 100, ghostty::TerminalAppearance::default())
            .expect("terminal must initialize")
    }

    #[test]
    fn a_transmitted_image_becomes_a_placement_with_an_uploaded_texture() {
        let mut terminal = terminal();
        let mut images = KittyImages::default();

        terminal
            .feed(&red_image_command())
            .expect("image command must parse");
        assert!(images.collect(&terminal).is_empty());

        enable(&mut terminal);
        terminal
            .feed(&red_image_command())
            .expect("image command must parse");
        let placements = images.collect(&terminal);
        assert_eq!(placements.len(), 1, "got {placements:?}");

        let placement = &placements[0];
        assert_eq!((placement.width, placement.height), (16, 32));
        assert_eq!((placement.col, placement.row), (0, 0));
        assert_eq!(
            (
                placement.source_x,
                placement.source_y,
                placement.source_width,
                placement.source_height
            ),
            (0, 0, 16, 32)
        );
        let size = placement.image.size(0);
        assert_eq!((i32::from(size.width), i32::from(size.height)), (16, 32));
        let bytes = placement.image.as_bytes(0).expect("frame 0");
        assert_eq!(&bytes[..4], &[0x00, 0x00, 0xff, 0xff]);
    }

    #[test]
    fn the_texture_is_reused_across_frames_and_replaced_on_retransmission() {
        let mut terminal = terminal();
        enable(&mut terminal);
        let mut images = KittyImages::default();

        terminal
            .feed(&red_image_command())
            .expect("image command must parse");
        let first = images.collect(&terminal);
        let second = images.collect(&terminal);
        assert!(
            Arc::ptr_eq(&first[0].image, &second[0].image),
            "an unchanged image must not be uploaded twice"
        );

        let blue = format!(
            "\x1b_Ga=T,f=24,s=16,v=32,q=2;{}\x1b\\",
            "AAD/".repeat(16 * 32)
        );
        terminal
            .feed(blue.as_bytes())
            .expect("retransmission must parse");
        let third = images.collect(&terminal);
        let replaced = third
            .iter()
            .find(|placement| !Arc::ptr_eq(&placement.image, &first[0].image))
            .expect("the retransmitted image must be re-uploaded");
        let bytes = replaced.image.as_bytes(0).expect("frame 0");
        assert_eq!(&bytes[..4], &[0xff, 0x00, 0x00, 0xff]);
    }

    #[test]
    fn a_placement_scrolled_out_of_the_viewport_is_dropped() {
        let mut terminal = terminal();
        enable(&mut terminal);
        let mut images = KittyImages::default();
        terminal
            .feed(&red_image_command())
            .expect("image command must parse");
        assert_eq!(images.collect(&terminal).len(), 1);

        for _ in 0..60 {
            terminal.feed(b"\r\nfiller").expect("filler must parse");
        }
        assert!(
            images.collect(&terminal).is_empty(),
            "an off-screen placement has nothing to draw"
        );
    }

    fn info(format: ghostty::ImageFormat, width: u32, height: u32) -> ghostty::ImageInfo {
        ghostty::ImageInfo {
            id: 1,
            number: 0,
            width,
            height,
            format,
            compression: ghostty::ImageCompression::None,
            len: 0,
            generation: 1,
        }
    }

    #[test]
    fn every_stored_format_widens_to_bgra() {
        assert_eq!(
            to_bgra(&info(ghostty::ImageFormat::Rgb, 1, 1), &[1, 2, 3]),
            Some(vec![3, 2, 1, 0xff])
        );
        assert_eq!(
            to_bgra(&info(ghostty::ImageFormat::Rgba, 1, 1), &[1, 2, 3, 4]),
            Some(vec![3, 2, 1, 4])
        );
        assert_eq!(
            to_bgra(&info(ghostty::ImageFormat::Gray, 1, 1), &[7]),
            Some(vec![7, 7, 7, 0xff])
        );
        assert_eq!(
            to_bgra(&info(ghostty::ImageFormat::GrayAlpha, 1, 1), &[7, 9]),
            Some(vec![7, 7, 7, 9])
        );
        assert_eq!(
            to_bgra(&info(ghostty::ImageFormat::Png, 1, 1), &[1, 2, 3, 4]),
            None
        );
    }

    #[test]
    fn a_payload_shorter_than_its_dimensions_is_refused() {
        assert_eq!(
            to_bgra(&info(ghostty::ImageFormat::Rgba, 2, 1), &[1, 2, 3, 4]),
            None
        );
    }

    #[test]
    fn an_oversized_image_is_skipped_rather_than_allocated() {
        assert!(build_texture(&info(ghostty::ImageFormat::Rgba, 65_536, 65_536), &[]).is_none());
        assert!(build_texture(&info(ghostty::ImageFormat::Rgba, 0, 4), &[]).is_none());
    }

    #[test]
    fn a_decoded_png_comes_back_as_tightly_packed_rgba() {
        let png = [
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08,
            0xd7, 0x63, 0xf8, 0xcf, 0xc0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xdd, 0x8d,
            0xb0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];
        let decoded = decode_png(&png).expect("a valid PNG must decode");
        assert_eq!((decoded.width, decoded.height), (1, 1));
        assert_eq!(decoded.rgba, vec![0xff, 0x00, 0x00, 0xff]);

        assert!(decode_png(b"not a png").is_none());
    }

    #[test]
    fn a_texture_is_rebuilt_only_when_the_generation_moves() {
        let mut images = KittyImages::default();
        let first = build_texture(&info(ghostty::ImageFormat::Rgba, 1, 1), &[1, 2, 3, 4])
            .expect("texture must build");
        images.textures.insert(7, (3, first.clone()));

        let cached = images.textures.get(&7).expect("cached entry");
        assert_eq!(cached.0, 3);
        assert!(Arc::ptr_eq(&cached.1, &first));

        let mut retransmitted = info(ghostty::ImageFormat::Rgba, 1, 1);
        retransmitted.id = 7;
        retransmitted.generation = 4;
        assert_ne!(images.textures[&7].0, retransmitted.generation);
    }
}
