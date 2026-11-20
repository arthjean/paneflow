#[cfg(target_os = "windows")]
pub mod backdrop;
pub mod csd;
#[cfg(target_os = "linux")]
pub mod linux_backdrop;
#[cfg(target_os = "macos")]
pub mod macos_backdrop;
pub(crate) mod title_band;
pub mod title_bar;
pub(crate) mod veil;

use gpui::{IntoElement, PathBuilder, Styled, canvas, point};

const DEV_BLUR_SUPPORT_ENV: &str = "PANEFLOW_DEV_BLUR_SUPPORT";

pub(crate) fn window_backdrop(
    config: &paneflow_config::schema::PaneFlowConfig,
    window: &gpui::Window,
) -> veil::Backdrop {
    style_backdrop(config.resolved_background_style(), window)
}

pub(crate) fn style_backdrop(
    style: paneflow_config::schema::BackgroundStyle,
    window: &gpui::Window,
) -> veil::Backdrop {
    veil::Backdrop::new(style, surface_has_alpha(window), blur_support(window))
}

fn surface_has_alpha(window: &gpui::Window) -> bool {
    !fullscreen_drops_surface_alpha(window.is_fullscreen()) && system_backdrop_available()
}

fn fullscreen_drops_surface_alpha(is_fullscreen: bool) -> bool {
    cfg!(target_os = "macos") && is_fullscreen
}

#[cfg(target_os = "windows")]
fn system_backdrop_available() -> bool {
    crate::app::constants::windows_supports_system_backdrop()
}

#[cfg(not(target_os = "windows"))]
fn system_backdrop_available() -> bool {
    true
}

fn blur_support(window: &gpui::Window) -> veil::BlurSupport {
    dev_blur_support_override().unwrap_or_else(|| detected_blur_support(window))
}

#[cfg(target_os = "linux")]
fn detected_blur_support(window: &gpui::Window) -> veil::BlurSupport {
    linux_backdrop::blur_support(window)
}

#[cfg(not(target_os = "linux"))]
fn detected_blur_support(_window: &gpui::Window) -> veil::BlurSupport {
    veil::BlurSupport::Full
}

fn dev_blur_support_override() -> Option<veil::BlurSupport> {
    static OVERRIDE: std::sync::OnceLock<Option<veil::BlurSupport>> = std::sync::OnceLock::new();
    *OVERRIDE.get_or_init(|| {
        dev_blur_support_for(
            cfg!(debug_assertions),
            std::env::var(DEV_BLUR_SUPPORT_ENV).ok().as_deref(),
        )
    })
}

fn dev_blur_support_for(debug_build: bool, value: Option<&str>) -> Option<veil::BlurSupport> {
    if !debug_build {
        return None;
    }
    let value = value?.trim();
    if value.eq_ignore_ascii_case("full") {
        Some(veil::BlurSupport::Full)
    } else if value.eq_ignore_ascii_case("none") {
        Some(veil::BlurSupport::None)
    } else {
        log::warn!(
            "{DEV_BLUR_SUPPORT_ENV}: unknown value '{value}', expected 'full' or 'none'; \
             using the detected blur support"
        );
        None
    }
}

#[derive(Clone, Copy)]
pub(crate) enum PanelCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

pub(crate) fn panel_corner_mask(corner: PanelCorner, background: gpui::Hsla) -> impl IntoElement {
    const KAPPA: f32 = 0.552_284_8;

    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let left = bounds.left();
            let right = bounds.right();
            let top = bounds.top();
            let bottom = bounds.bottom();
            let radius = bounds.size.width.min(bounds.size.height);
            let k = radius * KAPPA;

            let mut builder = PathBuilder::fill();
            match corner {
                PanelCorner::TopLeft => {
                    builder.move_to(point(left, top));
                    builder.line_to(point(right, top));
                    builder.cubic_bezier_to(
                        point(left, bottom),
                        point(right - k, top),
                        point(left, bottom - k),
                    );
                    builder.line_to(point(left, top));
                }
                PanelCorner::TopRight => {
                    builder.move_to(point(left, top));
                    builder.line_to(point(right, top));
                    builder.line_to(point(right, bottom));
                    builder.cubic_bezier_to(
                        point(left, top),
                        point(right, bottom - k),
                        point(left + k, top),
                    );
                }
                PanelCorner::BottomLeft => {
                    builder.move_to(point(left, bottom));
                    builder.line_to(point(right, bottom));
                    builder.cubic_bezier_to(
                        point(left, top),
                        point(right - k, bottom),
                        point(left, top + k),
                    );
                    builder.line_to(point(left, bottom));
                }
                PanelCorner::BottomRight => {
                    builder.move_to(point(left, bottom));
                    builder.line_to(point(right, bottom));
                    builder.line_to(point(right, top));
                    builder.cubic_bezier_to(
                        point(left, bottom),
                        point(right, top + k),
                        point(left + k, bottom),
                    );
                }
            }
            builder.close();

            if let Ok(path) = builder.build() {
                window.paint_path(path, background);
            }
        },
    )
    .size_full()
}

#[cfg(test)]
mod backdrop_source_tests {
    use super::{dev_blur_support_for, fullscreen_drops_surface_alpha, veil::BlurSupport};

    #[test]
    fn fullscreen_drops_the_surface_alpha_on_macos_only() {
        assert!(!fullscreen_drops_surface_alpha(false));
        assert_eq!(
            fullscreen_drops_surface_alpha(true),
            cfg!(target_os = "macos")
        );
    }

    #[test]
    fn a_debug_build_forces_the_blur_support_from_the_variable() {
        assert_eq!(
            dev_blur_support_for(true, Some("none")),
            Some(BlurSupport::None)
        );
        assert_eq!(
            dev_blur_support_for(true, Some(" Full ")),
            Some(BlurSupport::Full)
        );
        assert_eq!(dev_blur_support_for(true, None), None);
    }

    #[test]
    fn an_unknown_value_falls_back_to_the_detected_blur_support() {
        assert_eq!(dev_blur_support_for(true, Some("partial")), None);
        assert_eq!(dev_blur_support_for(true, Some("")), None);
    }

    #[test]
    fn a_release_build_ignores_the_variable() {
        for value in ["none", "full", "partial"] {
            assert_eq!(dev_blur_support_for(false, Some(value)), None);
        }
    }
}
