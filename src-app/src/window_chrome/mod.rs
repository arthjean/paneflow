#[cfg(target_os = "windows")]
pub mod backdrop;
pub mod csd;
#[cfg(target_os = "linux")]
pub mod linux_backdrop;
#[cfg(target_os = "macos")]
pub mod macos_backdrop;
pub mod title_bar;

use gpui::{IntoElement, PathBuilder, Styled, canvas, point};

pub(crate) fn native_backdrop_material_active(
    settings_open: bool,
    terminal_material_active: bool,
    chrome_material_active: bool,
) -> bool {
    chrome_material_active || (!settings_open && terminal_material_active)
}

pub(crate) fn native_material_suppressed_by_fullscreen(is_fullscreen: bool) -> bool {
    cfg!(target_os = "macos") && is_fullscreen
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
mod native_material_tests {
    use super::{native_backdrop_material_active, native_material_suppressed_by_fullscreen};

    #[test]
    fn fullscreen_suppresses_the_native_material_on_macos_only() {
        assert!(!native_material_suppressed_by_fullscreen(false));
        assert_eq!(
            native_material_suppressed_by_fullscreen(true),
            cfg!(target_os = "macos")
        );
    }

    #[test]
    fn terminal_material_can_activate_backdrop_without_chrome_material() {
        assert!(native_backdrop_material_active(false, true, false));
    }

    #[test]
    fn terminal_material_only_applies_to_a_visible_terminal() {
        assert!(!native_backdrop_material_active(true, true, false));
    }

    #[test]
    fn chrome_material_activates_backdrop_independently() {
        assert!(native_backdrop_material_active(true, false, true));
    }
}
