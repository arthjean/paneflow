use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use paneflow_config::schema::AppIcon;

#[cfg(any(target_os = "windows", test))]
mod ico;
#[cfg(target_os = "windows")]
mod shortcut;

const PREVIEW_MEDIUM_SCALE: f32 = 1.25;
const PREVIEW_DENSE_SCALE: f32 = 1.75;
const PREVIEW_SIZE: u32 = 64;
const PREVIEW_MEDIUM_SIZE: u32 = 96;
const PREVIEW_DENSE_SIZE: u32 = 128;
const GLASS_PREVIEW_SLUG: &str = "glass";
const X11_ICON_SIZE: u32 = 128;
#[cfg(any(target_os = "windows", test))]
const BITMAP_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 96, 128];
#[cfg(any(target_os = "windows", test))]
const ICON_FILE_SMALL_SIZES: [u32; 2] = [16, 24];
#[cfg(any(target_os = "windows", test))]
const ICON_FILE_SCALED_SIZES: [u32; 5] = [32, 40, 48, 64, 96];
#[cfg(any(target_os = "windows", test))]
const ICON_FILE_LARGEST_SIZE: u32 = 128;
#[cfg(any(target_os = "windows", test))]
const SLUGS_WITH_A_SCALED_DRAWING: [&str; 2] = ["black", "white"];
#[cfg(any(target_os = "macos", test))]
const MACOS_ICON_SIZE: u32 = 512;

static APPLIED: AtomicU8 = AtomicU8::new(0);
static SHORTCUTS_SYNCED: AtomicBool = AtomicBool::new(false);

fn code(icon: AppIcon) -> u8 {
    match icon {
        AppIcon::Glass => 0,
        AppIcon::Black => 1,
        AppIcon::White => 2,
        AppIcon::BlackSolid => 3,
        AppIcon::WhiteSolid => 4,
        AppIcon::Aubergine => 5,
        AppIcon::Paper => 6,
        AppIcon::MidnightMatte => 7,
        AppIcon::Polar => 8,
        AppIcon::Melon => 9,
        AppIcon::Honeydew => 10,
        AppIcon::Berry => 11,
        AppIcon::Rainbow => 12,
        AppIcon::Solarwave => 13,
        AppIcon::Neon => 14,
        AppIcon::Phosphor => 15,
        AppIcon::Holo => 16,
        AppIcon::Amethyst => 17,
        AppIcon::Blueprint => 18,
        AppIcon::Chip => 19,
        AppIcon::Nostalgia => 20,
    }
}

#[cfg(target_os = "windows")]
fn applied() -> AppIcon {
    let applied = APPLIED.load(Ordering::Relaxed);
    AppIcon::ALL
        .iter()
        .copied()
        .find(|&icon| code(icon) == applied)
        .unwrap_or_default()
}

fn asset_slug(icon: AppIcon) -> Option<&'static str> {
    match icon {
        AppIcon::Glass => None,
        AppIcon::Black => Some("black"),
        AppIcon::White => Some("white"),
        AppIcon::BlackSolid => Some("black-solid"),
        AppIcon::WhiteSolid => Some("white-solid"),
        AppIcon::Aubergine => Some("aubergine"),
        AppIcon::Paper => Some("paper"),
        AppIcon::MidnightMatte => Some("midnight-matte"),
        AppIcon::Polar => Some("polar"),
        AppIcon::Melon => Some("melon"),
        AppIcon::Honeydew => Some("honeydew"),
        AppIcon::Berry => Some("berry"),
        AppIcon::Rainbow => Some("rainbow"),
        AppIcon::Solarwave => Some("solarwave"),
        AppIcon::Neon => Some("neon"),
        AppIcon::Phosphor => Some("phosphor"),
        AppIcon::Holo => Some("holo"),
        AppIcon::Amethyst => Some("amethyst"),
        AppIcon::Blueprint => Some("blueprint"),
        AppIcon::Chip => Some("chip"),
        AppIcon::Nostalgia => Some("nostalgia"),
    }
}

fn bitmap_asset(slug: &str, size: u32) -> String {
    format!("icons/app/{slug}-{size}.png")
}

#[cfg(any(target_os = "windows", test))]
fn scaled_asset(slug: &str, size: u32) -> String {
    if SLUGS_WITH_A_SCALED_DRAWING.contains(&slug) {
        format!("icons/app/{slug}-shell-{size}.png")
    } else {
        bitmap_asset(slug, size)
    }
}

