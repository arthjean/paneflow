use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::atomic::{AtomicI8, AtomicIsize, Ordering};

const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;
const DWMSBT_MAINWINDOW: u32 = 2;
const THEME_UNKNOWN: i8 = -1;
const THEME_LIGHT: i8 = 0;
const THEME_DARK: i8 = 1;

static LAST_THEME: AtomicI8 = AtomicI8::new(THEME_UNKNOWN);
static LAST_HWND: AtomicIsize = AtomicIsize::new(0);

#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmSetWindowAttribute(
        hwnd: isize,
        attribute: u32,
        value: *const std::ffi::c_void,
        value_size: u32,
    ) -> i32;
}

pub(crate) fn apply_wallpaper_mica(window: &gpui::Window, is_light: bool) {
    let Some(hwnd) = win32_hwnd(window) else {
        return;
    };

    apply_native_theme(hwnd, is_light);

    let backdrop = DWMSBT_MAINWINDOW;
    let result = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            (&backdrop as *const u32).cast(),
            std::mem::size_of_val(&backdrop) as u32,
        )
    };

    if result < 0 {
        log::debug!("Mica is unavailable; retaining the GPUI backdrop");
    }
}

pub(crate) fn sync_wallpaper_mica_theme(window: &gpui::Window, is_light: bool) {
    let Some(hwnd) = win32_hwnd(window) else {
        return;
    };
    let theme = if is_light { THEME_LIGHT } else { THEME_DARK };

    if LAST_HWND.load(Ordering::Relaxed) == hwnd && LAST_THEME.load(Ordering::Relaxed) == theme {
        return;
    }

    apply_native_theme(hwnd, is_light);
}

fn apply_native_theme(hwnd: isize, is_light: bool) {
    let dark_mode: i32 = if is_light { 0 } else { 1 };
    let result = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&dark_mode as *const i32).cast(),
            std::mem::size_of_val(&dark_mode) as u32,
        )
    };

    if result < 0 {
        log::debug!("Could not align the Windows backdrop with PaneFlow's theme");
        return;
    }

    LAST_HWND.store(hwnd, Ordering::Relaxed);
    LAST_THEME.store(
        if is_light { THEME_LIGHT } else { THEME_DARK },
        Ordering::Relaxed,
    );
}

fn win32_hwnd(window: &gpui::Window) -> Option<isize> {
    let Ok(window_handle) = HasWindowHandle::window_handle(window) else {
        log::warn!("Could not obtain the Win32 window handle for Mica");
        return None;
    };
    let RawWindowHandle::Win32(handle) = window_handle.as_raw() else {
        log::warn!("PaneFlow received a non-Win32 window handle on Windows");
        return None;
    };
    Some(handle.hwnd.get())
}
