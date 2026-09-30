use gpui::{Decorations, Window, WindowBackgroundAppearance};
use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use std::sync::atomic::{AtomicBool, Ordering};

static TRANSLUCENT_WINDOW: AtomicBool = AtomicBool::new(false);

pub(crate) fn translucent_window_active() -> bool {
    TRANSLUCENT_WINDOW.load(Ordering::Relaxed)
}

fn unblurred_surface_appearance(
    translucent: bool,
    client_decorated: bool,
    explicit_alpha_required: bool,
) -> WindowBackgroundAppearance {
    if translucent || (client_decorated && explicit_alpha_required) {
        WindowBackgroundAppearance::Transparent
    } else {
        WindowBackgroundAppearance::Opaque
    }
}

fn unblurred_window_appearance(window: &Window, translucent: bool) -> WindowBackgroundAppearance {
    let client_decorated = matches!(window.window_decorations(), Decorations::Client { .. });
    let explicit_alpha_required = HasDisplayHandle::display_handle(window).is_ok_and(|handle| {
        matches!(
            handle.as_raw(),
            RawDisplayHandle::Xcb(_) | RawDisplayHandle::Xlib(_)
        )
    });
    unblurred_surface_appearance(translucent, client_decorated, explicit_alpha_required)
}

pub(crate) fn apply_subtle_chrome_material(window: &mut Window, translucent: bool) {
    refresh_blur_region(window, translucent);
    window.refresh();
}

pub(crate) fn refresh_blur_region(window: &mut Window, translucent: bool) {
    TRANSLUCENT_WINDOW.store(translucent, Ordering::Relaxed);
    window.set_background_appearance(unblurred_window_appearance(window, translucent));
}

#[cfg(test)]
mod tests {
    use super::unblurred_surface_appearance;
    use gpui::WindowBackgroundAppearance;

    #[test]
    fn only_x11_client_surfaces_require_explicit_alpha() {
        assert_eq!(
            unblurred_surface_appearance(false, true, true),
            WindowBackgroundAppearance::Transparent
        );
        assert_eq!(
            unblurred_surface_appearance(false, true, false),
            WindowBackgroundAppearance::Opaque
        );
        assert_eq!(
            unblurred_surface_appearance(false, false, true),
            WindowBackgroundAppearance::Opaque
        );
    }

    #[test]
    fn translucent_chrome_requests_alpha_on_every_surface() {
        for (client_decorated, explicit_alpha_required) in
            [(false, false), (false, true), (true, false), (true, true)]
        {
            assert_eq!(
                unblurred_surface_appearance(true, client_decorated, explicit_alpha_required),
                WindowBackgroundAppearance::Transparent
            );
        }
    }
}
