const DIRECTORY_HEADER_LEN: usize = 6;
const DIRECTORY_ENTRY_LEN: usize = 16;
const DIB_HEADER_LEN: u32 = 40;
const ICON_RESOURCE_TYPE: u16 = 1;
const COLOR_PLANES: u16 = 1;
const BITS_PER_PIXEL: u16 = 32;

pub(super) fn encode(images: &[image::RgbaImage]) -> Option<Vec<u8>> {
    let count = u16::try_from(images.len()).ok()?;
    let payloads = images.iter().map(dib).collect::<Option<Vec<_>>>()?;
    let mut offset = DIRECTORY_HEADER_LEN + DIRECTORY_ENTRY_LEN * images.len();
    let mut out = Vec::with_capacity(offset + payloads.iter().map(Vec::len).sum::<usize>());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&ICON_RESOURCE_TYPE.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    for (image, payload) in images.iter().zip(&payloads) {
        out.push(u8::try_from(image.width()).ok()?);
        out.push(u8::try_from(image.height()).ok()?);
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&COLOR_PLANES.to_le_bytes());
        out.extend_from_slice(&BITS_PER_PIXEL.to_le_bytes());
        out.extend_from_slice(&u32::try_from(payload.len()).ok()?.to_le_bytes());
        out.extend_from_slice(&u32::try_from(offset).ok()?.to_le_bytes());
        offset += payload.len();
    }
    for payload in &payloads {
        out.extend_from_slice(payload);
    }
    Some(out)
}

fn dib(image: &image::RgbaImage) -> Option<Vec<u8>> {
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return None;
    }
    let color_len = width.checked_mul(height)?.checked_mul(4)?;
    let mask_len = width.div_ceil(32).checked_mul(4)?.checked_mul(height)?;
    let image_len = color_len.checked_add(mask_len)?;
    let mut out = Vec::with_capacity(usize::try_from(DIB_HEADER_LEN + image_len).ok()?);
    out.extend_from_slice(&DIB_HEADER_LEN.to_le_bytes());
    out.extend_from_slice(&i32::try_from(width).ok()?.to_le_bytes());
    out.extend_from_slice(&i32::try_from(height.checked_mul(2)?).ok()?.to_le_bytes());
    out.extend_from_slice(&COLOR_PLANES.to_le_bytes());
    out.extend_from_slice(&BITS_PER_PIXEL.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&image_len.to_le_bytes());
    out.extend_from_slice(&[0; 16]);
    for y in (0..height).rev() {
        for x in 0..width {
            let [red, green, blue, alpha] = image.get_pixel(x, y).0;
            out.extend_from_slice(&[blue, green, red, alpha]);
        }
    }
    out.resize(out.len() + usize::try_from(mask_len).ok()?, 0);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u16_at(bytes: &[u8], at: usize) -> u16 {
        u16::from_le_bytes([bytes[at], bytes[at + 1]])
    }

    fn u32_at(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
    }

    fn gradient(side: u32) -> image::RgbaImage {
        image::RgbaImage::from_fn(side, side, |x, y| {
            image::Rgba([x as u8, y as u8, (x + y) as u8, 255 - x as u8])
        })
    }

    #[test]
    fn the_directory_lists_every_image_with_its_size_and_offset() {
        let images = [gradient(16), gradient(48)];
        let bytes = encode(&images).unwrap();

        assert_eq!(u16_at(&bytes, 0), 0);
        assert_eq!(u16_at(&bytes, 2), 1);
        assert_eq!(u16_at(&bytes, 4), 2);

        let small_len = 40 + 16 * 16 * 4 + 4 * 16;
        let large_len = 40 + 48 * 48 * 4 + 8 * 48;
        assert_eq!((bytes[6], bytes[7]), (16, 16));
        assert_eq!(u16_at(&bytes, 12), 32);
        assert_eq!(u32_at(&bytes, 14), small_len);
        assert_eq!(u32_at(&bytes, 18), 38);
        assert_eq!((bytes[22], bytes[23]), (48, 48));
        assert_eq!(u32_at(&bytes, 30), large_len);
        assert_eq!(u32_at(&bytes, 34), 38 + small_len);
        assert_eq!(bytes.len() as u32, 38 + small_len + large_len);
    }

    #[test]
    fn pixels_are_stored_bottom_up_in_blue_green_red_alpha_order() {
        let image = gradient(16);
        let bytes = encode(std::slice::from_ref(&image)).unwrap();
        let header = 22;

        assert_eq!(u32_at(&bytes, header), 40);
        assert_eq!(u32_at(&bytes, header + 4), 16);
        assert_eq!(u32_at(&bytes, header + 8), 32);

        let first_stored = &bytes[header + 40..header + 44];
        let [red, green, blue, alpha] = image.get_pixel(0, 15).0;
        assert_eq!(first_stored, [blue, green, red, alpha]);

        let last_row = header + 40 + 15 * 16 * 4;
        let [red, green, blue, alpha] = image.get_pixel(3, 0).0;
        assert_eq!(
            &bytes[last_row + 12..last_row + 16],
            [blue, green, red, alpha]
        );
    }

    #[test]
    fn an_image_too_large_for_a_directory_entry_is_refused() {
        assert!(encode(&[gradient(256)]).is_none());
        assert!(encode(&[image::RgbaImage::new(0, 0)]).is_none());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_loads_each_size_from_the_encoded_file() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            DestroyIcon, IMAGE_ICON, LR_LOADFROMFILE, LoadImageW,
        };

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("probe.ico");
        std::fs::write(&path, encode(&[gradient(16), gradient(32)]).unwrap()).unwrap();
        let wide: Vec<u16> = std::os::windows::ffi::OsStrExt::encode_wide(path.as_os_str())
            .chain(std::iter::once(0))
            .collect();

        for side in [16, 32] {
            let handle = unsafe {
                LoadImageW(
                    std::ptr::null_mut(),
                    wide.as_ptr(),
                    IMAGE_ICON,
                    side,
                    side,
                    LR_LOADFROMFILE,
                )
            };
            assert!(!handle.is_null(), "Windows rejects the {side} px image");
            unsafe { DestroyIcon(handle) };
        }
    }
}
