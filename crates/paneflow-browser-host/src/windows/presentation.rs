use std::time::{Duration, Instant};

use cef::{AcceleratedPaintInfo, ColorType, PaintElementType, Rect};
use paneflow_browser_protocol::{
    BufferLayout, DirtyRect, Document, FrameAck, FrameFailure, FrameFormat, FrameMessage,
    PlaneLayout, MAX_PENDING_FRAMES, POOL_BUFFERS, RETIRE_DEADLINE_MS,
};
use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, DuplicateHandle, DUPLICATE_CLOSE_SOURCE, HANDLE};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11Device1, ID3D11DeviceContext, ID3D11Resource,
    ID3D11Texture2D, D3D11_BIND_SHADER_RESOURCE, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX, D3D11_RESOURCE_MISC_SHARED_NTHANDLE, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM,
};
use windows::Win32::Graphics::Dxgi::{IDXGIKeyedMutex, IDXGIResource1, DXGI_SHARED_RESOURCE_READ};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcess, PROCESS_DUP_HANDLE};

const MAX_POOL_WIDTH: u32 = 16_384;
const MAX_POOL_HEIGHT: u32 = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Free,
    InFlight(u64),
}

struct Pool {
    document: Document,
    generation: u64,
    width: u32,
    height: u32,
    format: FrameFormat,
    textures: Vec<ID3D11Texture2D>,
    mutexes: Vec<IDXGIKeyedMutex>,
    slots: [Slot; POOL_BUFFERS as usize],
    ready: bool,
    retiring: bool,
    retiring_since: Option<Instant>,
}

struct PoolSpec {
    width: u32,
    height: u32,
    format: FrameFormat,
}

pub struct Presenter {
    document: Document,
    client_process: HANDLE,
    device: ID3D11Device,
    device1: ID3D11Device1,
    context: ID3D11DeviceContext,
    pools: Vec<Pool>,
    pending_resize: Option<PoolSpec>,
    next_generation: u64,
    next_sequence: u64,
    pending_frames: usize,
    disabled: Option<FrameFailure>,
}

