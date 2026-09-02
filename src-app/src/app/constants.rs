use gpui::{Hsla, Pixels, WindowBackgroundAppearance, px};

pub(crate) const SIDEBAR_WIDTH: f32 = 300.;
pub(crate) const TITLE_BAR_EDGE_INSET: Pixels = px(8.);
pub(crate) const TITLE_BAR_CONTROL_SPACING: Pixels = px(12.);
pub(crate) const TITLE_BAR_CONTROL_SIZE: Pixels = px(20.);
pub(crate) const TITLE_BAR_MIN_HEIGHT: Pixels = px(32.);
pub(crate) const PANEL_INSET: f32 = 4.;
pub(crate) const PANEL_CORNER_RADIUS: Pixels = WINDOW_CORNER_RADIUS;
pub(crate) const PANE_CARD_RADIUS: Pixels = px(20.);
pub(crate) const PANE_CONTENT_INSET_X: f32 = 10.;
pub(crate) const PANE_CONTENT_INSET_Y: f32 = 6.;
const DARK_SIDEBAR_TAB_TINT: u32 = 0xffffff;
const LIGHT_SIDEBAR_TAB_TINT: u32 = 0x262626;
const DARK_SIDEBAR_TAB_ACTIVE_OPACITY: f32 = 0.11;
const DARK_SIDEBAR_TAB_HOVER_OPACITY: f32 = 0.07;
const LIGHT_SIDEBAR_TAB_ACTIVE_OPACITY: f32 = 0.08;
const LIGHT_SIDEBAR_TAB_HOVER_OPACITY: f32 = 0.04;
const SIDEBAR_TAB_ICON_CARD_TINT: u32 = 0x000000;
const DARK_SIDEBAR_TAB_ICON_CARD_DARKEN: f32 = 0.10;
const LIGHT_SIDEBAR_TAB_ICON_CARD_DARKEN: f32 = 0.05;

pub(crate) const SIDEBAR_TAB_CORNER_RADIUS: Pixels = px(8.);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowBackdropPreference {
    Auto,
    Mica,
    Blurred,
    Transparent,
    Opaque,
}

pub(crate) fn window_backdrop_preference(config_value: Option<&str>) -> WindowBackdropPreference {
    if let Ok(value) = std::env::var("PANEFLOW_WINDOW_BACKDROP") {
        return parse_window_backdrop_preference(&value);
    }

    config_window_backdrop_preference(config_value)
}

fn config_window_backdrop_preference(config_value: Option<&str>) -> WindowBackdropPreference {
    #[cfg(target_os = "windows")]
    if let Some(value) = config_value.map(str::trim)
        && (value.eq_ignore_ascii_case("blurred") || value.eq_ignore_ascii_case("acrylic"))
    {
        return WindowBackdropPreference::Auto;
    }

    config_value
        .map(parse_window_backdrop_preference)
        .unwrap_or(WindowBackdropPreference::Auto)
}

fn parse_window_backdrop_preference(value: &str) -> WindowBackdropPreference {
    match value.trim().to_ascii_lowercase() {
        value if value.is_empty() || value == "auto" => WindowBackdropPreference::Auto,
        value if value == "mica" => WindowBackdropPreference::Mica,
        value if value == "blurred" || value == "acrylic" => WindowBackdropPreference::Blurred,
        value if value == "transparent" => WindowBackdropPreference::Transparent,
        value if value == "opaque" || value == "off" => WindowBackdropPreference::Opaque,
        value => {
            log::warn!("Invalid window_backdrop value '{value}', using 'auto'");
            WindowBackdropPreference::Auto
        }
    }
}

pub(crate) fn window_background_appearance(
    config_value: Option<&str>,
) -> WindowBackgroundAppearance {
    let preference = window_backdrop_preference(config_value);
    window_background_appearance_for_preference(preference)
}