#[cfg(any(target_os = "windows", test))]
fn icon_file_assets(slug: &str) -> Vec<String> {
    ICON_FILE_SMALL_SIZES
        .iter()
        .map(|&size| bitmap_asset(slug, size))
        .chain(
            ICON_FILE_SCALED_SIZES
                .iter()
                .map(|&size| scaled_asset(slug, size)),
        )
        .chain(std::iter::once(bitmap_asset(slug, ICON_FILE_LARGEST_SIZE)))
        .collect()
}

#[cfg(any(target_os = "macos", test))]
fn macos_asset(slug: &str) -> String {
    format!("icons/app/{slug}-macos-{MACOS_ICON_SIZE}.png")
}

#[cfg(any(target_os = "windows", test))]
fn drawn_size(sizes: &[u32], logical: u32, dpi: u32) -> u32 {
    let wanted = (logical * dpi).div_ceil(96);
    sizes
        .iter()
        .copied()
        .find(|&size| size >= wanted)
        .or_else(|| sizes.last().copied())
        .unwrap_or(wanted)
}

pub(crate) fn preview_asset(icon: AppIcon, scale_factor: f32) -> String {
    let size = if scale_factor > PREVIEW_DENSE_SCALE {
        PREVIEW_DENSE_SIZE
    } else if scale_factor > PREVIEW_MEDIUM_SCALE {
        PREVIEW_MEDIUM_SIZE
    } else {
        PREVIEW_SIZE
    };
    bitmap_asset(asset_slug(icon).unwrap_or(GLASS_PREVIEW_SLUG), size)
}

pub(crate) fn window_icon(icon: AppIcon) -> Option<Arc<image::RgbaImage>> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let slug = asset_slug(icon)?;
    let file = crate::assets::Assets::get(&bitmap_asset(slug, X11_ICON_SIZE))?;
    match image::load_from_memory_with_format(&file.data, image::ImageFormat::Png) {
        Ok(decoded) => Some(Arc::new(decoded.into_rgba8())),
        Err(err) => {
            log::warn!("Could not decode the {slug} app icon: {err}");
            None
        }
    }
}

pub(crate) fn apply_to_window(window: &gpui::Window, icon: AppIcon) {
    #[cfg(target_os = "windows")]
    windows::apply(window, icon);
    #[cfg(not(target_os = "windows"))]
    let _ = (window, icon);
}

pub(crate) fn apply(icon: AppIcon, cx: &mut gpui::App) {
    let changed = APPLIED.swap(code(icon), Ordering::Relaxed) != code(icon);
    let first = !SHORTCUTS_SYNCED.swap(true, Ordering::Relaxed);
    #[cfg(target_os = "windows")]
    if changed || first {
        windows::sync_shortcuts();
    }
    #[cfg(not(target_os = "windows"))]
    let _ = first;
    if !changed {
        return;
    }
    #[cfg(target_os = "macos")]
    macos::apply(icon);
    #[cfg(target_os = "windows")]
    cx.defer(move |cx| {
        for handle in cx.windows() {
            let _ = handle.update(cx, |_, window, _| apply_to_window(window, icon));
        }
    });
    #[cfg(not(target_os = "windows"))]
    let _ = cx;
}

#[cfg(target_os = "windows")]
mod windows {
    use std::sync::Mutex;

