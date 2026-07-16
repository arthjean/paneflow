//! Native Linux compositor blur for PaneFlow's sidebar and title bar.
//!
//! Linux has no distribution-wide material API. Capability detection is done
//! against the active display server: ext-background-effect-v1 on Wayland,
//! GPUI's legacy KDE Wayland integration, then KWin's X11 property.

use std::{
    cell::RefCell,
    ffi::c_void,
    sync::atomic::{AtomicBool, Ordering},
};

use anyhow::{Context as _, Result, anyhow};
use gpui::{Decorations, Window, WindowBackgroundAppearance};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};

const LINUX_NATIVE_BACKDROP_ENV: &str = "PANEFLOW_LINUX_NATIVE_BACKDROP";

static NATIVE_BLUR_ACTIVE: AtomicBool = AtomicBool::new(false);

thread_local! {
    static BACKDROP: RefCell<Option<LinuxBackdrop>> = const { RefCell::new(None) };
    static CHROME_GEOMETRY: RefCell<Option<ChromeGeometry>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ChromeGeometry {
    pub(crate) left_sidebar_width: f32,
    pub(crate) right_sidebar_width: f32,
    pub(crate) title_bar_height: f32,
    pub(crate) title_bar_spans_window: bool,
}

impl ChromeGeometry {
    fn fallback(window: &Window) -> Self {
        Self {
            left_sidebar_width: crate::SIDEBAR_WIDTH,
            right_sidebar_width: 0.,
            title_bar_height: (1.75 * f32::from(window.rem_size())).max(34.),
            title_bar_spans_window: true,
        }
    }
}

pub(crate) fn set_chrome_geometry(geometry: ChromeGeometry) {
    CHROME_GEOMETRY.with(|slot| {
        *slot.borrow_mut() = Some(geometry);
    });
}

/// Whether PaneFlow can safely expose translucent application chrome.
pub(crate) fn native_blur_active() -> bool {
    NATIVE_BLUR_ACTIVE.load(Ordering::Relaxed)
}

fn native_blur_is_active(capability_available: bool, refresh_succeeded: bool) -> bool {
    capability_available && refresh_succeeded
}

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

fn native_backdrop_enabled() -> bool {
    match std::env::var(LINUX_NATIVE_BACKDROP_ENV) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

/// Detects and installs the best native blur mechanism for the active session.
pub(crate) fn apply_subtle_chrome_material(window: &mut Window) {
    if !native_backdrop_enabled() {
        BACKDROP.with(|slot| {
            slot.borrow_mut().take();
        });
        NATIVE_BLUR_ACTIVE.store(false, Ordering::Relaxed);
        window.set_background_appearance(unblurred_window_appearance(window));
        window.refresh();
        return;
    }

    ensure_backdrop(window);
    refresh_blur_region(window);
    window.refresh();
}

fn ensure_backdrop(window: &Window) {
    BACKDROP.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            let backdrop = match LinuxBackdrop::new(window) {
                Ok(backdrop) => backdrop,
                Err(error) => {
                    log::warn!("Linux native blur initialization failed: {error:#}");
                    LinuxBackdrop::Unsupported
                }
            };
            *slot = Some(backdrop);
        }
    });
}

/// Refreshes compositor regions after resize and processes Wayland capability
/// changes. The blur is confined to the sidebar plus title bar.
pub(crate) fn refresh_blur_region(window: &mut Window) {
    if !native_backdrop_enabled() {
        BACKDROP.with(|slot| {
            slot.borrow_mut().take();
        });
        NATIVE_BLUR_ACTIVE.store(false, Ordering::Relaxed);
        window.set_background_appearance(unblurred_window_appearance(window));
        return;
    }

    ensure_backdrop(window);
    BACKDROP.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(backdrop) = slot.as_mut() else {
            return;
        };

        let refresh_succeeded = backdrop
            .refresh(window)
            .inspect_err(|error| {
                log::warn!("Could not refresh the Linux blur region: {error:#}");
            })
            .is_ok();

        let active = native_blur_is_active(backdrop.is_active(), refresh_succeeded);
        NATIVE_BLUR_ACTIVE.store(active, Ordering::Relaxed);
        window.set_background_appearance(if active {
            backdrop.background_appearance()
        } else {
            unblurred_window_appearance(window)
        });
    });
}

/// Releases guest Wayland/X11 resources before GPUI tears down its display.
pub(crate) fn clear_subtle_chrome_material() {
    BACKDROP.with(|slot| {
        slot.borrow_mut().take();
    });
    NATIVE_BLUR_ACTIVE.store(false, Ordering::Relaxed);
}

enum LinuxBackdrop {
    WaylandExt(WaylandExtBackdrop),
    WaylandKde(WaylandGuest),
    WaylandUnsupported(WaylandGuest),
    X11Kde(X11Backdrop),
    Unsupported,
}

