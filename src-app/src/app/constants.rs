use gpui::{Hsla, Pixels, WindowBackgroundAppearance, px};

pub(crate) const SIDEBAR_MAX_WIDTH: f32 = 480.;
const SIDEBAR_DEFAULT_SHARE_OF_MAX: f32 = 0.9;
pub(crate) const SIDEBAR_WIDTH: f32 = SIDEBAR_MAX_WIDTH * SIDEBAR_DEFAULT_SHARE_OF_MAX;
pub(crate) const TITLE_BAR_EDGE_INSET: Pixels = px(8.);
pub(crate) const TITLE_BAR_CONTROL_SPACING: Pixels = px(12.);
pub(crate) const TITLE_BAR_CONTROL_SIZE: Pixels = px(20.);
pub(crate) const TITLE_BAR_MIN_HEIGHT: Pixels = px(40.);
pub(crate) const PANEL_INSET: f32 = 4.;
pub(crate) const PANE_OUTER_GUTTER: f32 = crate::layout::PANE_GUTTER_PX - PANEL_INSET;
pub(crate) const PANEL_CORNER_RADIUS: Pixels = WINDOW_CORNER_RADIUS;
pub(crate) const PANE_CARD_RADIUS: Pixels = px(24.);
pub(crate) const SETTINGS_CARD_RADIUS: Pixels = px(20.);
pub(crate) const TOAST_RADIUS: Pixels = px(20.);
pub(crate) const DIFF_DOCK_CONTENT_INSET: Pixels = px(8.);
pub(crate) const PANE_CONTENT_INSET_X: f32 = 10.;
pub(crate) const PANE_CONTENT_INSET_Y: f32 = 6.;
const SIDEBAR_TAB_ACTIVE_OPACITY: f32 = 0.16;
const SIDEBAR_TAB_HOVER_OPACITY: f32 = 0.10;

pub(crate) const SIDEBAR_TAB_CORNER_RADIUS: Pixels = px(8.);
pub(crate) const RAIL_ROW_RADIUS: Pixels = px(9.);

pub(crate) fn rail_row_radius() -> Pixels {
    crate::ui_primitives::interface_style::pick(
        RAIL_ROW_RADIUS,
        crate::ui_primitives::interface_style::CLASSIC_CONTROL,
    )
}

#[cfg(not(target_os = "linux"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowBackdropPreference {
    Auto,
    Mica,
    Blurred,
    Transparent,
    Opaque,
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn window_backdrop_preference(config_value: Option<&str>) -> WindowBackdropPreference {
    if let Ok(value) = std::env::var("PANEFLOW_WINDOW_BACKDROP") {
        return parse_window_backdrop_preference(&value);
    }

    config_window_backdrop_preference(config_value)
}

#[cfg(not(target_os = "linux"))]
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

#[cfg(not(target_os = "linux"))]
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

#[cfg(not(target_os = "linux"))]
pub(crate) fn window_background_appearance(
    config_value: Option<&str>,
) -> WindowBackgroundAppearance {
    let preference = window_backdrop_preference(config_value);
    window_background_appearance_for_preference(preference)
}

#[cfg(not(target_os = "linux"))]
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
}

#[cfg(target_os = "windows")]
pub(crate) fn window_backdrop_uses_mica(config_value: Option<&str>) -> bool {
    matches!(
        window_background_appearance(config_value),
        WindowBackgroundAppearance::MicaBackdrop
    )
}