    use paneflow_config::schema::AppIcon;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateIconFromResourceEx, ICON_BIG, ICON_SMALL, LR_DEFAULTCOLOR, SendMessageW, WM_SETICON,
    };

    const ICON_RESOURCE_VERSION: u32 = 0x0003_0000;
    const ICON_FILE_HASH_BYTES: usize = 4;
    const SMALL_ICON_AT_96_DPI: u32 = 16;
    const LARGE_ICON_AT_96_DPI: u32 = 32;
    const DEFAULT_DPI: u32 = 96;
    const MIRROR_RECORD: &str = "start-menu-shortcuts";

    static HANDLES: Mutex<Vec<(String, isize)>> = Mutex::new(Vec::new());
    static SHORTCUT_SYNC: Mutex<()> = Mutex::new(());

    fn icon_handle(asset: String, size: u32) -> Option<isize> {
        let mut handles = HANDLES.lock().ok()?;
        if let Some((_, handle)) = handles.iter().find(|(cached, _)| *cached == asset) {
            return Some(*handle);
        }
        let file = crate::assets::Assets::get(&asset)?;
        let length = u32::try_from(file.data.len()).ok()?;
        let side = i32::try_from(size).ok()?;
        let handle = unsafe {
            CreateIconFromResourceEx(
                file.data.as_ptr(),
                length,
                1,
                ICON_RESOURCE_VERSION,
                side,
                side,
                LR_DEFAULTCOLOR,
            )
        };
        if handle.is_null() {
            log::warn!("Could not create the app icon from {asset}");
            return None;
        }
        handles.push((asset, handle as isize));
        Some(handle as isize)
    }

    fn icon_file_bytes(slug: &str) -> Option<Vec<u8>> {
        let assets = super::icon_file_assets(slug);
        let mut images = Vec::with_capacity(assets.len());
        for asset in assets {
            let file = crate::assets::Assets::get(&asset)?;
            let decoded =
                image::load_from_memory_with_format(&file.data, image::ImageFormat::Png).ok()?;
            images.push(decoded.into_rgba8());
        }
        super::ico::encode(&images)
    }

    fn icons_directory() -> Option<std::path::PathBuf> {
        Some(crate::runtime_paths::data_dir()?.join("icons"))
    }

    fn ensure_icon_file(slug: &str) -> Option<std::path::PathBuf> {
        use sha2::Digest;

        let bytes = icon_file_bytes(slug)?;
        let digest = sha2::Sha256::digest(&bytes);
        let hash: String = digest
            .iter()
            .take(ICON_FILE_HASH_BYTES)
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let directory = icons_directory()?;
        let path = directory.join(format!("paneflow-{slug}-{hash}.ico"));
        if std::fs::read(&path).is_ok_and(|existing| existing == bytes) {
            return Some(path);
        }
        if let Err(err) =
            std::fs::create_dir_all(&directory).and_then(|()| std::fs::write(&path, &bytes))
        {
            log::warn!("Could not write the app icon {}: {err}", path.display());
            return None;
        }
        Some(path)
    }

    fn sync_shortcuts_to_the_applied_icon() {
        let Ok(_exclusive) = SHORTCUT_SYNC.lock() else {
            return;
        };
        let icon = super::applied();
        let Ok(executable) = std::env::current_exe() else {
            return;
        };
        let menu = super::shortcut::start_menu();
        let record = icons_directory().map(|directory| directory.join(MIRROR_RECORD));
        let location = match super::asset_slug(icon) {
            None => {
                if let (Some(menu), Some(record)) = (&menu, &record) {
                    super::shortcut::remove_mirrored_shortcuts(menu, record);
                }
                String::new()
            }
            Some(slug) => {
                let Some(path) = ensure_icon_file(slug) else {
                    return;
                };
                if let (Some(menu), Some(record)) = (&menu, &record) {
                    super::shortcut::mirror_machine_shortcuts(menu, &executable, record);
                }
                path.to_string_lossy().into_owned()
            }
        };
        let rewritten = super::shortcut::point_shortcuts_at(
            &super::shortcut::user_shortcut_roots(),
            &executable,
            &location,
        );
        if rewritten > 0 {
            log::info!("Pointed {rewritten} Paneflow shortcut(s) at the chosen app icon");
        }
    }

    pub(super) fn sync_shortcuts() {
        let spawned = std::thread::Builder::new()
            .name("paneflow-app-icon".into())
            .spawn(sync_shortcuts_to_the_applied_icon);
        if let Err(err) = spawned {
            log::warn!("Could not start the app icon shortcut sync: {err}");
        }
    }

    pub(super) fn apply(window: &gpui::Window, icon: AppIcon) {
        let Ok(window_handle) = HasWindowHandle::window_handle(window) else {
            return;
        };
        let RawWindowHandle::Win32(handle) = window_handle.as_raw() else {
            return;
        };
        let hwnd = handle.hwnd.get() as HWND;
        let dpi = match unsafe { GetDpiForWindow(hwnd) } {
            0 => DEFAULT_DPI,
            dpi => dpi,
        };
        let (small, big) = match super::asset_slug(icon) {
            None => (0, 0),
            Some(slug) => {
                let small = super::drawn_size(&super::BITMAP_SIZES, SMALL_ICON_AT_96_DPI, dpi);
                let big =
                    super::drawn_size(&super::ICON_FILE_SCALED_SIZES, LARGE_ICON_AT_96_DPI, dpi);
                (
                    icon_handle(super::bitmap_asset(slug, small), small).unwrap_or_default(),
                    icon_handle(super::scaled_asset(slug, big), big).unwrap_or_default(),
                )
            }
        };
        unsafe {
            SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, small);
            SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, big);
        }
    }
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
mod macos {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    use paneflow_config::schema::AppIcon;