impl LinuxBackdrop {
    fn new(window: &Window) -> Result<Self> {
        let window_handle = HasWindowHandle::window_handle(window)
            .map_err(|error| anyhow!("GPUI did not expose a Linux window handle: {error:?}"))?;
        let display_handle = HasDisplayHandle::display_handle(window)
            .map_err(|error| anyhow!("GPUI did not expose a Linux display handle: {error:?}"))?;

        match (window_handle.as_raw(), display_handle.as_raw()) {
            (
                RawWindowHandle::Wayland(window_handle),
                RawDisplayHandle::Wayland(display_handle),
            ) => setup_wayland(
                window_handle.surface.as_ptr(),
                display_handle.display.as_ptr(),
            ),
            (RawWindowHandle::Xcb(window_handle), RawDisplayHandle::Xcb(display_handle)) => {
                let connection = display_handle
                    .connection
                    .ok_or_else(|| anyhow!("GPUI returned a null XCB connection"))?;
                setup_x11(
                    window,
                    connection.as_ptr(),
                    display_handle.screen,
                    window_handle.window.get(),
                )
            }
            _ => Ok(Self::Unsupported),
        }
    }

    fn is_active(&self) -> bool {
        match self {
            Self::WaylandExt(backdrop) => backdrop.guest.state.blur_supported,
            Self::WaylandKde(_) | Self::X11Kde(_) => true,
            Self::WaylandUnsupported(_) | Self::Unsupported => false,
        }
    }

    fn background_appearance(&self) -> WindowBackgroundAppearance {
        match self {
            Self::WaylandKde(_) => WindowBackgroundAppearance::Blurred,
            Self::WaylandExt(_) | Self::X11Kde(_) => WindowBackgroundAppearance::Transparent,
            Self::WaylandUnsupported(_) | Self::Unsupported => WindowBackgroundAppearance::Opaque,
        }
    }

    fn refresh(&mut self, window: &Window) -> Result<()> {
        match self {
            Self::WaylandExt(backdrop) => backdrop.refresh(window),
            Self::X11Kde(backdrop) => backdrop.refresh(window),
            Self::WaylandKde(guest) => {
                guest.dispatch_pending()?;
                Ok(())
            }
            Self::WaylandUnsupported(guest) => {
                guest.dispatch_pending()?;
                Ok(())
            }
            Self::Unsupported => Ok(()),
        }
    }
}

fn chrome_rectangles(window: &Window, scale: f32) -> Vec<[i32; 4]> {
    let bounds = window.bounds().size;
    let width = (f32::from(bounds.width) * scale).ceil().max(1.0) as i32;
    let height = (f32::from(bounds.height) * scale).ceil().max(1.0) as i32;
    let geometry = CHROME_GEOMETRY.with(|slot| {
        slot.borrow()
            .unwrap_or_else(|| ChromeGeometry::fallback(window))
    });
    chrome_rectangles_for_geometry(width, height, geometry, scale)
}

fn chrome_rectangles_for_geometry(
    width: i32,
    height: i32,
    geometry: ChromeGeometry,
    scale: f32,
) -> Vec<[i32; 4]> {
    let left_sidebar = (geometry.left_sidebar_width * scale)
        .ceil()
        .clamp(0.0, width as f32) as i32;
    let right_sidebar = (geometry.right_sidebar_width * scale)
        .ceil()
        .clamp(0.0, width.saturating_sub(left_sidebar) as f32) as i32;
    let title_bar = (geometry.title_bar_height * scale)
        .ceil()
        .clamp(1.0, height as f32) as i32;

    let mut rectangles = Vec::with_capacity(3);
    if left_sidebar > 0 {
        rectangles.push([0, 0, left_sidebar, height]);
    }
    if geometry.title_bar_spans_window && left_sidebar < width {
        let title_width = width - left_sidebar;
        rectangles.push([left_sidebar, 0, title_width, title_bar]);
    }
    if right_sidebar > 0 {
        let x = width - right_sidebar;
        let y = if geometry.title_bar_spans_window {
            title_bar
        } else {
            0
        };
        let h = height.saturating_sub(y);
        if h > 0 {
            rectangles.push([x, y, right_sidebar, h]);
        }
    }
    rectangles
}

use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop,
    globals::{GlobalList, GlobalListContents, registry_queue_init},
    protocol::{wl_compositor, wl_region, wl_registry, wl_surface},
};
use wayland_protocols::ext::background_effect::v1::client::{
    ext_background_effect_manager_v1, ext_background_effect_surface_v1,
};

const KDE_WAYLAND_BLUR_INTERFACE: &str = "org_kde_kwin_blur_manager";

