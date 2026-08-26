//! Layout & timing constants shared across the app shell.
//!
//! Extracted from `main.rs` per US-002 (anti edit-thrashing). All items
//! are `pub(crate)` and re-exported at the crate root via `main.rs` so
//! existing `crate::SIDEBAR_WIDTH` / `crate::TOAST_HOLD_MS` references in
//! sibling modules keep compiling without import churn.

use gpui::{Hsla, Pixels, WindowBackgroundAppearance, px};

/// Sidebar width in pixels - shared between sidebar and title bar for alignment.
///
/// Sized so a tab row keeps at least the title budget of a sidebar row
/// once the row margins, the folder lead, and the 48px agent-status slot are
/// taken out. It also lines up with the files rail, which already sits at 300.
pub(crate) const SIDEBAR_WIDTH: f32 = 300.;
/// Outer title-bar inset aligned with workspace rows and the sidebar footer.
pub(crate) const TITLE_BAR_EDGE_INSET: Pixels = px(8.);
/// Inter-button rhythm for compact title-bar controls.
pub(crate) const TITLE_BAR_CONTROL_SPACING: Pixels = px(12.);
/// Compact custom control size used by Linux CSD title bars.
pub(crate) const TITLE_BAR_CONTROL_SIZE: Pixels = px(20.);
/// Minimum title-bar height: the native Win11 caption strip is exactly 32px,
/// and the Windows control cluster paints at full bar height (46x`height`), so
/// anything taller reads as an oversized caption there. It still leaves a 6px
/// inset around the 20px compact controls Linux and the brand cluster use.
pub(crate) const TITLE_BAR_MIN_HEIGHT: Pixels = px(32.);
/// Inset between the window shell and the main panel.
pub(crate) const PANEL_INSET: f32 = 4.;
/// The main panel's structural corner language.
pub(crate) const PANEL_CORNER_RADIUS: Pixels = WINDOW_CORNER_RADIUS;
/// Corner radius of a single CLI pane card.
///
/// The card draws its corners as a superellipse rather than a circular arc
/// (see [`crate::ui_primitives::squircle`]), and the two are not interchangeable
/// at equal radius: a superellipse hugs the corner far more tightly at its
/// midpoint, so it reads as noticeably less rounded than a circle of the same
/// value. The radius is therefore the distance the corner runs *along the
/// edge*, not its depth, and it has to be roughly 1.5x a circular radius to
/// land on the same apparent roundness. That is what buys the softer
/// silhouette: a long, gentle departure from the edge instead of a short arc.
///
/// This is larger than [`PANEL_CORNER_RADIUS`] by design - the cards are the
/// foreground objects, and it is their corner the eye reads, not the shell's.
pub(crate) const PANE_CARD_RADIUS: Pixels = px(20.);
/// Inner CLI content inset, placing the header row and the terminal cells this
/// far from the pane card's edges. Mirrors Ghostty's `window-padding-x` /
/// `window-padding-y` (`src/renderer/size.zig`): an explicit padding subtracted
/// from the viewport *before* the grid is sized, applied to both edges of each
/// axis, so the cells never hug the card - on any side. The card's hairline is
/// painted as a path rather than reserved as a border, so it costs no layout
/// and is not folded in here.
pub(crate) const PANE_CONTENT_INSET_X: f32 = 10.;
/// Vertical half of [`PANE_CONTENT_INSET_X`]. Ghostty splits the two axes for
/// the same reason: a line box already carries leading, so the horizontal gap
/// has to be the larger of the two to read as symmetric.
pub(crate) const PANE_CONTENT_INSET_Y: f32 = 6.;
/// Selected rows carry a stronger lift than hover rows so current navigation
/// remains legible without a separate indicator. Linux precomposes these tints
/// over the opaque chrome color; macOS and Windows keep their native material.
const DARK_SIDEBAR_TAB_TINT: u32 = 0xffffff;
const LIGHT_SIDEBAR_TAB_TINT: u32 = 0x262626;
const DARK_SIDEBAR_TAB_ACTIVE_OPACITY: f32 = 0.11;
const DARK_SIDEBAR_TAB_HOVER_OPACITY: f32 = 0.07;
const LIGHT_SIDEBAR_TAB_ACTIVE_OPACITY: f32 = 0.08;
const LIGHT_SIDEBAR_TAB_HOVER_OPACITY: f32 = 0.04;
const SIDEBAR_TAB_ICON_CARD_TINT: u32 = 0x000000;
/// How much darker than the selected tab card the pane-icon card sits. Small
/// on purpose: the card reads as the same material as the row it lives on,
/// one step down, not as a second color.
const DARK_SIDEBAR_TAB_ICON_CARD_DARKEN: f32 = 0.10;
const LIGHT_SIDEBAR_TAB_ICON_CARD_DARKEN: f32 = 0.05;

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

/// Fill used by chrome children inside the window shell.
///
/// Windows must paint an opaque title bar when its dedicated chrome material is
/// disabled. The terminal can still activate the host-window backdrop, so a
/// transparent title bar would otherwise expose that terminal-only material.
/// Other platforms keep the child transparent and let the rounded shell own the
/// tint, avoiding rectangular paint outside GPUI's corner mask.
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

/// Background for the selected tab in the CLI sidebar.
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

/// US-013: fill of one pane-icon card in a tab row's stacked cluster.
///
/// Opaque on every platform, unlike the row backgrounds above: the cards
/// overlap, and a translucent fill doubles up in the seam and lets the card
/// underneath show through its own right edge. Blending a tint onto an opaque
/// title-bar background is the recipe `sidebar_tab_background` already uses on
/// Linux, applied unconditionally here because a 15x18 card is far too small
/// for the macOS/Windows material to read through anyway.
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
    // Same material as the selected tab card, one step darker: the card is the
    // row's own surface pushed back, not a color of its own. The tab tint is
    // composed here rather than reused from `sidebar_tab_background` because
    // that one stays translucent off Linux, and this card must be opaque.
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