    pub(super) fn apply(icon: AppIcon) {
        let file = super::asset_slug(icon)
            .and_then(|slug| crate::assets::Assets::get(&super::macos_asset(slug)));
        unsafe {
            let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
            let image: *mut Object = match &file {
                Some(file) => {
                    let data: *mut Object = msg_send![
                        class!(NSData),
                        dataWithBytes: file.data.as_ptr().cast::<std::ffi::c_void>()
                        length: file.data.len()
                    ];
                    let image: *mut Object = msg_send![class!(NSImage), alloc];
                    msg_send![image, initWithData: data]
                }
                None => std::ptr::null_mut(),
            };
            let _: () = msg_send![app, setApplicationIconImage: image];
            if !image.is_null() {
                let _: () = msg_send![image, release];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_side(path: &str) -> u32 {
        let file = crate::assets::Assets::get(path).unwrap_or_else(|| panic!("{path} is embedded"));
        let decoded = image::load_from_memory_with_format(&file.data, image::ImageFormat::Png)
            .unwrap_or_else(|err| panic!("{path} decodes: {err}"));
        assert_eq!(decoded.width(), decoded.height(), "{path} is square");
        decoded.width()
    }

    #[test]
    fn every_icon_embeds_each_bitmap_at_its_named_size() {
        for &icon in AppIcon::ALL {
            let Some(slug) = asset_slug(icon) else {
                continue;
            };
            for size in BITMAP_SIZES {
                assert_eq!(png_side(&bitmap_asset(slug, size)), size);
            }
            assert_eq!(png_side(&macos_asset(slug)), MACOS_ICON_SIZE);
        }
    }

    #[test]
    fn the_icon_file_lists_each_image_once_in_ascending_size() {
        for &icon in AppIcon::ALL {
            let Some(slug) = asset_slug(icon) else {
                continue;
            };
            for size in ICON_FILE_SCALED_SIZES {
                assert_eq!(png_side(&scaled_asset(slug, size)), size);
            }
            let sides: Vec<u32> = icon_file_assets(slug)
                .iter()
                .map(|asset| png_side(asset))
                .collect();
            assert_eq!(sides, [16, 24, 32, 40, 48, 64, 96, 128]);
        }
    }

    #[test]
    fn every_icon_has_a_preview_drawn_for_each_display_density() {
        for &icon in AppIcon::ALL {
            assert_eq!(png_side(&preview_asset(icon, 1.0)), 64);
            assert_eq!(png_side(&preview_asset(icon, 1.5)), 96);
            assert_eq!(png_side(&preview_asset(icon, 2.0)), 128);
        }
    }

    #[test]
    fn only_the_bar_icons_carry_a_separate_scaled_drawing() {
        assert_eq!(scaled_asset("black", 32), "icons/app/black-shell-32.png");
        assert_eq!(scaled_asset("white", 96), "icons/app/white-shell-96.png");
        assert_eq!(
            scaled_asset("black-solid", 40),
            "icons/app/black-solid-40.png"
        );
        assert_eq!(scaled_asset("aubergine", 32), "icons/app/aubergine-32.png");
    }

    #[test]
    fn every_icon_keeps_its_own_code_and_artwork() {
        let mut codes: Vec<u8> = AppIcon::ALL.iter().map(|&icon| code(icon)).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), AppIcon::ALL.len());
        let mut slugs: Vec<&str> = AppIcon::ALL
            .iter()
            .filter_map(|&icon| asset_slug(icon))
            .collect();
        slugs.sort_unstable();
        slugs.dedup();
        assert_eq!(slugs.len(), AppIcon::ALL.len() - 1);
    }

    #[test]
    fn glass_keeps_the_installed_icon() {
        assert_eq!(asset_slug(AppIcon::Glass), None);
        assert!(window_icon(AppIcon::Glass).is_none());
    }

    #[test]
    fn the_drawn_size_covers_the_scaled_icon_without_upscaling() {
        assert_eq!(drawn_size(&BITMAP_SIZES, 16, 96), 16);
        assert_eq!(drawn_size(&BITMAP_SIZES, 16, 120), 24);
        assert_eq!(drawn_size(&BITMAP_SIZES, 16, 192), 32);
        assert_eq!(drawn_size(&BITMAP_SIZES, 16, 960), 128);
        assert_eq!(drawn_size(&ICON_FILE_SCALED_SIZES, 32, 96), 32);
        assert_eq!(drawn_size(&ICON_FILE_SCALED_SIZES, 32, 120), 40);
        assert_eq!(drawn_size(&ICON_FILE_SCALED_SIZES, 32, 144), 48);
        assert_eq!(drawn_size(&ICON_FILE_SCALED_SIZES, 32, 168), 64);
        assert_eq!(drawn_size(&ICON_FILE_SCALED_SIZES, 32, 288), 96);
        assert_eq!(drawn_size(&ICON_FILE_SCALED_SIZES, 32, 960), 96);
    }
}