struct WaylandDispatchState {
    blur_supported: bool,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for WaylandDispatchState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _connection: &Connection,
        _queue: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1, ()>
    for WaylandDispatchState
{
    fn event(
        state: &mut Self,
        _proxy: &ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1,
        event: ext_background_effect_manager_v1::Event,
        _data: &(),
        _connection: &Connection,
        _queue: &QueueHandle<Self>,
    ) {
        if let ext_background_effect_manager_v1::Event::Capabilities { flags } = event {
            state.blur_supported = matches!(
                flags,
                WEnum::Value(capabilities)
                    if capabilities.contains(
                        ext_background_effect_manager_v1::Capability::Blur
                    )
            );
        }
    }
}

delegate_noop!(WaylandDispatchState: ignore wl_compositor::WlCompositor);
delegate_noop!(WaylandDispatchState: ignore wl_region::WlRegion);
delegate_noop!(
    WaylandDispatchState:
    ignore ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1
);

struct WaylandGuest {
    connection: Connection,
    _globals: GlobalList,
    event_queue: EventQueue<WaylandDispatchState>,
    state: WaylandDispatchState,
}

impl WaylandGuest {
    fn dispatch_pending(&mut self) -> Result<()> {
        self.event_queue
            .dispatch_pending(&mut self.state)
            .context("Wayland background-effect dispatch failed")?;
        Ok(())
    }
}

struct WaylandExtBackdrop {
    guest: WaylandGuest,
    manager: ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1,
    compositor: wl_compositor::WlCompositor,
    effect: ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1,
    last_rectangles: Vec<[i32; 4]>,
}

fn setup_wayland(surface_ptr: *mut c_void, display_ptr: *mut c_void) -> Result<LinuxBackdrop> {
    let backend =
        unsafe { wayland_client::backend::Backend::from_foreign_display(display_ptr.cast()) };
    let connection = Connection::from_backend(backend);
    let (globals, mut event_queue) = registry_queue_init::<WaylandDispatchState>(&connection)
        .context("Could not read Wayland globals")?;
    let queue = event_queue.handle();
    let kde_available = globals.contents().with_list(|globals| {
        globals
            .iter()
            .any(|global| global.interface == KDE_WAYLAND_BLUR_INTERFACE)
    });

    let mut state = WaylandDispatchState {
        blur_supported: false,
    };
    let manager = match globals
        .bind::<ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1, _, _>(
            &queue,
            1..=1,
            (),
        ) {
        Ok(manager) => manager,
        Err(_) => {
            let guest = WaylandGuest {
                connection,
                _globals: globals,
                event_queue,
                state,
            };
            return Ok(if kde_available {
                LinuxBackdrop::WaylandKde(guest)
            } else {
                LinuxBackdrop::WaylandUnsupported(guest)
            });
        }
    };

    event_queue
        .roundtrip(&mut state)
        .context("Could not read Wayland background-effect capabilities")?;
    if !state.blur_supported {
        manager.destroy();
        connection.flush().ok();
        let guest = WaylandGuest {
            connection,
            _globals: globals,
            event_queue,
            state,
        };
        return Ok(if kde_available {
            LinuxBackdrop::WaylandKde(guest)
        } else {
            LinuxBackdrop::WaylandUnsupported(guest)
        });
    }

    let compositor: wl_compositor::WlCompositor = globals
        .bind(&queue, 1..=6, ())
        .context("Wayland compositor global is unavailable")?;
    let surface_id = unsafe {
        wayland_client::backend::ObjectId::from_ptr(
            wl_surface::WlSurface::interface(),
            surface_ptr.cast(),
        )
    }
    .context("Could not import GPUI's Wayland surface")?;
    let surface = wl_surface::WlSurface::from_id(&connection, surface_id)
        .context("Could not wrap GPUI's Wayland surface")?;
    let effect = manager.get_background_effect(&surface, &queue, ());

    Ok(LinuxBackdrop::WaylandExt(WaylandExtBackdrop {
        guest: WaylandGuest {
            connection,
            _globals: globals,
            event_queue,
            state,
        },
        manager,
        compositor,
        effect,
        last_rectangles: Vec::new(),
    }))
}

impl WaylandExtBackdrop {
    fn refresh(&mut self, window: &Window) -> Result<()> {
        self.guest.dispatch_pending()?;
        if !self.guest.state.blur_supported {
            self.effect.set_blur_region(None);
            self.guest.connection.flush().ok();
            self.last_rectangles.clear();
            return Ok(());
        }

        let rectangles = chrome_rectangles(window, 1.0);
        if rectangles == self.last_rectangles {
            return Ok(());
        }
        if rectangles.is_empty() {
            self.effect.set_blur_region(None);
            self.guest
                .connection
                .flush()
                .context("Could not clear the Wayland blur region")?;
            self.last_rectangles = rectangles;
            return Ok(());
        }

        let queue = self.guest.event_queue.handle();
        let region = self.compositor.create_region(&queue, ());
        for [x, y, width, height] in &rectangles {
            region.add(*x, *y, *width, *height);
        }
        self.effect.set_blur_region(Some(&region));
        region.destroy();
        self.guest
            .connection
            .flush()
            .context("Could not flush the Wayland blur region")?;
        self.last_rectangles = rectangles;
        Ok(())
    }
}

impl Drop for WaylandExtBackdrop {
    fn drop(&mut self) {
        self.effect.set_blur_region(None);
        self.effect.destroy();
        self.manager.destroy();
        let _ = self.guest.connection.flush();
    }
}

use x11rb::{
    connection::Connection as _,
    protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode},
    wrapper::ConnectionExt as _,
    xcb_ffi::XCBConnection,
};

