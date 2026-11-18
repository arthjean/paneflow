use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context as _;
use raw_window_handle as rwh;
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface},
};

use crate::linux::WaylandClientStatePtr;
use gpui_wgpu::{PresentationProbe, PresentationProbeWindow, SoftwareFrame, SoftwarePresenter};

const MAX_SHM_BUFFERS: usize = 3;

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

struct ShmBuffer {
    buffer: wl_buffer::WlBuffer,
    pool: wl_shm_pool::WlShmPool,
    memory: NonNull<u8>,
    len: usize,
    width: u32,
    height: u32,
    stride: usize,
    opaque: bool,
    released: Arc<AtomicBool>,
    _fd: OwnedFd,
}

impl ShmBuffer {
    fn new(
        shm: &wl_shm::WlShm,
        qh: &QueueHandle<WaylandClientStatePtr>,
        width: u32,
        height: u32,
        opaque: bool,
    ) -> anyhow::Result<Self> {
        let stride = width as usize * 4;
        let len = stride * height as usize;
        let raw_fd = unsafe { libc::memfd_create(c"gpui-wl-shm".as_ptr(), libc::MFD_CLOEXEC) };
        anyhow::ensure!(
            raw_fd >= 0,
            "memfd_create failed: {}",
            std::io::Error::last_os_error()
        );
        let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
        anyhow::ensure!(
            unsafe { libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) } == 0,
            "ftruncate of a {len}-byte shm buffer failed: {}",
            std::io::Error::last_os_error()
        );
        let memory = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        anyhow::ensure!(
            memory != libc::MAP_FAILED,
            "mmap of a {len}-byte shm buffer failed: {}",
            std::io::Error::last_os_error()
        );
        let memory = NonNull::new(memory.cast::<u8>()).context("mmap returned null")?;
        let pool = shm.create_pool(fd.as_fd(), len as i32, qh, ());
        let released = Arc::new(AtomicBool::new(true));
        let format = if opaque {
            wl_shm::Format::Xrgb8888
        } else {
            wl_shm::Format::Argb8888
        };
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            stride as i32,
            format,
            qh,
            Arc::clone(&released),
        );
        Ok(Self {
            buffer,
            pool,
            memory,
            len,
            width,
            height,
            stride,
            opaque,
            released,
            _fd: fd,
        })
    }

    fn is_released(&self) -> bool {
        self.released.load(Ordering::Acquire)
    }

    fn fits(&self, frame: &SoftwareFrame<'_>) -> bool {
        self.width == frame.width && self.height == frame.height && self.opaque == frame.opaque
    }
}

impl Drop for ShmBuffer {
    fn drop(&mut self) {
        self.buffer.destroy();
        self.pool.destroy();
        unsafe {
            libc::munmap(self.memory.as_ptr().cast(), self.len);
        }
    }
}

pub(crate) struct ShmPresenter {
    shm: wl_shm::WlShm,
    surface: wl_surface::WlSurface,
    qh: QueueHandle<WaylandClientStatePtr>,
    buffers: Vec<ShmBuffer>,
}

impl ShmPresenter {
    pub(crate) fn new(
        shm: wl_shm::WlShm,
        surface: wl_surface::WlSurface,
        qh: QueueHandle<WaylandClientStatePtr>,
    ) -> Self {
        Self {
            shm,
            surface,
            qh,
            buffers: Vec::new(),
        }
    }
}

impl SoftwarePresenter for ShmPresenter {
    fn present(&mut self, frame: SoftwareFrame<'_>) -> bool {
        self.buffers
            .retain(|buffer| !buffer.is_released() || buffer.fits(&frame));
        let index = match self
            .buffers
            .iter()
            .position(|buffer| buffer.is_released() && buffer.fits(&frame))
        {
            Some(index) => index,
            None if self.buffers.len() < MAX_SHM_BUFFERS => {
                match ShmBuffer::new(&self.shm, &self.qh, frame.width, frame.height, frame.opaque) {
                    Ok(buffer) => {
                        self.buffers.push(buffer);
                        self.buffers.len() - 1
                    }
                    Err(error) => {
                        log::error!("Cannot allocate a shm buffer: {error:#}");
                        return false;
                    }
                }
            }
            None => return false,
        };

        let buffer = &mut self.buffers[index];
        let row = frame.width as usize * 4;
        let pixels = unsafe { std::slice::from_raw_parts_mut(buffer.memory.as_ptr(), buffer.len) };
        for (destination, source) in pixels
            .chunks_exact_mut(buffer.stride)
            .zip(frame.pixels.chunks(frame.stride))
            .take(frame.height as usize)
        {
            destination[..row].copy_from_slice(&source[..row]);
            if !frame.bgra {
                for pixel in destination[..row].chunks_exact_mut(4) {
                    pixel.swap(0, 2);
                }
            }
        }

        buffer.released.store(false, Ordering::Release);
        self.surface.attach(Some(&buffer.buffer), 0, 0);
        if self.surface.version() >= wl_surface::REQ_DAMAGE_BUFFER_SINCE {
            self.surface
                .damage_buffer(0, 0, frame.width as i32, frame.height as i32);
        } else {
            self.surface.damage(0, 0, i32::MAX, i32::MAX);
        }
        self.surface.commit();
        true
    }
}

impl Dispatch<wl_buffer::WlBuffer, Arc<AtomicBool>> for WaylandClientStatePtr {
    fn event(
        _: &mut Self,
        _: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        released: &Arc<AtomicBool>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            released.store(true, Ordering::Release);
        }
    }
}
