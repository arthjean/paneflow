use gpui::{App, AppContext, Decorations, Window, WindowBackgroundAppearance};
use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::ext::background_effect::v1::client::ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1;

use super::veil::BlurSupport;

const KDE_BLUR_MANAGER_INTERFACE: &str = "org_kde_kwin_blur_manager";

static BLUR_PROBE_STARTED: AtomicBool = AtomicBool::new(false);
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
    Unreachable,
}

fn unblurred_surface_appearance(
    blended: bool,
    client_decorated: bool,
    explicit_alpha_required: bool,
) -> WindowBackgroundAppearance {
    if blended || (client_decorated && explicit_alpha_required) {
        WindowBackgroundAppearance::Transparent
    } else {
        WindowBackgroundAppearance::Opaque
    }
}

fn surface_appearance(
    blended: bool,
    client_decorated: bool,
    surface: SurfaceKind,
    blur: CompositorBlur,
) -> WindowBackgroundAppearance {
    if blended && blur_support_for(surface, blur) == BlurSupport::Full {
        return WindowBackgroundAppearance::Blurred;
    }
    unblurred_surface_appearance(blended, client_decorated, surface == SurfaceKind::X11)
}

fn blur_support_for(surface: SurfaceKind, blur: CompositorBlur) -> BlurSupport {
    if surface == SurfaceKind::Wayland && blur == CompositorBlur::Offered {
        BlurSupport::Full
    } else {
        BlurSupport::None
    }
}

pub(crate) fn blur_support(window: &Window) -> BlurSupport {
    blur_support_for(surface_kind(window), compositor_blur())
}

fn blur_falls_back(blended: bool, surface: SurfaceKind, blur: CompositorBlur) -> bool {
    blended
        && match surface {
            SurfaceKind::Wayland => blur == CompositorBlur::Missing,
            SurfaceKind::X11 | SurfaceKind::Other => true,
        }
}

fn needs_blur_probe(blended: bool, surface: SurfaceKind, blur: CompositorBlur) -> bool {
    blended && surface == SurfaceKind::Wayland && blur == CompositorBlur::Pending
}

fn first_blur_fallback() -> bool {
    !BLUR_FALLBACK_LOGGED.swap(true, Ordering::Relaxed)
}

fn first_blur_probe() -> bool {
    !BLUR_PROBE_STARTED.swap(true, Ordering::Relaxed)
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

fn fallback_surface_name(surface: SurfaceKind) -> &'static str {
    match surface {
        SurfaceKind::Wayland => "this Wayland compositor offers neither",
        SurfaceKind::X11 => "an X11 session offers neither",
        SurfaceKind::Other => "this display server offers neither",
    }
}

fn window_appearance(
    window: &Window,
    surface: SurfaceKind,
    blended: bool,
) -> WindowBackgroundAppearance {
    let client_decorated = matches!(
        crate::window_chrome::csd::decorations(window),
        Decorations::Client { .. }
    );
    let blur = compositor_blur();
    if blur_falls_back(blended, surface, blur) && first_blur_fallback() {
        log::warn!(
            "background_style: Blended needs compositor blur (ext-background-effect-v1 or \
             KDE blur) and {}, so the window uses the thick veil over an unblurred desktop",
            fallback_surface_name(surface)
        );
    }
    surface_appearance(blended, client_decorated, surface, blur)
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
            log::warn!(
                "background_style: could not list the compositor's globals ({error}), so \
                 Blended uses the thick veil over an unblurred desktop"
            );
            CompositorBlur::Unreachable
        }
    }
}

fn timed_blur_probe() -> CompositorBlur {
    let started = Instant::now();
    let blur = probe_compositor_blur();
    log::debug!(
        "background_style: the compositor blur probe answered {blur:?} in {:?}",
        started.elapsed()
    );
    blur
}

fn spawn_blur_probe(cx: &mut App) {
    cx.spawn(async move |cx| {
        let blur = cx.background_spawn(async move { timed_blur_probe() }).await;
        let _ = COMPOSITOR_BLUR.set(blur);
        cx.update(|cx| {
            for handle in cx.windows() {
                let _ = handle.update(cx, |_, window, _| window.refresh());
            }
        });
    })
    .detach();
}

pub(crate) fn sync_window_backdrop(window: &mut Window, blended: bool, cx: &mut App) {
    let surface = surface_kind(window);
    if needs_blur_probe(blended, surface, compositor_blur()) && first_blur_probe() {
        spawn_blur_probe(cx);
    }
    window.set_background_appearance(window_appearance(window, surface, blended));
}

#[cfg(test)]
mod tests {
    use super::{
        BlurSupport, CompositorBlur, SurfaceKind, blur_falls_back, blur_support_for,
        compositor_offers_blur, first_blur_fallback, needs_blur_probe, surface_appearance,
        unblurred_surface_appearance,
    };
    use gpui::WindowBackgroundAppearance;

