use gpui::{App, AppContext, Decorations, Window, WindowBackgroundAppearance};
use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::ext::background_effect::v1::client::ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1;

const KDE_BLUR_MANAGER_INTERFACE: &str = "org_kde_kwin_blur_manager";

static TRANSLUCENT_WINDOW: AtomicBool = AtomicBool::new(false);
static BLUR_REQUESTED: AtomicBool = AtomicBool::new(false);
static BLUR_FALLBACK_LOGGED: AtomicBool = AtomicBool::new(false);
static COMPOSITOR_BLUR: OnceLock<CompositorBlur> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SurfaceKind {
    Wayland,
    X11,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompositorBlur {
    Pending,
    Offered,
    Missing,
}

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

fn surface_appearance(
    translucent: bool,
    client_decorated: bool,
    surface: SurfaceKind,
    blur_requested: bool,
    blur: CompositorBlur,
) -> WindowBackgroundAppearance {
    if translucent
        && blur_requested
        && surface == SurfaceKind::Wayland
        && blur == CompositorBlur::Offered
    {
        return WindowBackgroundAppearance::Blurred;
    }
    unblurred_surface_appearance(translucent, client_decorated, surface == SurfaceKind::X11)
}

fn blur_falls_back(surface: SurfaceKind, blur_requested: bool, blur: CompositorBlur) -> bool {
    blur_requested
        && match surface {
            SurfaceKind::Wayland => blur == CompositorBlur::Missing,
            SurfaceKind::X11 | SurfaceKind::Other => true,
        }
}

fn first_blur_fallback() -> bool {
    !BLUR_FALLBACK_LOGGED.swap(true, Ordering::Relaxed)
}

fn compositor_offers_blur<'a>(mut interfaces: impl Iterator<Item = &'a str>) -> bool {
    let effect = ExtBackgroundEffectManagerV1::interface().name;
    interfaces.any(|interface| interface == effect || interface == KDE_BLUR_MANAGER_INTERFACE)
}

fn surface_kind(window: &Window) -> SurfaceKind {
    match HasDisplayHandle::display_handle(window).map(|handle| handle.as_raw()) {
        Ok(RawDisplayHandle::Wayland(_)) => SurfaceKind::Wayland,
        Ok(RawDisplayHandle::Xcb(_) | RawDisplayHandle::Xlib(_)) => SurfaceKind::X11,
        _ => SurfaceKind::Other,
    }
}

fn compositor_blur() -> CompositorBlur {
    COMPOSITOR_BLUR
        .get()
        .copied()
        .unwrap_or(CompositorBlur::Pending)
}

fn window_appearance(window: &Window, translucent: bool) -> WindowBackgroundAppearance {
    let client_decorated = matches!(
        crate::window_chrome::csd::decorations(window),
        Decorations::Client { .. }
    );
    let surface = surface_kind(window);
    let blur_requested = BLUR_REQUESTED.load(Ordering::Relaxed);
    let blur = compositor_blur();
    if blur_falls_back(surface, blur_requested, blur) && first_blur_fallback() {
        log::warn!(
            "window_backdrop: 'blurred' needs a Wayland compositor with ext-background-effect-v1 \
             or KDE blur; this session offers neither, so the window keeps its {} appearance",
            if translucent { "translucent" } else { "opaque" }
        );
    }
    surface_appearance(translucent, client_decorated, surface, blur_requested, blur)
}

struct BlurProbe;

impl Dispatch<WlRegistry, GlobalListContents> for BlurProbe {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

fn probe_compositor_blur() -> CompositorBlur {
    let globals = Connection::connect_to_env()
        .map_err(|error| error.to_string())
        .and_then(|connection| {
            registry_queue_init::<BlurProbe>(&connection)
                .map(|(globals, _)| globals)
                .map_err(|error| error.to_string())
        });
    match globals {
        Ok(globals) => {
            let offered = globals.contents().with_list(|list| {
                compositor_offers_blur(list.iter().map(|global| global.interface.as_str()))
            });
            if offered {
                CompositorBlur::Offered
            } else {
                CompositorBlur::Missing
            }
        }
        Err(error) => {
            log::warn!("window_backdrop: could not list the compositor's globals: {error}");
            CompositorBlur::Missing
        }
    }
}

fn request_compositor_blur(window: &Window, cx: &mut App) {
    BLUR_REQUESTED.store(true, Ordering::Relaxed);
    if surface_kind(window) != SurfaceKind::Wayland || COMPOSITOR_BLUR.get().is_some() {
        return;
    }
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        let blur = cx
            .background_spawn(async move { probe_compositor_blur() })
            .await;
        let _ = COMPOSITOR_BLUR.set(blur);
        let _ = handle.update(cx, |_, window, _| window.refresh());
    })
    .detach();
}