impl Presenter {
    pub fn new(document: Document, client_pid: u32) -> Result<Self, String> {
        if client_pid == 0 {
            return Err("browser client process id is invalid".to_string());
        }
        let client_process = unsafe { OpenProcess(PROCESS_DUP_HANDLE, false, client_pid) }
            .map_err(|error| format!("opening browser client for handle duplication: {error}"))?;
        let mut device = None;
        let mut context = None;
        let result = unsafe {
            D3D11CreateDevice(
                None::<&windows::Win32::Graphics::Dxgi::IDXGIAdapter>,
                D3D_DRIVER_TYPE_HARDWARE,
                Default::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        };
        if let Err(error) = result {
            unsafe {
                let _ = CloseHandle(client_process);
            }
            return Err(format!(
                "creating the Windows browser D3D11 device: {error}"
            ));
        }
        let Some(device) = device else {
            unsafe {
                let _ = CloseHandle(client_process);
            }
            return Err("D3D11 device creation returned no device".to_string());
        };
        let Some(context) = context else {
            unsafe {
                let _ = CloseHandle(client_process);
            }
            return Err("D3D11 device creation returned no immediate context".to_string());
        };
        let device1 = match device.cast::<ID3D11Device1>() {
            Ok(device1) => device1,
            Err(error) => {
                unsafe {
                    let _ = CloseHandle(client_process);
                }
                return Err(format!(
                    "D3D11 device has no shared-resource interface: {error}"
                ));
            }
        };
        Ok(Self {
            document,
            client_process,
            device,
            device1,
            context,
            pools: Vec::new(),
            pending_resize: None,
            next_generation: 1,
            next_sequence: 1,
            pending_frames: 0,
            disabled: None,
        })
    }

    pub fn update_document(&mut self, document: Document) {
        self.document = document;
        for pool in &mut self.pools {
            pool.retiring = true;
            pool.retiring_since.get_or_insert_with(Instant::now);
        }
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn handle_ack(&mut self, ack: FrameAck) {
        match ack {
            FrameAck::PoolReady {
                document,
                pool_generation,
            } => self.pool_ready(document, pool_generation),
            FrameAck::PoolRejected {
                document,
                pool_generation,
            } => self.pool_rejected(document, pool_generation),
            FrameAck::Release {
                document,
                pool_generation,
                buffer,
                sequence,
            } => self.release(document, pool_generation, buffer, sequence),
        }
        self.retire_expired();
        self.flush_pending_resize();
    }

    pub fn paint(
        &mut self,
        type_: PaintElementType,
        dirty_rects: Option<&[Rect]>,
        info: &AcceleratedPaintInfo,
    ) {
        let callback_ns = super::now_ns();
        if type_ != PaintElementType::VIEW || self.disabled.is_some() {
            return;
        }
        self.retire_expired();
        let Some(spec) = pool_spec(info) else {
            self.fail(
                FrameFailure::UnsupportedFormat,
                "CEF returned an unsupported texture format",
            );
            return;
        };
        if self.ensure_pool(&spec).is_err() {
            self.fail(
                FrameFailure::CopyFailed,
                "creating the D3D11 frame pool failed",
            );
            return;
        }
        if self.disabled.is_some() {
            return;
        }
        if self.pending_frames >= MAX_PENDING_FRAMES {
            return;
        }
        let Some(pool_index) = self.active_pool(spec.width, spec.height, spec.format) else {
            return;
        };
        let Some(buffer) = self.free_buffer(pool_index) else {
            return;
        };
        let source = match self.open_source(info, &spec) {
            Ok(source) => source,
            Err((reason, detail)) => {
                self.fail(reason, &detail);
                return;
            }
        };
        let destination = self.pools[pool_index].textures[usize::from(buffer)].clone();
        let source_resource = match source.cast::<ID3D11Resource>() {
            Ok(resource) => resource,
            Err(error) => {
                self.fail(
                    FrameFailure::WrongDevice,
                    &format!("CEF source texture is not a D3D11 resource: {error}"),
                );
                return;
            }
        };
        let destination_resource = match destination.cast::<ID3D11Resource>() {
            Ok(resource) => resource,
            Err(error) => {
                self.fail(
                    FrameFailure::WrongDevice,
                    &format!("owned destination texture is not a D3D11 resource: {error}"),
                );
                return;
            }
        };
        let mutex = &self.pools[pool_index].mutexes[usize::from(buffer)];
        let acquired = unsafe { (mutex.vtable().AcquireSync)(mutex.as_raw(), 0, 0) };
        if acquired.0 == 258 {
            return;
        }
        if acquired.0 != 0 {
            self.fail(
                FrameFailure::WrongDevice,
                "owned texture synchronization failed",
            );
            return;
        }
        unsafe {
            self.context
                .CopyResource(&destination_resource, &source_resource);
            self.context.Flush();
        }
        if unsafe { mutex.ReleaseSync(0) }.is_err() {
            self.fail(FrameFailure::WrongDevice, "owned texture release failed");
            return;
        }
        if let Err(error) = unsafe { self.device.GetDeviceRemovedReason() } {
            self.fail(
                FrameFailure::WrongDevice,
                &format!("D3D11 device was removed during frame copy: {error}"),
            );
            return;
        }
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.pools[pool_index].slots[usize::from(buffer)] = Slot::InFlight(sequence);
        self.pending_frames = self.pending_frames.saturating_add(1);
        let message = FrameMessage::Frame {
            document: self.document.clone(),
            pool_generation: self.pools[pool_index].generation,
            buffer,
            sequence,
            callback_ns,
            ready_ns: super::now_ns(),
            capture_timestamp_us: info.extra.timestamp / 1_000,
            capture_counter: (info.extra.has_capture_counter != 0)
                .then_some(info.extra.capture_counter),
            dirty: union(dirty_rects.unwrap_or_default(), spec.width, spec.height),
        };
        if super::emit_frame(&message).is_err() {
            self.pools[pool_index].slots[usize::from(buffer)] = Slot::Free;
            self.pending_frames = self.pending_frames.saturating_sub(1);
            self.fail(FrameFailure::CopyFailed, "frame publication failed");
        }
    }

    fn ensure_pool(&mut self, spec: &PoolSpec) -> Result<(), ()> {
        if self.pools.iter().any(|pool| {
            !pool.retiring
                && pool.width == spec.width
                && pool.height == spec.height
                && pool.format == spec.format
        }) {
            return Ok(());
        }
        if self.pools.len() >= 2 {
            self.pending_resize = Some(PoolSpec {
                width: spec.width,
                height: spec.height,
                format: spec.format,
            });
            return Ok(());
        }
        for pool in &mut self.pools {
            pool.retiring = true;
            pool.retiring_since.get_or_insert_with(Instant::now);
        }
        let pool = self.create_pool(spec)?;
        self.pools.push(pool);
        Ok(())
    }

    fn create_pool(&mut self, spec: &PoolSpec) -> Result<Pool, ()> {
        let dxgi_format = format_to_dxgi(spec.format);
        let buffer_size = u64::from(spec.width)
            .checked_mul(u64::from(spec.height))
            .and_then(|value| value.checked_mul(4))
            .ok_or(())?;
        let stride = spec.width.checked_mul(4).ok_or(())?;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: spec.width,
            Height: spec.height,
            MipLevels: 1,
            ArraySize: 1,
            Format: dxgi_format,
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: (D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX | D3D11_RESOURCE_MISC_SHARED_NTHANDLE)
                .0 as u32,
        };
        let mut textures = Vec::with_capacity(POOL_BUFFERS as usize);
        let mut mutexes = Vec::with_capacity(POOL_BUFFERS as usize);
        let mut remote_handles = Vec::with_capacity(POOL_BUFFERS as usize);
        let mut handles = Vec::with_capacity(POOL_BUFFERS as usize);
        for _ in 0..POOL_BUFFERS {
            let mut texture = None;
            if unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut texture)) }.is_err() {
                self.close_remote_handles(&remote_handles);
                return Err(());
            }
            let Some(texture) = texture else {
                self.close_remote_handles(&remote_handles);
                return Err(());
            };
            let mutex = match texture.cast::<IDXGIKeyedMutex>() {
                Ok(mutex) => mutex,
                Err(_) => {
                    self.close_remote_handles(&remote_handles);
                    return Err(());
                }
            };
            let resource = match texture.cast::<IDXGIResource1>() {
                Ok(resource) => resource,
                Err(_) => {
                    self.close_remote_handles(&remote_handles);
                    return Err(());
                }
            };
            let local = match unsafe {
                resource.CreateSharedHandle(None, DXGI_SHARED_RESOURCE_READ.0, PCWSTR::null())
            } {
                Ok(handle) => handle,
                Err(_) => {
                    self.close_remote_handles(&remote_handles);
                    return Err(());
                }
            };
            let mut remote = HANDLE::default();
            let duplicated = unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    local,
                    self.client_process,
                    &mut remote,
                    0,
                    false,
                    windows::Win32::Foundation::DUPLICATE_SAME_ACCESS,
                )
            };
            unsafe {
                let _ = CloseHandle(local);
            }
            if duplicated.is_err() || remote.is_invalid() {
                self.close_remote_handles(&remote_handles);
                return Err(());
            }
            remote_handles.push(remote);
            handles.push(remote.0 as usize as u64);
            textures.push(texture);
            mutexes.push(mutex);
        }
        let generation = self.next_generation;
        self.next_generation = self.next_generation.saturating_add(1);
        let buffers = (0..POOL_BUFFERS)
            .map(|slot| BufferLayout {
                slot,
                planes: vec![PlaneLayout {
                    stride,
                    offset: 0,
                    size: buffer_size,
                }],
            })
            .collect();
        let message = FrameMessage::PoolCreated {
            document: self.document.clone(),
            pool_generation: generation,
            width: spec.width,
            height: spec.height,
            format: spec.format,
            modifier: 0,
            buffers,
            shared_handles: handles,
        };
        if super::emit_frame(&message).is_err() {
            self.close_remote_handles(&remote_handles);
            self.fail(FrameFailure::CopyFailed, "pool publication failed");
            return Err(());
        }
        Ok(Pool {
            document: self.document.clone(),
            generation,
            width: spec.width,
            height: spec.height,
            format: spec.format,
            textures,
            mutexes,
            slots: [Slot::Free; POOL_BUFFERS as usize],
            ready: false,
            retiring: false,
            retiring_since: None,
        })
    }

    fn open_source(
        &self,
        info: &AcceleratedPaintInfo,
        spec: &PoolSpec,
    ) -> Result<ID3D11Texture2D, (FrameFailure, String)> {
        let source_handle = HANDLE(info.shared_texture_handle);
        if source_handle.is_invalid() {
            return Err((
                FrameFailure::InvalidHandle,
                "CEF returned an invalid D3D11 shared texture handle".to_string(),
            ));
        }
        let source = unsafe {
            self.device1
                .OpenSharedResource1::<ID3D11Texture2D>(source_handle)
        }
        .map_err(|error| {
            (
                FrameFailure::InvalidHandle,
                format!("opening CEF D3D11 shared texture: {error}"),
            )
        })?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { source.GetDesc(&mut desc) };
        if desc.Width != spec.width
            || desc.Height != spec.height
            || desc.Format != format_to_dxgi(spec.format)
            || desc.MipLevels != 1
            || desc.ArraySize != 1
            || desc.SampleDesc.Count != 1
        {
            return Err((
                FrameFailure::WrongDevice,
                format!(
                    "CEF D3D11 texture description does not match the pool: {}x{} {:?}",
                    desc.Width, desc.Height, desc.Format
                ),
            ));
        }
        Ok(source)
    }

    fn active_pool(&self, width: u32, height: u32, format: FrameFormat) -> Option<usize> {
        self.pools.iter().position(|pool| {
            pool.ready
                && !pool.retiring
                && pool.width == width
                && pool.height == height
                && pool.format == format
        })
    }

    fn free_buffer(&self, pool_index: usize) -> Option<u8> {
        self.pools[pool_index]
            .slots
            .iter()
            .position(|slot| *slot == Slot::Free)
            .and_then(|slot| u8::try_from(slot).ok())
    }

    fn pool_ready(&mut self, document: Document, generation: u64) {
        let Some(index) = self
            .pools
            .iter()
            .position(|pool| pool.document == document && pool.generation == generation)
        else {
            return;
        };
        self.pools[index].ready = true;
        if self.pools[index].retiring && self.pool_drained(index) {
            self.retire(index);
        }
    }

    fn pool_rejected(&mut self, document: Document, generation: u64) {
        let Some(index) = self
            .pools
            .iter()
            .position(|pool| pool.document == document && pool.generation == generation)
        else {
            return;
        };
        if self.pools[index]
            .slots
            .iter()
            .any(|slot| *slot != Slot::Free)
        {
            self.fail(
                FrameFailure::RetireTimeout,
                "rejected pool still has an in-flight frame",
            );
            return;
        }
        self.pools.remove(index);
    }

    fn release(&mut self, document: Document, generation: u64, buffer: u8, sequence: u64) {
        let Some(index) = self
            .pools
            .iter()
            .position(|pool| pool.document == document && pool.generation == generation)
        else {
            return;
        };
        let Some(slot) = self.pools[index].slots.get_mut(usize::from(buffer)) else {
            return;
        };
        if *slot != Slot::InFlight(sequence) {
            return;
        }
        *slot = Slot::Free;
        self.pending_frames = self.pending_frames.saturating_sub(1);
        if self.pools[index].retiring && self.pools[index].ready && self.pool_drained(index) {
            self.retire(index);
        }
    }

    fn pool_drained(&self, index: usize) -> bool {
        self.pools[index]
            .slots
            .iter()
            .all(|slot| *slot == Slot::Free)
    }

    fn retire(&mut self, index: usize) {
        let pool = self.pools.remove(index);
        let _ = super::emit_frame(&FrameMessage::PoolRetired {
            document: pool.document,
            pool_generation: pool.generation,
        });
    }

    fn retire_expired(&mut self) {
        for index in (0..self.pools.len()).rev() {
            if self.pools[index].retiring && self.pools[index].ready && self.pool_drained(index) {
                self.retire(index);
            }
        }
        let now = Instant::now();
        let expired = self
            .pools
            .iter()
            .find(|pool| retirement_expired(pool.retiring_since, now))
            .map(|pool| format!("retiring D3D11 pool {} exceeded its ownership deadline: ready={}, slots={:?}, pending={}", pool.generation, pool.ready, pool.slots, self.pending_frames));
        if let Some(detail) = expired {
            self.fail(FrameFailure::RetireTimeout, &detail);
        }
    }

    fn flush_pending_resize(&mut self) {
        let Some(spec) = self.pending_resize.take() else {
            return;
        };
        if self.pools.len() >= 2 {
            self.pending_resize = Some(spec);
            return;
        }
        if self.pools.iter().any(|pool| {
            !pool.retiring
                && pool.width == spec.width
                && pool.height == spec.height
                && pool.format == spec.format
        }) {
            return;
        }
        for pool in &mut self.pools {
            pool.retiring = true;
            pool.retiring_since.get_or_insert_with(Instant::now);
        }
        match self.create_pool(&spec) {
            Ok(pool) => self.pools.push(pool),
            Err(()) => self.fail(
                FrameFailure::WrongDevice,
                "allocating the pending D3D11 pool failed",
            ),
        }
    }

    fn fail(&mut self, reason: FrameFailure, detail: &str) {
        if self.disabled.replace(reason).is_some() {
            return;
        }
        let _ = super::emit_frame(&FrameMessage::Failed {
            document: self.document.clone(),
            reason,
            detail: detail.to_string(),
        });
    }

    fn close_remote_handles(&self, handles: &[HANDLE]) {
        for handle in handles {
            if handle.is_invalid() {
                continue;
            }
            let mut local = HANDLE::default();
            let duplicated = unsafe {
                DuplicateHandle(
                    self.client_process,
                    *handle,
                    GetCurrentProcess(),
                    &mut local,
                    0,
                    false,
                    DUPLICATE_CLOSE_SOURCE,
                )
            };
            if duplicated.is_ok() && !local.is_invalid() {
                unsafe {
                    let _ = CloseHandle(local);
                }
            }
        }
    }
}

