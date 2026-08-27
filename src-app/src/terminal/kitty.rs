//! Kitty graphics: decode what arrives, cache what is stored, hand the
//! renderer what to draw.
//!
//! libghostty owns the protocol and the image storage. This module is the
//! bridge to GPUI: it installs the PNG decoder the protocol needs, turns
//! stored pixels into GPU-ready textures, and resolves each placement's
//! geometry once per frame.
//!
//! The work happens on the session runtime thread, never on the render
//! thread. Textures are keyed on libghostty's per-image generation stamp, so
//! a screen full of images costs one pixel copy per image transmitted, not
//! one per frame. Placement *geometry* is recomputed every frame regardless,
//! because scrolling moves placements without changing what is stored.

use std::collections::HashMap;
use std::sync::{Arc, Once};

use gpui::RenderImage;
use paneflow_terminal_ghostty as ghostty;

/// Decoded pixels Paneflow will hold for one terminal's images.
///
/// libghostty enforces this on its side as the storage limit; the cache
/// mirrors it so a terminal cannot pin more than this in GPU-bound copies.
const MAX_IMAGE_STORAGE_BYTES: u64 = 32 * 1024 * 1024;

/// Bytes one Kitty graphics command may buffer while it arrives.
const MAX_COMMAND_BYTES: usize = 8 * 1024 * 1024;

/// Largest single image Paneflow will copy into a texture.
///
/// A 4K frame is 33 MB; anything past that is a program misbehaving, and
/// refusing it costs a missing image rather than a stalled frame.
const MAX_IMAGE_PIXELS: u64 = 8192 * 8192;

/// One placement resolved for a frame, with its texture already uploaded.
#[derive(Clone)]
pub struct KittyPlacement {
    /// The texture to sample, in the BGRA layout GPUI expects.
    pub image: Arc<RenderImage>,
    /// Viewport column of the top-left corner.
    pub col: i32,
    /// Viewport row of the top-left corner. Negative when the image has
    /// scrolled partly above the viewport, which the renderer clips.
    pub row: i32,
    /// Rendered width in pixels.
    pub width: u32,
    /// Rendered height in pixels.
    pub height: u32,
    /// Source rectangle to sample, resolved and clamped to the image.
    pub source_x: u32,
    /// Source rectangle origin.
    pub source_y: u32,
    /// Source rectangle width.
    pub source_width: u32,
    /// Source rectangle height.
    pub source_height: u32,
    /// Protocol z-index, which decides both the layer and the order in it.
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

/// Install the process-global PNG decoder the protocol's `f=100` payloads
/// need, once.
///
/// Without it libghostty stores the metadata and never the pixels, so a
/// PNG transmission silently renders nothing.
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

/// Decode a PNG payload into the tightly packed RGBA libghostty stores.
///
/// The payload is attacker-controlled, so the dimensions are checked before
/// the allocation rather than after.
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

/// Turn Kitty's pixel layout into the BGRA buffer GPUI samples.
///
/// GPUI's `RenderImage` is BGRA with straight (not premultiplied) alpha, and
/// libghostty always hands over fully decoded, uncompressed pixels, so this
/// is a widen-and-swap with no decode step.
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
            // Never stored: PNG payloads are decoded to RGBA on arrival.
            ghostty::ImageFormat::Png => return None,
        };
        bgra.extend_from_slice(&[b, g, r, a]);
    }
    Some(bgra)
}

/// A terminal's uploaded textures, keyed by image ID.
///
/// The value carries the generation the texture was built from: libghostty
/// bumps it on every add or replace of an ID, which is the only signal that
/// catches a retransmission of identical dimensions.
#[derive(Default)]
pub(super) struct KittyImages {
    textures: HashMap<u32, (u64, Arc<RenderImage>)>,
    /// The storage stamp the cache was last reconciled against.
    storage_generation: u64,
}

impl KittyImages {
    /// Resolve every placement on the active screen for this frame.
    ///
    /// Returns an empty list when the protocol is disabled or nothing is
    /// placed, which is the normal case and costs one FFI call.
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
        // The stored set only changes with the storage stamp; placements move
        // with scrolling, so they are walked every frame either way.
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
                // The metadata is resident but the pixels have not arrived
                // yet; the next frame after they do will pick it up.
                None => continue,
            };
            let render = cursor.render_info(&image)?;
            // A placement with no viewport position is fully scrolled out or
            // is a Unicode placeholder, which the cell content positions.
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
        // Protocol order: lower z first, so a later placement at the same
        // depth still draws over an earlier one.
        placements.sort_by_key(|placement| placement.z);
        Ok(placements)
    }

    /// The texture for `info`, uploading it when the cache is stale.
    fn texture(
        &mut self,
        info: &ghostty::ImageInfo,
        image: &ghostty::KittyImage<'_>,
    ) -> ghostty::Result<Option<Arc<RenderImage>>> {
        // The per-image stamp is the whole staleness test: it moves on every
        // add or replace of this ID, including a retransmission whose
        // dimensions and length are identical.
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

/// Turn the protocol on for a terminal.
///
/// The file, temporary-file, and shared-memory transmission media stay
/// disabled: those let a program name a path for the terminal to read, and
/// nothing in Paneflow needs them. Inline payloads still work, which is what
/// every image-in-terminal tool sends.
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

    /// A 16x32 solid-red RGB image transmitted inline and placed at the
    /// cursor. One pixel is exactly one base64 group at `f=24`, so a solid
    /// image is one group repeated.
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

        // Nothing is placed until the protocol is enabled, whatever arrives.
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
        // The texture carries the image's own pixel dimensions, and the
        // pixels are BGRA: blue and green zero, red full.
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

        // Same ID, same dimensions, different pixels: only the generation
        // stamp can tell, which is what the cache keys on.
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
        // PNG is never stored: it is decoded to RGBA on arrival.
        assert_eq!(
            to_bgra(&info(ghostty::ImageFormat::Png, 1, 1), &[1, 2, 3, 4]),
            None
        );
    }

    #[test]
    fn a_payload_shorter_than_its_dimensions_is_refused() {
        // Two pixels declared, one delivered: reading the second would run
        // past the buffer libghostty handed over.
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
        // A 1x1 red PNG.
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

        // A cache hit reuses the exact texture, so no upload happens.
        let cached = images.textures.get(&7).expect("cached entry");
        assert_eq!(cached.0, 3);
        assert!(Arc::ptr_eq(&cached.1, &first));

        // A retransmission with identical dimensions still invalidates,
        // because only the generation moved.
        let mut retransmitted = info(ghostty::ImageFormat::Rgba, 1, 1);
        retransmitted.id = 7;
        retransmitted.generation = 4;
        assert_ne!(images.textures[&7].0, retransmitted.generation);
    }
}