pub(crate) fn apply_subtle_chrome_material(
    window: &mut Window,
    translucent: bool,
    blur_requested: bool,
    cx: &mut App,
) {
    if blur_requested {
        request_compositor_blur(window, cx);
    }
    refresh_blur_region(window, translucent);
    window.refresh();
}

pub(crate) fn refresh_blur_region(window: &mut Window, translucent: bool) {
    TRANSLUCENT_WINDOW.store(translucent, Ordering::Relaxed);
    window.set_background_appearance(window_appearance(window, translucent));
}

#[cfg(test)]
mod tests {
    use super::{
        CompositorBlur, SurfaceKind, blur_falls_back, compositor_offers_blur, first_blur_fallback,
        surface_appearance, unblurred_surface_appearance,
    };
    use gpui::WindowBackgroundAppearance;

    const SURFACES: [SurfaceKind; 3] = [SurfaceKind::Wayland, SurfaceKind::X11, SurfaceKind::Other];
    const BLUR_STATES: [CompositorBlur; 3] = [
        CompositorBlur::Pending,
        CompositorBlur::Offered,
        CompositorBlur::Missing,
    ];

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

    #[test]
    fn a_blurred_setting_requests_blur_where_the_compositor_offers_it() {
        for client_decorated in [false, true] {
            assert_eq!(
                surface_appearance(
                    true,
                    client_decorated,
                    SurfaceKind::Wayland,
                    true,
                    CompositorBlur::Offered
                ),
                WindowBackgroundAppearance::Blurred
            );
        }
    }

    #[test]
    fn an_opaque_window_asks_for_no_blur_behind_it() {
        for client_decorated in [false, true] {
            assert_eq!(
                surface_appearance(
                    false,
                    client_decorated,
                    SurfaceKind::Wayland,
                    true,
                    CompositorBlur::Offered
                ),
                unblurred_surface_appearance(false, client_decorated, false)
            );
        }
    }

    #[test]
    fn a_wayland_compositor_without_blur_keeps_the_unblurred_appearance_and_logs_it() {
        for blur in [CompositorBlur::Pending, CompositorBlur::Missing] {
            for translucent in [false, true] {
                for client_decorated in [false, true] {
                    assert_eq!(
                        surface_appearance(
                            translucent,
                            client_decorated,
                            SurfaceKind::Wayland,
                            true,
                            blur
                        ),
                        unblurred_surface_appearance(translucent, client_decorated, false)
                    );
                }
            }
        }
        assert!(blur_falls_back(
            SurfaceKind::Wayland,
            true,
            CompositorBlur::Missing
        ));
        assert!(!blur_falls_back(
            SurfaceKind::Wayland,
            true,
            CompositorBlur::Pending
        ));
        assert!(!blur_falls_back(
            SurfaceKind::Wayland,
            true,
            CompositorBlur::Offered
        ));
    }

    #[test]
    fn the_blur_fallback_is_logged_once() {
        assert!(first_blur_fallback());
        assert!(!first_blur_fallback());
        assert!(!first_blur_fallback());
    }

    #[test]
    fn x11_keeps_its_appearance_whatever_the_blur_setting() {
        for blur in BLUR_STATES {
            for translucent in [false, true] {
                for client_decorated in [false, true] {
                    assert_eq!(
                        surface_appearance(
                            translucent,
                            client_decorated,
                            SurfaceKind::X11,
                            true,
                            blur
                        ),
                        unblurred_surface_appearance(translucent, client_decorated, true)
                    );
                }
            }
        }
    }

    #[test]
    fn without_the_blurred_setting_no_surface_requests_blur() {
        for surface in SURFACES {
            for blur in BLUR_STATES {
                for translucent in [false, true] {
                    assert_ne!(
                        surface_appearance(translucent, true, surface, false, blur),
                        WindowBackgroundAppearance::Blurred
                    );
                    assert!(!blur_falls_back(surface, false, blur));
                }
            }
        }
    }

    #[test]
    fn either_blur_protocol_counts_as_offered() {
        assert!(compositor_offers_blur(
            ["wl_compositor", "ext_background_effect_manager_v1"].into_iter()
        ));
        assert!(compositor_offers_blur(
            ["wl_compositor", "org_kde_kwin_blur_manager"].into_iter()
        ));
        assert!(!compositor_offers_blur(
            ["wl_compositor", "xdg_wm_base", "wp_viewporter"].into_iter()
        ));
    }
}