impl Drop for Presenter {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.client_process);
        }
    }
}

fn retirement_expired(started: Option<Instant>, now: Instant) -> bool {
    started.is_some_and(|started| {
        now.saturating_duration_since(started) >= Duration::from_millis(RETIRE_DEADLINE_MS)
    })
}

fn pool_spec(info: &AcceleratedPaintInfo) -> Option<PoolSpec> {
    let width = u32::try_from(info.extra.coded_size.width).ok()?;
    let height = u32::try_from(info.extra.coded_size.height).ok()?;
    if width == 0 || height == 0 || width > MAX_POOL_WIDTH || height > MAX_POOL_HEIGHT {
        return None;
    }
    let format = if info.format == ColorType::BGRA_8888 {
        FrameFormat::Bgra8
    } else if info.format == ColorType::RGBA_8888 {
        FrameFormat::Rgba8
    } else {
        return None;
    };
    Some(PoolSpec {
        width,
        height,
        format,
    })
}

fn format_to_dxgi(format: FrameFormat) -> DXGI_FORMAT {
    match format {
        FrameFormat::Bgra8 => DXGI_FORMAT_B8G8R8A8_UNORM,
        FrameFormat::Rgba8 => DXGI_FORMAT_R8G8B8A8_UNORM,
    }
}