    const SURFACES: [SurfaceKind; 3] = [SurfaceKind::Wayland, SurfaceKind::X11, SurfaceKind::Other];
    const BLUR_STATES: [CompositorBlur; 4] = [
        CompositorBlur::Pending,
        CompositorBlur::Offered,
        CompositorBlur::Missing,
        CompositorBlur::Unreachable,
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
    fn a_blended_window_requests_alpha_on_every_surface() {
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
    fn blended_blurs_where_the_wayland_compositor_offers_blur() {
        for client_decorated in [false, true] {
            assert_eq!(
                surface_appearance(
                    true,
                    client_decorated,
                    SurfaceKind::Wayland,
                    CompositorBlur::Offered
                ),
                WindowBackgroundAppearance::Blurred
            );
        }
        assert_eq!(
            blur_support_for(SurfaceKind::Wayland, CompositorBlur::Offered),
            BlurSupport::Full
        );
    }

    #[test]
    fn themed_never_requests_blur_and_keeps_the_unblurred_appearance() {
        for surface in SURFACES {
            for blur in BLUR_STATES {
                for client_decorated in [false, true] {
                    assert_eq!(
                        surface_appearance(false, client_decorated, surface, blur),
                        unblurred_surface_appearance(
                            false,
                            client_decorated,
                            surface == SurfaceKind::X11
                        )
                    );
                }
                assert!(!blur_falls_back(false, surface, blur));
                assert!(!needs_blur_probe(false, surface, blur));
            }
        }
    }

    #[test]
    fn blended_on_wayland_without_blur_is_transparent_and_unblurred() {
        for blur in [
            CompositorBlur::Pending,
            CompositorBlur::Missing,
            CompositorBlur::Unreachable,
        ] {
            assert_eq!(
                blur_support_for(SurfaceKind::Wayland, blur),
                BlurSupport::None
            );
            for client_decorated in [false, true] {
                assert_eq!(
                    surface_appearance(true, client_decorated, SurfaceKind::Wayland, blur),
                    WindowBackgroundAppearance::Transparent
                );
            }
        }
    }

    #[test]
    fn only_a_missing_blur_global_logs_the_fallback_on_wayland() {
        assert!(blur_falls_back(
            true,
            SurfaceKind::Wayland,
            CompositorBlur::Missing
        ));
        for blur in [
            CompositorBlur::Pending,
            CompositorBlur::Offered,
            CompositorBlur::Unreachable,
        ] {
            assert!(!blur_falls_back(true, SurfaceKind::Wayland, blur));
        }
    }

    #[test]
    fn the_blur_fallback_is_logged_once() {
        assert!(first_blur_fallback());
        assert!(!first_blur_fallback());
        assert!(!first_blur_fallback());
    }

    #[test]
    fn x11_never_blurs_whatever_the_probe_state() {
        for blur in BLUR_STATES {
            assert_eq!(blur_support_for(SurfaceKind::X11, blur), BlurSupport::None);
            assert!(blur_falls_back(true, SurfaceKind::X11, blur));
            assert!(!needs_blur_probe(true, SurfaceKind::X11, blur));
            for blended in [false, true] {
                for client_decorated in [false, true] {
                    assert_eq!(
                        surface_appearance(blended, client_decorated, SurfaceKind::X11, blur),
                        unblurred_surface_appearance(blended, client_decorated, true)
                    );
                }
            }
        }
    }

    #[test]
    fn blended_on_wayland_probes_only_until_the_compositor_answers() {
        assert!(needs_blur_probe(
            true,
            SurfaceKind::Wayland,
            CompositorBlur::Pending
        ));
        for blur in [
            CompositorBlur::Offered,
            CompositorBlur::Missing,
            CompositorBlur::Unreachable,
        ] {
            assert!(!needs_blur_probe(true, SurfaceKind::Wayland, blur));
        }
        for blur in BLUR_STATES {
            assert!(!needs_blur_probe(true, SurfaceKind::Other, blur));
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

    #[test]
    fn every_style_surface_and_probe_state_follows_the_decision_table() {
        for surface in SURFACES {
            for blur in BLUR_STATES {
                for blended in [false, true] {
                    for client_decorated in [false, true] {
                        let support =
                            if surface == SurfaceKind::Wayland && blur == CompositorBlur::Offered {
                                BlurSupport::Full
                            } else {
                                BlurSupport::None
                            };
                        let expected = if blended && support == BlurSupport::Full {
                            WindowBackgroundAppearance::Blurred
                        } else if blended || (client_decorated && surface == SurfaceKind::X11) {
                            WindowBackgroundAppearance::Transparent
                        } else {
                            WindowBackgroundAppearance::Opaque
                        };
                        assert_eq!(blur_support_for(surface, blur), support);
                        assert_eq!(
                            surface_appearance(blended, client_decorated, surface, blur),
                            expected,
                            "{surface:?} {blur:?} blended={blended} csd={client_decorated}"
                        );
                    }
                }
            }
        }
    }
}