const KDE_X11_BLUR_ATOM: &[u8] = b"_KDE_NET_WM_BLUR_BEHIND_REGION";

struct X11Backdrop {
    connection: XCBConnection,
    window: u32,
    atom: u32,
    last_rectangles: Vec<[i32; 4]>,
}

fn setup_x11(
    window: &Window,
    connection_ptr: *mut c_void,
    screen: i32,
    window_id: u32,
) -> Result<LinuxBackdrop> {
    let connection = unsafe { XCBConnection::from_raw_xcb_connection(connection_ptr, false) }
        .context("Could not import GPUI's XCB connection")?;
    let root = connection
        .setup()
        .roots
        .get(screen.max(0) as usize)
        .ok_or_else(|| anyhow!("XCB screen index {screen} is unavailable"))?
        .root;
    let atom = connection
        .intern_atom(true, KDE_X11_BLUR_ATOM)
        .context("Could not query the KDE X11 blur atom")?
        .reply()
        .context("KDE X11 blur atom query failed")?
        .atom;
    if atom == x11rb::NONE {
        return Ok(LinuxBackdrop::Unsupported);
    }

    let properties = connection
        .list_properties(root)
        .context("Could not inspect X11 root properties")?
        .reply()
        .context("X11 root property query failed")?;
    if !properties.atoms.contains(&atom) {
        return Ok(LinuxBackdrop::Unsupported);
    }

    let mut backdrop = X11Backdrop {
        connection,
        window: window_id,
        atom,
        last_rectangles: Vec::new(),
    };
    backdrop.refresh(window)?;
    Ok(LinuxBackdrop::X11Kde(backdrop))
}

impl X11Backdrop {
    fn refresh(&mut self, window: &Window) -> Result<()> {
        let rectangles = chrome_rectangles(window, window.scale_factor());
        if rectangles == self.last_rectangles {
            return Ok(());
        }
        if rectangles.is_empty() {
            self.connection
                .delete_property(self.window, self.atom)
                .context("Could not clear the KDE X11 blur region")?
                .check()
                .context("KDE X11 rejected the cleared blur region")?;
            self.connection
                .flush()
                .context("Could not flush the cleared KDE X11 blur region")?;
            self.last_rectangles = rectangles;
            return Ok(());
        }

        let data: Vec<u32> = rectangles
            .iter()
            .flat_map(|rectangle| rectangle.iter().map(|value| *value as u32))
            .collect();
        self.connection
            .change_property32(
                PropMode::REPLACE,
                self.window,
                self.atom,
                AtomEnum::CARDINAL,
                &data,
            )
            .context("Could not set the KDE X11 blur region")?
            .check()
            .context("KDE X11 rejected the blur region")?;
        self.connection
            .flush()
            .context("Could not flush the KDE X11 blur region")?;
        self.last_rectangles = rectangles;
        Ok(())
    }
}

impl Drop for X11Backdrop {
    fn drop(&mut self) {
        if let Ok(cookie) = self.connection.delete_property(self.window, self.atom) {
            let _ = cookie.check();
        }
        let _ = self.connection.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ChromeGeometry, chrome_rectangles_for_geometry, native_blur_is_active,
        unblurred_surface_appearance,
    };
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

    #[test]
    fn chrome_material_requests_only_chrome_regions() {
        let geometry = ChromeGeometry {
            left_sidebar_width: 240.0,
            right_sidebar_width: 300.0,
            title_bar_height: 34.0,
            title_bar_spans_window: true,
        };

        assert_eq!(
            chrome_rectangles_for_geometry(1200, 800, geometry, 1.0),
            vec![[0, 0, 240, 800], [240, 0, 960, 34], [900, 34, 300, 766]]
        );
    }

    #[test]
    fn transient_refresh_failure_does_not_activate_blur() {
        assert!(!native_blur_is_active(true, false));
        assert!(native_blur_is_active(true, true));
    }
}