#[cfg(target_os = "linux")]
pub(crate) fn initial_window_background(
    _config: &paneflow_config::schema::PaneFlowConfig,
) -> WindowBackgroundAppearance {
    WindowBackgroundAppearance::Opaque
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn initial_window_background(
    config: &paneflow_config::schema::PaneFlowConfig,
) -> WindowBackgroundAppearance {
    window_background_appearance(config.window_backdrop.as_deref())
}

#[cfg(target_os = "macos")]
pub(crate) fn macos_sidebar_material_enabled(config_value: Option<&str>) -> bool {
    !matches!(
        window_backdrop_preference(config_value),
        WindowBackdropPreference::Opaque | WindowBackdropPreference::Transparent
    )
}

#[cfg(target_os = "windows")]
pub(crate) fn windows_supports_system_backdrop() -> bool {
    static SUPPORTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SUPPORTED.get_or_init(windows_build_has_system_backdrop)
}

#[cfg(target_os = "windows")]
fn windows_build_has_system_backdrop() -> bool {
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

pub(crate) fn cockpit_chrome_background(_shell: Hsla, _blended: bool) -> Hsla {
    crate::window_chrome::veil::chrome_over_shell()
}

pub(crate) fn cockpit_backdrop_background(
    background: Hsla,
    backdrop: crate::window_chrome::veil::Backdrop,
) -> Hsla {
    backdrop.over_shell(background)
}

pub(crate) fn cockpit_shell_background(
    background: Hsla,
    backdrop: crate::window_chrome::veil::Backdrop,
) -> Hsla {
    backdrop.shell(background)
}

pub(crate) fn sidebar_tab_active_background() -> Hsla {
    sidebar_tab_background(SIDEBAR_TAB_ACTIVE_OPACITY)
}

pub(crate) fn sidebar_menu_background(
    style: paneflow_config::schema::BackgroundStyle,
    ui: crate::theme::UiColors,
) -> Hsla {
    if style != paneflow_config::schema::BackgroundStyle::Themed {
        sidebar_tab_fill(
            SIDEBAR_TAB_ACTIVE_OPACITY,
            crate::window_chrome::veil::Backdrop::THEMED,
        )
    } else {
        crate::settings::components::select_menu_surface(ui)
    }
}

pub(crate) fn sidebar_filter_icon_color() -> Hsla {
    gpui::rgb(if crate::theme::active_theme().background.l > 0.5 {
        0x000000
    } else {
        0xffffff
    })
    .into()
}

pub(crate) fn sidebar_needs_input_color() -> Hsla {
    gpui::rgb(if crate::theme::active_theme().background.l > 0.5 {
        0x92400e
    } else {
        0xfbbf24
    })
    .into()
}

pub(crate) fn sidebar_finished_color() -> Hsla {
    gpui::rgb(if crate::theme::active_theme().background.l > 0.5 {
        0x0369a1
    } else {
        0x83c3ff
    })
    .into()
}

pub(crate) fn filter_match_color() -> Hsla {
    gpui::rgb(if crate::theme::active_theme().background.l > 0.5 {
        0x0550ae
    } else {
        0x5aa6ff
    })
    .into()
}

pub(crate) fn sidebar_tab_hover_background() -> Hsla {
    sidebar_tab_background(SIDEBAR_TAB_HOVER_OPACITY)
}

fn sidebar_tab_background(opacity: f32) -> Hsla {
    sidebar_tab_fill(opacity, crate::window_chrome::veil::frame_backdrop())
}

fn sidebar_tab_fill(opacity: f32, backdrop: crate::window_chrome::veil::Backdrop) -> Hsla {
    let theme = crate::theme::active_theme();
    let tint = crate::theme::ui_colors_with(&theme).text.opacity(opacity);
    backdrop.tint_over_shell(tint, theme.title_bar_background)
}

pub(crate) const TOAST_ENTER_MS: u64 = 180;
pub(crate) const TOAST_HOLD_MS: u64 = 1440;
pub(crate) const TOAST_EXIT_MS: u64 = 180;
pub(crate) const RELEASE_TOAST_DELAY_MS: u64 = 1500;
pub(crate) const UPDATE_CHECK_PILL_HOLD_MS: u64 = 3000;

pub(crate) const MAX_CLOSED_PANES: usize = 5;

pub(crate) const CLOSED_SESSION_GRACE_MS: u64 = 5_000;

pub(crate) const RESIZE_BORDER: Pixels = px(10.0);
pub(crate) const WINDOW_CORNER_RADIUS: Pixels = px(10.0);
pub(crate) const WINDOW_BORDER_SIZE: Pixels = px(1.0);

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn linux_window_starts_opaque_until_the_display_backend_is_known() {
        for json in [
            serde_json::json!({}),
            serde_json::json!({ "background_style": "blended" }),
            serde_json::json!({ "background_style": "themed" }),
            serde_json::json!({ "window_backdrop": "blurred", "linux_chrome_material": true }),
        ] {
            let config: paneflow_config::schema::PaneFlowConfig =
                serde_json::from_value(json).expect("the config deserializes");
            assert_eq!(
                initial_window_background(&config),
                WindowBackgroundAppearance::Opaque
            );
        }
    }
}

#[cfg(test)]
mod material_tests {
    use super::*;

    #[test]
    fn a_sidebar_menu_over_a_translucent_window_hides_the_rows_beneath_it() {
        let ui = crate::theme::ui_colors_with(&crate::theme::active_theme());
        let fill = sidebar_menu_background(paneflow_config::schema::BackgroundStyle::Blended, ui);

        assert_eq!(
            fill.a, 1.0,
            "a sidebar menu at alpha {} lets the sidebar rows show through, \
             since the compositor blur only reaches the desktop behind the window",
            fill.a
        );
    }

    use crate::window_chrome::veil::{Backdrop, BlurSupport};
    use paneflow_config::schema::BackgroundStyle;

    fn blended(blur: BlurSupport) -> Backdrop {
        Backdrop::new(BackgroundStyle::Blended, true, blur)
    }

    fn opaque(color: Hsla) -> Hsla {
        Hsla { a: 1.0, ..color }
    }

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

    #[test]
    fn a_veiled_shell_carries_one_veil_and_clears_the_corner_masks() {
        let dark = Hsla::from(gpui::rgb(0x141414));
        let light = Hsla::from(gpui::rgb(0xf5f7fd));
        let full = blended(BlurSupport::Full);

        assert_eq!(
            cockpit_shell_background(dark, full),
            full.shell(dark),
            "the shell veil comes from the veil module"
        );
        assert!((cockpit_shell_background(dark, full).a - 0.72).abs() < 1e-6);
        assert!((cockpit_shell_background(light, full).a - 0.80).abs() < 1e-6);
        assert_eq!(
            cockpit_backdrop_background(dark, full),
            gpui::transparent_black()
        );
        assert_eq!(
            cockpit_chrome_background(dark, true),
            gpui::transparent_black()
        );
    }

    #[test]
    fn a_shell_without_blur_thickens_its_veil() {
        let dark = Hsla::from(gpui::rgb(0x141414));
        let light = Hsla::from(gpui::rgb(0xf5f7fd));
        let none = blended(BlurSupport::None);

        assert!((cockpit_shell_background(dark, none).a - 0.93).abs() < 1e-6);
        assert!((cockpit_shell_background(light, none).a - 0.95).abs() < 1e-6);
        assert_eq!(
            cockpit_backdrop_background(dark, none),
            gpui::transparent_black()
        );
    }

    #[test]
    fn themed_fills_are_the_opaque_theme_colors() {
        let dark = gpui::hsla(0.71, 0.62, 0.32, 0.42);
        let light = gpui::hsla(0.09, 0.54, 0.78, 0.58);
        let solid = Hsla::from(gpui::rgb(0x141414));

        for background in [dark, light, solid] {
            for blur in [BlurSupport::Full, BlurSupport::None] {
                let themed = Backdrop::new(BackgroundStyle::Themed, true, blur);
                assert_eq!(
                    cockpit_shell_background(background, themed),
                    opaque(background)
                );
                assert_eq!(
                    cockpit_backdrop_background(background, themed),
                    opaque(background)
                );
            }
            assert_eq!(
                cockpit_chrome_background(background, false),
                gpui::transparent_black(),
                "themed chrome shows the shell, which carries the inactive window color"
            );
        }
        assert_eq!(cockpit_shell_background(solid, Backdrop::THEMED), solid);
    }

    #[test]
    fn an_invalid_background_style_renders_every_fill_themed() {
        let config: paneflow_config::schema::PaneFlowConfig = serde_json::from_value(
            serde_json::json!({ "background_style": "mica", "linux_chrome_material": true }),
        )
        .expect("an invalid background_style still loads");
        let solid = Hsla::from(gpui::rgb(0x141414));

        for blur in [BlurSupport::Full, BlurSupport::None] {
            let backdrop = Backdrop::new(config.resolved_background_style(), true, blur);
            assert!(!backdrop.is_blended());
            assert_eq!(cockpit_shell_background(solid, backdrop), solid);
            assert_eq!(cockpit_backdrop_background(solid, backdrop), solid);
            assert_eq!(backdrop.terminal_card(solid), solid);
        }
    }

    #[test]
    fn a_blur_support_change_reaches_the_next_frame_veil() {
        let dark = Hsla::from(gpui::rgb(0x141414));
        let before = blended(BlurSupport::None);
        let after = blended(BlurSupport::Full);

        crate::window_chrome::veil::note_frame_backdrop(before);
        assert_eq!(crate::window_chrome::veil::frame_backdrop(), before);
        assert!((cockpit_shell_background(dark, before).a - 0.93).abs() < 1e-6);
        crate::window_chrome::veil::note_frame_backdrop(after);
        assert_eq!(crate::window_chrome::veil::frame_backdrop(), after);
        assert!((cockpit_shell_background(dark, after).a - 0.72).abs() < 1e-6);
        crate::window_chrome::veil::note_frame_backdrop(Backdrop::THEMED);
    }

    #[test]
    fn sidebar_tabs_preblend_their_tint_only_over_an_opaque_shell() {
        let theme = crate::theme::active_theme();
        let tint = crate::theme::ui_colors_with(&theme)
            .text
            .opacity(SIDEBAR_TAB_ACTIVE_OPACITY);

        assert_eq!(
            sidebar_tab_fill(SIDEBAR_TAB_ACTIVE_OPACITY, blended(BlurSupport::Full)),
            tint
        );
        assert_eq!(
            sidebar_tab_fill(SIDEBAR_TAB_ACTIVE_OPACITY, Backdrop::THEMED),
            opaque(theme.title_bar_background).blend(tint)
        );
    }
}
