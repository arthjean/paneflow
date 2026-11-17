use std::ptr::NonNull;
use std::rc::Rc;

use anyhow::Context as _;
use raw_window_handle as rwh;
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_compositor, wl_registry, wl_surface},
};

use gpui_wgpu::{PresentationProbe, PresentationProbeWindow};

struct ProbeState;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ProbeState {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for ProbeState {
    fn event(
        _: &mut Self,
        _: &wl_compositor::WlCompositor,
        _: wl_compositor::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for ProbeState {
    fn event(
        _: &mut Self,
        _: &wl_surface::WlSurface,
        _: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

struct SacrificialSurface {
    surface: wl_surface::WlSurface,
    queue: EventQueue<ProbeState>,
    connection: Connection,
}

impl SacrificialSurface {
    fn connect() -> anyhow::Result<Self> {
        let connection = Connection::connect_to_env()
            .context("cannot open a second connection to the Wayland compositor")?;
        let (globals, queue) = registry_queue_init::<ProbeState>(&connection)
            .context("cannot list the globals of the second Wayland connection")?;
        let compositor: wl_compositor::WlCompositor = globals
            .bind(&queue.handle(), 1..=4, ())
            .context("the second Wayland connection has no wl_compositor")?;
        let surface = compositor.create_surface(&queue.handle(), ());
        Ok(Self {
            surface,
            queue,
            connection,
        })
    }
}

impl rwh::HasWindowHandle for SacrificialSurface {
    fn window_handle(&self) -> Result<rwh::WindowHandle<'_>, rwh::HandleError> {
        let surface = NonNull::new(self.surface.id().as_ptr().cast())
            .ok_or(rwh::HandleError::Unavailable)?;
        let handle = rwh::WaylandWindowHandle::new(surface);
        Ok(unsafe { rwh::WindowHandle::borrow_raw(handle.into()) })
    }
}

impl rwh::HasDisplayHandle for SacrificialSurface {
    fn display_handle(&self) -> Result<rwh::DisplayHandle<'_>, rwh::HandleError> {
        let display = NonNull::new(self.connection.backend().display_ptr().cast())
            .ok_or(rwh::HandleError::Unavailable)?;
        let handle = rwh::WaylandDisplayHandle::new(display);
        Ok(unsafe { rwh::DisplayHandle::borrow_raw(handle.into()) })
    }
}

impl PresentationProbeWindow for SacrificialSurface {
    fn connection_survived(&mut self) -> bool {
        self.queue.roundtrip(&mut ProbeState).is_ok() && self.connection.protocol_error().is_none()
    }
}

pub(crate) fn sacrificial_surface_probe() -> PresentationProbe {
    Rc::new(|| {
        SacrificialSurface::connect()
            .map(|surface| Box::new(surface) as Box<dyn PresentationProbeWindow>)
    })
}
