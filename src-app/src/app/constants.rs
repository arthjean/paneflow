//! Layout & timing constants shared across the app shell.
//!
//! Extracted from `main.rs` per US-002 (anti edit-thrashing). All items
//! are `pub(crate)` and re-exported at the crate root via `main.rs` so
//! existing `crate::SIDEBAR_WIDTH` / `crate::TOAST_HOLD_MS` references in
//! sibling modules keep compiling without import churn.

use gpui::{Hsla, Pixels, WindowBackgroundAppearance, px};

/// Sidebar width in pixels - shared between sidebar and title bar for alignment.
pub(crate) const SIDEBAR_WIDTH: f32 = 240.;
/// Outer title-bar inset aligned with workspace rows and the sidebar footer.
pub(crate) const TITLE_BAR_EDGE_INSET: Pixels = px(8.);
/// Inter-button rhythm for compact title-bar controls.
pub(crate) const TITLE_BAR_CONTROL_SPACING: Pixels = px(12.);
/// Compact custom control size used by Linux CSD title bars.
pub(crate) const TITLE_BAR_CONTROL_SIZE: Pixels = px(20.);
/// Minimum title-bar height preserving an 8px inset around compact controls.
pub(crate) const TITLE_BAR_MIN_HEIGHT: Pixels = px(36.);
/// Inset between the window shell and the primary navigation card.
pub(crate) const SIDEBAR_CARD_INSET: f32 = 4.;
/// The primary navigation rails share the main panel's structural corner language.
pub(crate) const SIDEBAR_CARD_CORNER_RADIUS: Pixels = WINDOW_CORNER_RADIUS;
/// Inner CLI content inset. Combined with the pane's reserved 1px border,
/// this places tabs and terminal cells 4px from the main panel edge.
pub(crate) const PANE_CONTENT_INSET: f32 = 3.;
/// Windows Mica and macOS Sidebar material already supply theme-aware tints.
/// The card remains fully transparent there so the material stays perceptible;
/// its border still defines the inset surface.
#[cfg(any(target_os = "windows", target_os = "macos"))]
const SIDEBAR_CARD_MATERIAL_OPACITY: f32 = 0.;
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const SIDEBAR_CARD_MATERIAL_OPACITY: f32 = 0.84;

/// Selected rows carry a stronger lift than hover rows so current navigation
/// remains legible without a separate indicator. Linux precomposes these tints
/// over the opaque chrome color; macOS and Windows keep their native material.
const DARK_SIDEBAR_TAB_TINT: u32 = 0xffffff;
const LIGHT_SIDEBAR_TAB_TINT: u32 = 0x25262b;
const DARK_SIDEBAR_TAB_ACTIVE_OPACITY: f32 = 0.11;
const DARK_SIDEBAR_TAB_HOVER_OPACITY: f32 = 0.07;
const LIGHT_SIDEBAR_TAB_ACTIVE_OPACITY: f32 = 0.08;
const LIGHT_SIDEBAR_TAB_HOVER_OPACITY: f32 = 0.04;

/// Shared radius for the Agents search field and its primary navigation rows.
pub(crate) const SIDEBAR_TAB_CORNER_RADIUS: Pixels = px(8.);

/// Native material used behind the main application window.
///
/// Windows delegates to GPUI's system backdrop support. On macOS PaneFlow
/// installs a semantic AppKit sidebar material after the native window opens.
/// Linux starts opaque. Once the native handle exists, the Linux window layer
/// enables explicit alpha only for X11 CSD; Wayland CSD is alpha-capable by
/// construction and can keep opaque text rendering semantics.
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

    // NTSTATUS values greater than or equal to zero indicate success.
    unsafe { RtlGetVersion(&mut version) >= 0 && version.build >= 22_621 }
}

/// Transparent fill used by chrome children inside the window shell.
///
/// The rounded shell owns the actual theme tint. Keeping title-adjacent rails
/// transparent prevents their rectangular backgrounds from repainting pixels
/// outside the shell's corner radius, since GPUI overflow masks are rectangular.
pub(crate) fn cockpit_chrome_background(
    _background: Hsla,
    _is_window_active: bool,
    _material_active: bool,
) -> Hsla {
    gpui::transparent_black()
}

/// Fill for the inset primary navigation card.
///
/// Linux uses a fully opaque surface. Windows and macOS expose their raw native
/// materials, with the same opaque fallback when the material is off.
pub(crate) fn primary_sidebar_card_background(surface: Hsla, material_active: bool) -> Hsla {
    let opaque_surface = Hsla { a: 1.0, ..surface };
    if cfg!(target_os = "linux") || !material_active {
        opaque_surface
    } else {
        opaque_surface.opacity(SIDEBAR_CARD_MATERIAL_OPACITY)
    }
}

/// Window-level backdrop behind the application chrome.
///
/// This is what the rounded panel corners reveal in their clip notch, so it MUST
/// show through the transparent rail ([`cockpit_chrome_background`]) - otherwise
/// the corner exposes a different surface and the radius reads as a square patch.
/// Native semantic materials remain raw on macOS and Windows. Linux always
/// resolves to the opaque theme color: wallpaper never participates in the UI.
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

/// Background for the selected tab in the CLI and Agents sidebars.
pub(crate) fn sidebar_tab_active_background() -> Hsla {
    sidebar_tab_background(
        LIGHT_SIDEBAR_TAB_ACTIVE_OPACITY,
        DARK_SIDEBAR_TAB_ACTIVE_OPACITY,
    )
}

/// Background for a hovered, non-selected sidebar tab.
pub(crate) fn sidebar_tab_hover_background() -> Hsla {
    sidebar_tab_background(
        LIGHT_SIDEBAR_TAB_HOVER_OPACITY,
        DARK_SIDEBAR_TAB_HOVER_OPACITY,
    )
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

/// Toast animation durations (ms). The `hold_ms` carried on each `Toast`
/// must match the dismiss timer in `push_toast` - otherwise the exit
/// animation plays early and the element persists as a ghost.
pub(crate) const TOAST_ENTER_MS: u64 = 180;
pub(crate) const TOAST_HOLD_MS: u64 = 1440;
pub(crate) const TOAST_EXIT_MS: u64 = 180;

/// Maximum number of closed-pane records kept for undo-close-pane (US-014).
pub(crate) const MAX_CLOSED_PANES: usize = 5;

/// EP-003: cumulative text budget for undo-close captured scrollback.
pub(crate) const MAX_CLOSED_PANE_SCROLLBACK_BYTES: usize = 2 * 1024 * 1024;

/// Width of the invisible border zone used for CSD edge/corner resize handles.
pub(crate) const RESIZE_BORDER: Pixels = px(10.0);
/// Radius of the visible application shell inside the transparent CSD shadow.
pub(crate) const WINDOW_CORNER_RADIUS: Pixels = px(10.0);
/// Hairline separating the themed shell from its native compositor shadow.
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

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[test]
    fn native_sidebar_card_exposes_raw_material() {
        let surface = Hsla::from(gpui::rgb(0x212122));
        let card = primary_sidebar_card_background(surface, true);

        assert_eq!(card.a, 0., "the sidebar must not veil native material");
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
        assert_eq!(primary_sidebar_card_background(dark, true).a, 1.0);
    }
}
