use std::borrow::Cow;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use gpui::{Image, ImageFormat};

const DIR_NAME: &str = "clipboard-images";
const RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

pub(super) fn default_dir() -> Option<PathBuf> {
    paneflow_home::cache_dir().map(|cache| cache.join(DIR_NAME))
}

pub(super) fn materialize(image: &Image, dir: &Path) -> io::Result<PathBuf> {
    let (bytes, extension) = agent_readable(image)?;
    paneflow_home::create_private_dir_all(dir)?;
    prune_expired(dir, SystemTime::now());
    let path = dir.join(format!("{:016x}.{extension}", image.id));
    paneflow_home::write_atomically(&path, &bytes)?;
    Ok(path)
}

fn agent_readable(image: &Image) -> io::Result<(Cow<'_, [u8]>, &'static str)> {
    let stored_as_is = |extension| Ok((Cow::Borrowed(image.bytes.as_slice()), extension));
    match image.format {
        ImageFormat::Png => stored_as_is("png"),
        ImageFormat::Jpeg => stored_as_is("jpg"),
        ImageFormat::Gif => stored_as_is("gif"),
        ImageFormat::Webp => stored_as_is("webp"),
        ImageFormat::Bmp => transcoded_to_png(&image.bytes, image::ImageFormat::Bmp),
        ImageFormat::Tiff => transcoded_to_png(&image.bytes, image::ImageFormat::Tiff),
        other => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("a {other:?} clipboard image has no form an agent can read"),
        )),
    }
}

fn transcoded_to_png(
    bytes: &[u8],
    format: image::ImageFormat,
) -> io::Result<(Cow<'static, [u8]>, &'static str)> {
    let decoded = image::load_from_memory_with_format(bytes, format).map_err(io::Error::other)?;
    let mut png = io::Cursor::new(Vec::new());
    decoded
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(io::Error::other)?;
    Ok((Cow::Owned(png.into_inner()), "png"))
}

fn prune_expired(dir: &Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let expired = entry
            .metadata()
            .ok()
            .filter(std::fs::Metadata::is_file)
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > RETENTION);
        if expired {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn red_square(format: image::ImageFormat) -> Vec<u8> {
        let pixels = image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]));
        let mut encoded = io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(pixels)
            .write_to(&mut encoded, format)
            .expect("encode the fixture");
        encoded.into_inner()
    }

    #[test]
    fn a_bmp_clipboard_image_is_stored_as_a_png_with_the_same_pixels() {
        let dir = tempfile::tempdir().expect("tempdir");
        let copied = Image::from_bytes(ImageFormat::Bmp, red_square(image::ImageFormat::Bmp));

        let path = materialize(&copied, dir.path()).expect("stored");

        assert_eq!(path, dir.path().join(format!("{:016x}.png", copied.id)));
        let stored = std::fs::read(&path).expect("read back");
        let decoded = image::load_from_memory_with_format(&stored, image::ImageFormat::Png)
            .expect("a valid PNG")
            .to_rgba8();
        assert!(decoded.pixels().all(|pixel| pixel.0 == [255, 0, 0, 255]));
    }

    #[test]
    fn an_agent_readable_clipboard_image_is_stored_byte_for_byte() {
        let dir = tempfile::tempdir().expect("tempdir");
        let png = red_square(image::ImageFormat::Png);
        let copied = Image::from_bytes(ImageFormat::Png, png.clone());

        let path = materialize(&copied, dir.path()).expect("stored");

        assert_eq!(std::fs::read(&path).expect("read back"), png);
    }

    #[test]
    fn storing_an_image_prunes_only_the_expired_ones() {
        let dir = tempfile::tempdir().expect("tempdir");
        let expired = dir.path().join("expired.png");
        let recent = dir.path().join("recent.png");
        std::fs::write(&expired, b"old").expect("seed the expired image");
        std::fs::write(&recent, b"new").expect("seed the recent image");
        std::fs::File::options()
            .write(true)
            .open(&expired)
            .and_then(|file| file.set_modified(SystemTime::now() - RETENTION * 2))
            .expect("age the expired image");

        let copied = Image::from_bytes(ImageFormat::Png, red_square(image::ImageFormat::Png));
        materialize(&copied, dir.path()).expect("stored");

        assert!(!expired.exists(), "an image past the retention is pruned");
        assert!(recent.exists(), "a recent image survives");
    }
}