fn window_background_appearance_for_preference(
    preference: WindowBackdropPreference,
) -> WindowBackgroundAppearance {
    #[cfg(target_os = "windows")]
    {
        match preference {
            WindowBackdropPreference::Auto | WindowBackdropPreference::Mica => {
                if windows_supports_system_backdrop() {
                    WindowBackgroundAppearance::MicaBackdrop
                } else {
                    WindowBackgroundAppearance::Opaque
                }
            }
            WindowBackdropPreference::Blurred => WindowBackgroundAppearance::Blurred,
            WindowBackdropPreference::Transparent => WindowBackgroundAppearance::Transparent,
            WindowBackdropPreference::Opaque => WindowBackgroundAppearance::Opaque,
        }
    }

    #[cfg(target_os = "macos")]
    {
        match preference {
            WindowBackdropPreference::Opaque => WindowBackgroundAppearance::Opaque,
            WindowBackdropPreference::Blurred => WindowBackgroundAppearance::Blurred,
            _ => WindowBackgroundAppearance::Transparent,
        }
    }

    #[cfg(target_os = "linux")]
    {
        let _ = preference;
        WindowBackgroundAppearance::Opaque
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        let _ = preference;
        WindowBackgroundAppearance::Opaque
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn window_backdrop_uses_mica(config_value: Option<&str>) -> bool {
    matches!(
        window_background_appearance(config_value),
        WindowBackgroundAppearance::MicaBackdrop
    )
}

#[cfg(target_os = "macos")]
pub(crate) fn macos_sidebar_material_enabled(config_value: Option<&str>) -> bool {
    !matches!(
        window_backdrop_preference(config_value),
        WindowBackdropPreference::Opaque | WindowBackdropPreference::Transparent
    )
}

#[cfg(target_os = "windows")]
fn windows_supports_system_backdrop() -> bool {
    #[repr(C)]
    struct RtlOsVersionInfo {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform_id: u32,
        service_pack: [u16; 128],
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn RtlGetVersion(version: *mut RtlOsVersionInfo) -> i32;
    }

    let mut version = RtlOsVersionInfo {
        size: std::mem::size_of::<RtlOsVersionInfo>() as u32,
        major: 0,
        minor: 0,
        build: 0,
        platform_id: 0,
        service_pack: [0; 128],
    };

    unsafe { RtlGetVersion(&mut version) >= 0 && version.build >= 22_621 }
}

pub(crate) fn cockpit_chrome_background(
    background: Hsla,
    is_window_active: bool,
    material_active: bool,
) -> Hsla {
    #[cfg(target_os = "windows")]
    {
        let _ = is_window_active;
        if material_active {
            gpui::transparent_black()
        } else {
            Hsla {
                a: 1.0,
                ..background
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (background, is_window_active, material_active);
        gpui::transparent_black()
    }
}

pub(crate) fn cockpit_backdrop_background(
    background: Hsla,
    is_window_active: bool,
    material_active: bool,
) -> Hsla {
    #[cfg(target_os = "linux")]
    {
        let _ = (is_window_active, material_active);
        Hsla {
            a: 1.0,
            ..background
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = is_window_active;
        if !material_active {
            background
        } else if cfg!(any(target_os = "windows", target_os = "macos")) {
            gpui::transparent_black()
        } else {
            background
        }
    }
}

pub(crate) fn sidebar_tab_active_background() -> Hsla {
    sidebar_tab_background(
        LIGHT_SIDEBAR_TAB_ACTIVE_OPACITY,
        DARK_SIDEBAR_TAB_ACTIVE_OPACITY,
    )
}

pub(crate) fn sidebar_tab_hover_background() -> Hsla {
    sidebar_tab_background(
        LIGHT_SIDEBAR_TAB_HOVER_OPACITY,
        DARK_SIDEBAR_TAB_HOVER_OPACITY,
    )
}

pub(crate) fn sidebar_tab_icon_card_background() -> Hsla {
    let theme = crate::theme::active_theme();
    let is_light = theme.background.l > 0.5;
    let (tab_tint, tab_opacity, darken) = if is_light {
        (
            LIGHT_SIDEBAR_TAB_TINT,
            LIGHT_SIDEBAR_TAB_ACTIVE_OPACITY,
            LIGHT_SIDEBAR_TAB_ICON_CARD_DARKEN,
        )
    } else {
        (
            DARK_SIDEBAR_TAB_TINT,
            DARK_SIDEBAR_TAB_ACTIVE_OPACITY,
            DARK_SIDEBAR_TAB_ICON_CARD_DARKEN,
        )
    };
    let card = Hsla {
        a: 1.0,
        ..theme.title_bar_background
    }
    .blend(Hsla::from(gpui::rgb(tab_tint)).opacity(tab_opacity));
    card.blend(Hsla::from(gpui::rgb(SIDEBAR_TAB_ICON_CARD_TINT)).opacity(darken))
}

fn sidebar_tab_background(light_opacity: f32, dark_opacity: f32) -> Hsla {
    let theme = crate::theme::active_theme();
    let is_light = theme.background.l > 0.5;
    let (tint, opacity) = if is_light {
        (LIGHT_SIDEBAR_TAB_TINT, light_opacity)
    } else {
        (DARK_SIDEBAR_TAB_TINT, dark_opacity)
    };
    let tint = Hsla::from(gpui::rgb(tint)).opacity(opacity);

    #[cfg(target_os = "linux")]
    {
        Hsla {
            a: 1.0,
            ..theme.title_bar_background
        }
        .blend(tint)
    }

    #[cfg(not(target_os = "linux"))]
    tint
}

pub(crate) const TOAST_ENTER_MS: u64 = 180;
pub(crate) const TOAST_HOLD_MS: u64 = 1440;
pub(crate) const TOAST_EXIT_MS: u64 = 180;

pub(crate) const MAX_CLOSED_PANES: usize = 5;

pub(crate) const MAX_CLOSED_PANE_SCROLLBACK_BYTES: usize = 2 * 1024 * 1024;

pub(crate) const RESIZE_BORDER: Pixels = px(10.0);
pub(crate) const WINDOW_CORNER_RADIUS: Pixels = px(10.0);
pub(crate) const WINDOW_BORDER_SIZE: Pixels = px(1.0);

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn linux_window_starts_opaque_until_the_display_backend_is_known() {
        for preference in [
            WindowBackdropPreference::Auto,
            WindowBackdropPreference::Mica,
            WindowBackdropPreference::Blurred,
            WindowBackdropPreference::Transparent,
            WindowBackdropPreference::Opaque,
        ] {
            assert_eq!(
                window_background_appearance_for_preference(preference),
                WindowBackgroundAppearance::Opaque
            );
        }
    }
}

#[cfg(test)]
mod material_tests {
    use super::*;

    #[cfg(target_os = "windows")]
    #[test]
    fn legacy_blurred_config_falls_back_to_auto_after_mica_subsetting_removal() {
        assert_eq!(
            config_window_backdrop_preference(Some("blurred")),
            WindowBackdropPreference::Auto
        );
        assert_eq!(
            config_window_backdrop_preference(Some("acrylic")),
            WindowBackdropPreference::Auto
        );
        assert_eq!(
            parse_window_backdrop_preference("blurred"),
            WindowBackdropPreference::Blurred
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn cockpit_children_stay_transparent_over_an_opaque_shell() {
        let background = Hsla::from(gpui::rgb(0x141414));

        assert_eq!(
            cockpit_chrome_background(background, true, false),
            gpui::transparent_black()
        );
        assert_eq!(
            cockpit_backdrop_background(background, true, false),
            background
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn disabled_chrome_material_keeps_title_bar_opaque() {
        let background = Hsla::from(gpui::rgb(0x141414));

        assert_eq!(
            cockpit_chrome_background(background, true, false),
            Hsla {
                a: 1.0,
                ..background
            }
        );
        assert_eq!(
            cockpit_chrome_background(background, true, true),
            gpui::transparent_black()
        );
        assert_eq!(
            cockpit_backdrop_background(background, true, false),
            background
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_chrome_stays_opaque_when_material_is_requested() {
        let dark = gpui::hsla(0.71, 0.62, 0.32, 0.42);
        let light = gpui::hsla(0.09, 0.54, 0.78, 0.58);

        assert_eq!(
            cockpit_backdrop_background(dark, true, true),
            Hsla { a: 1.0, ..dark }
        );
        assert_eq!(
            cockpit_backdrop_background(light, true, true),
            Hsla { a: 1.0, ..light }
        );
    }
}
