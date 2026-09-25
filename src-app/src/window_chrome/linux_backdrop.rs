use gpui::{Decorations, Window, WindowBackgroundAppearance};
use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};

fn unblurred_surface_appearance(
    client_decorated: bool,
    explicit_alpha_required: bool,
) -> WindowBackgroundAppearance {
    if client_decorated && explicit_alpha_required {
        WindowBackgroundAppearance::Transparent
    } else {
        WindowBackgroundAppearance::Opaque
    }
}

fn unblurred_window_appearance(window: &Window) -> WindowBackgroundAppearance {
    let client_decorated = matches!(window.window_decorations(), Decorations::Client { .. });
    let explicit_alpha_required = HasDisplayHandle::display_handle(window).is_ok_and(|handle| {
        matches!(
            handle.as_raw(),
            RawDisplayHandle::Xcb(_) | RawDisplayHandle::Xlib(_)
        )
    });
    unblurred_surface_appearance(client_decorated, explicit_alpha_required)
}

pub(crate) fn apply_subtle_chrome_material(window: &mut Window) {
    window.set_background_appearance(unblurred_window_appearance(window));
    window.refresh();
}

pub(crate) fn refresh_blur_region(window: &mut Window) {
    window.set_background_appearance(unblurred_window_appearance(window));
}

#[cfg(test)]
mod tests {
    use super::unblurred_surface_appearance;
    use gpui::WindowBackgroundAppearance;

    #[test]
    fn only_x11_client_surfaces_require_explicit_alpha() {
        assert_eq!(
            unblurred_surface_appearance(true, true),
            WindowBackgroundAppearance::Transparent
        );
        assert_eq!(
            unblurred_surface_appearance(true, false),
            WindowBackgroundAppearance::Opaque
        );
        assert_eq!(
            unblurred_surface_appearance(false, true),
            WindowBackgroundAppearance::Opaque
        );
    }
}