fn union(rects: &[Rect], width: u32, height: u32) -> Option<DirtyRect> {
    let mut left = i32::MAX;
    let mut top = i32::MAX;
    let mut right = i32::MIN;
    let mut bottom = i32::MIN;
    for rect in rects {
        if rect.width <= 0 || rect.height <= 0 {
            continue;
        }
        left = left.max(0).min(rect.x.max(0));
        top = top.max(0).min(rect.y.max(0));
        right = right.max((rect.x + rect.width).min(width as i32));
        bottom = bottom.max((rect.y + rect.height).min(height as i32));
    }
    if left >= right || top >= bottom {
        return None;
    }
    Some(DirtyRect {
        x: left as u32,
        y: top as u32,
        width: (right - left) as u32,
        height: (bottom - top) as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_pool_age_does_not_start_retirement_timeout() {
        let now = Instant::now();
        assert!(!retirement_expired(None, now + Duration::from_secs(120)));
        assert!(!retirement_expired(
            Some(now),
            now + Duration::from_millis(999)
        ));
        assert!(retirement_expired(
            Some(now),
            now + Duration::from_millis(1000)
        ));
    }

    #[test]
    fn dirty_rectangles_are_clipped_to_the_owned_texture() {
        let rects = [
            Rect {
                x: -10,
                y: -5,
                width: 50,
                height: 40,
            },
            Rect {
                x: 80,
                y: 70,
                width: 40,
                height: 50,
            },
        ];
        assert_eq!(
            union(&rects, 100, 90),
            Some(DirtyRect {
                x: 0,
                y: 0,
                width: 100,
                height: 90
            })
        );
        assert_eq!(union(&[], 100, 90), None);
    }
}
