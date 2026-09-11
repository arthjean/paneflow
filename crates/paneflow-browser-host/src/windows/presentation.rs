use std::time::{Duration, Instant};

use cef::{AcceleratedPaintInfo, ColorType, PaintElementType, Rect};
use paneflow_browser_protocol::{
    BufferLayout, DirtyRect, Document, FrameAck, FrameFailure, FrameFormat, FrameMessage,
    PlaneLayout, MAX_PENDING_FRAMES, POOL_BUFFERS, RETIRE_DEADLINE_MS,
};
use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, DuplicateHandle, DUPLICATE_CLOSE_SOURCE, HANDLE, LUID,
};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11Device1, ID3D11DeviceContext, ID3D11Resource,
    ID3D11Texture2D, D3D11_BIND_SHADER_RESOURCE, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX, D3D11_RESOURCE_MISC_SHARED_NTHANDLE, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter, IDXGIDevice, IDXGIFactory2, IDXGIFactory4, IDXGIKeyedMutex,
    IDXGIResource1, DXGI_SHARED_RESOURCE_READ,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcess, PROCESS_DUP_HANDLE};

const ADAPTER_LUID_ENV: &str = "PANEFLOW_BROWSER_ADAPTER_LUID";

fn adapter_name(units: &[u16]) -> String {
    units
        .iter()
        .take_while(|unit| **unit != 0)
        .map(|unit| char::from_u32(u32::from(*unit)).unwrap_or('?'))
        .collect()
}

fn parse_adapter_luid(value: &str) -> Option<LUID> {
    let (high, low) = value.split_once(',')?;
    Some(LUID {
        HighPart: high.trim().parse().ok()?,
        LowPart: low.trim().parse().ok()?,
    })
}

pub(super) fn requested_adapter_luid() -> Option<LUID> {
    parse_adapter_luid(std::env::var(ADAPTER_LUID_ENV).ok()?.as_str())
}

pub(super) fn requested_adapter_switch() -> Option<String> {
    requested_adapter_luid().map(|luid| format!("{},{}", luid.HighPart, luid.LowPart))
}

fn adapter_inventory(factory: &IDXGIFactory4) -> String {
    let mut adapters = Vec::new();
    for index in 0.. {
        let Ok(adapter) = (unsafe { factory.EnumAdapters(index) }) else {
            break;
        };
        let Ok(description) = (unsafe { adapter.GetDesc() }) else {
            continue;
        };
        adapters.push(format!(
            "{} ({:x}-{:x})",
            adapter_name(&description.Description),
            description.AdapterLuid.HighPart,
            description.AdapterLuid.LowPart
        ));
    }
    if adapters.is_empty() {
        "none".to_string()
    } else {
        adapters.join(", ")
    }
}

fn adapter_by_luid(luid: LUID) -> Result<IDXGIAdapter, String> {
    let factory: IDXGIFactory4 = unsafe { CreateDXGIFactory1() }
        .map_err(|error| format!("enumerating the Windows graphics adapters: {error}"))?;
    unsafe { factory.EnumAdapterByLuid(luid) }.map_err(|error| {
        format!(
            "the application draws on adapter {:x}-{:x} and the browser host cannot open it ({error}); the host sees {}",
            luid.HighPart,
            luid.LowPart,
            adapter_inventory(&factory)
        )
    })
}

fn shared_texture_adapter(handle: HANDLE) -> Option<LUID> {
    let factory: IDXGIFactory2 = unsafe { CreateDXGIFactory1() }.ok()?;
    unsafe { factory.GetSharedResourceAdapterLuid(handle) }.ok()
}

fn emit_adapter_identity(device: &ID3D11Device, pinned: bool) {
    let Ok(dxgi) = device.cast::<IDXGIDevice>() else {
        return;
    };
    let Ok(adapter) = (unsafe { dxgi.GetAdapter() }) else {
        return;
    };
    let Ok(description) = (unsafe { adapter.GetDesc() }) else {
        return;
    };
    super::emit_adapter(
        &adapter_name(&description.Description),
        description.VendorId,
        description.DeviceId,
        description.DedicatedVideoMemory as u64,
        i64::from(description.AdapterLuid.HighPart),
        description.AdapterLuid.LowPart,
        pinned,
    );
}

const MAX_POOL_WIDTH: u32 = 16_384;
const MAX_POOL_HEIGHT: u32 = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Free,
    InFlight(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Retirement {
    Active,
    AwaitingReplacement,
    AwaitingRelease(Instant),
}

impl Retirement {
    fn superseded(self) -> bool {
        !matches!(self, Self::Active)
    }

    fn begin_release(&mut self, now: Instant) -> bool {
        if matches!(self, Self::AwaitingReplacement) {
            *self = Self::AwaitingRelease(now);
            true
        } else {
            false
        }
    }

    fn expired(self, now: Instant) -> bool {
        matches!(self, Self::AwaitingRelease(since)
            if now.saturating_duration_since(since) >= Duration::from_millis(RETIRE_DEADLINE_MS))
    }
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
    retirement: Retirement,
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
    expected_size: Option<(u32, u32)>,
    next_generation: u64,
    next_sequence: u64,
    pending_frames: usize,
    slot_starved: bool,
    published_size: Option<(u32, u32)>,
    disabled: Option<FrameFailure>,
}

impl Presenter {
    pub fn new(document: Document, client_pid: u32) -> Result<Self, String> {
        if client_pid == 0 {
            return Err("browser client process id is invalid".to_string());
        }
        let adapter = match requested_adapter_luid() {
            Some(luid) => Some(adapter_by_luid(luid)?),
            None => None,
        };
        let client_process = unsafe { OpenProcess(PROCESS_DUP_HANDLE, false, client_pid) }
            .map_err(|error| format!("opening browser client for handle duplication: {error}"))?;
        let mut device = None;
        let mut context = None;
        let result = unsafe {
            D3D11CreateDevice(
                adapter.as_ref(),
                if adapter.is_some() {
                    D3D_DRIVER_TYPE_UNKNOWN
                } else {
                    D3D_DRIVER_TYPE_HARDWARE
                },
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
        emit_adapter_identity(&device, adapter.is_some());
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
            expected_size: None,
            next_generation: 1,
            next_sequence: 1,
            pending_frames: 0,
            slot_starved: false,
            published_size: None,
            disabled: None,
        })
    }

    pub fn update_document(&mut self, document: Document) {
        self.document = document;
        self.supersede_pools(false);
        self.retire_drained();
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn prepare_resize(&mut self, width: u32, height: u32) {
        self.expected_size = Some((width, height));
        if self.disabled.is_some() {
            return;
        }
        let Some(pool) = self.pools.iter().find(|pool| !pool.retirement.superseded()) else {
            return;
        };
        let spec = PoolSpec {
            width,
            height,
            format: pool.format,
        };
        if self.ensure_pool(&spec).is_err() {
            self.fail(
                FrameFailure::CopyFailed,
                "preparing the D3D11 resize pool failed",
            );
        }
    }

    pub fn handle_ack(&mut self, ack: FrameAck) -> bool {
        let outstanding = self.pending_frames;
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
        self.disabled.is_none()
            && self.pending_frames < outstanding
            && std::mem::take(&mut self.slot_starved)
    }

    pub fn paint(
        &mut self,
        type_: PaintElementType,
        dirty_rects: Option<&[Rect]>,
        info: &AcceleratedPaintInfo,
    ) -> bool {
        let callback_ns = super::now_ns();
        if type_ != PaintElementType::VIEW || self.disabled.is_some() {
            return false;
        }
        self.retire_expired();
        let Some(spec) = pool_spec(info) else {
            self.fail(
                FrameFailure::UnsupportedFormat,
                "CEF returned an unsupported texture format",
            );
            return false;
        };
        if self
            .expected_size
            .is_some_and(|size| size != (spec.width, spec.height))
        {
            self.probe_capture("stale_geometry", &spec, callback_ns, capture_identity(info));
            return false;
        }
        if self.ensure_pool(&spec).is_err() {
            self.fail(
                FrameFailure::CopyFailed,
                "creating the D3D11 frame pool failed",
            );
            return false;
        }
        if self.disabled.is_some() {
            return false;
        }
        if self.pending_frames > MAX_PENDING_FRAMES {
            self.slot_starved = true;
            self.probe("pending_limit", &spec, callback_ns);
            return false;
        }
        let Some(pool_index) = self.active_pool(spec.width, spec.height, spec.format) else {
            self.probe("pool_pending", &spec, callback_ns);
            return false;
        };
        let Some(buffer) = self.free_buffer(pool_index) else {
            self.slot_starved = true;
            self.probe("slot_starved", &spec, callback_ns);
            return false;
        };
        let source = match self.open_source(info, &spec) {
            Ok(source) => source,
            Err((reason, detail)) => {
                self.fail(reason, &detail);
                return false;
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
                return false;
            }
        };
        let destination_resource = match destination.cast::<ID3D11Resource>() {
            Ok(resource) => resource,
            Err(error) => {
                self.fail(
                    FrameFailure::WrongDevice,
                    &format!("owned destination texture is not a D3D11 resource: {error}"),
                );
                return false;
            }
        };
        let mutex = &self.pools[pool_index].mutexes[usize::from(buffer)];
        let acquired = unsafe { (mutex.vtable().AcquireSync)(mutex.as_raw(), 0, 0) };
        if acquired.0 == 258 {
            self.probe("acquire_busy", &spec, callback_ns);
            return false;
        }
        if acquired.0 != 0 {
            self.fail(
                FrameFailure::WrongDevice,
                "owned texture synchronization failed",
            );
            return false;
        }
        unsafe {
            self.context
                .CopyResource(&destination_resource, &source_resource);
            self.context.Flush();
        }
        if unsafe { mutex.ReleaseSync(0) }.is_err() {
            self.fail(FrameFailure::WrongDevice, "owned texture release failed");
            return false;
        }
        if let Err(error) = unsafe { self.device.GetDeviceRemovedReason() } {
            self.fail(
                FrameFailure::WrongDevice,
                &format!("D3D11 device was removed during frame copy: {error}"),
            );
            return false;
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
            return false;
        }
        self.probe_capture("published", &spec, callback_ns, capture_identity(info));
        self.published_size = Some((spec.width, spec.height));
        self.begin_retirements();
        self.slot_starved = false;
        true
    }

    fn probe(&self, outcome: &str, spec: &PoolSpec, callback_ns: u64) {
        self.probe_capture(outcome, spec, callback_ns, None)
    }

    fn probe_capture(
        &self,
        outcome: &str,
        spec: &PoolSpec,
        callback_ns: u64,
        capture: Option<(u64, Option<u64>)>,
    ) {
        if !super::paint_probe_enabled()
            || self.expected_size.is_none()
            || self.expected_size == self.published_size
        {
            return;
        }
        let pools = self
            .pools
            .iter()
            .map(|pool| {
                serde_json::json!({"generation":pool.generation,"width":pool.width,
                    "height":pool.height,"ready":pool.ready,
                    "superseded":pool.retirement.superseded(),
                    "free":pool.slots.iter().filter(|slot| **slot == Slot::Free).count()})
            })
            .collect::<Vec<_>>();
        super::emit(
            serde_json::json!({"native":"paint_probe", "document":self.document,
            "at_ns":super::now_ns(), "callback_ns":callback_ns, "outcome":outcome,
            "coded":[spec.width, spec.height],
            "expected":self.expected_size.map(|(width, height)| [width, height]),
            "pending_frames":self.pending_frames, "pools":pools,
            "capture_timestamp_us":capture.map(|(timestamp, _)| timestamp / 1_000),
            "capture_counter":capture.and_then(|(_, counter)| counter)}),
        );
    }

    fn ensure_pool(&mut self, spec: &PoolSpec) -> Result<(), ()> {
        if self.pools.iter().any(|pool| {
            !pool.retirement.superseded()
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
        self.supersede_pools(true);
        self.retire_drained();
        let pool = self.create_pool(spec)?;
        self.pools.push(pool);
        Ok(())
    }

    fn create_pool(&mut self, spec: &PoolSpec) -> Result<Pool, ()> {
        let started_ns = super::now_ns();
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
        if super::benchmark_enabled() {
            super::emit(
                serde_json::json!({"native":"resize_pool", "document":self.document,
                "at_ns":super::now_ns(), "generation":generation,
                "width":spec.width, "height":spec.height}),
            );
        }
        super::record_span("create_pool", started_ns, 5);
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
            retirement: Retirement::Active,
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
            let origin = match shared_texture_adapter(source_handle) {
                Some(luid) => format!(
                    "; that texture belongs to adapter {:x}-{:x}",
                    luid.HighPart, luid.LowPart
                ),
                None => String::new(),
            };
            (
                FrameFailure::InvalidHandle,
                format!("opening CEF D3D11 shared texture: {error}{origin}"),
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
                && !pool.retirement.superseded()
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
        if super::paint_probe_enabled() {
            super::emit(
                serde_json::json!({"native":"pool_ready", "document":self.document,
                "at_ns":super::now_ns(), "generation":generation,
                "width":self.pools[index].width, "height":self.pools[index].height}),
            );
        }
        if self.pools[index].retirement.superseded() && self.pool_drained(index) {
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
        if self.pools[index].retirement.superseded()
            && self.pools[index].ready
            && self.pool_drained(index)
        {
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
        let started_ns = super::now_ns();
        let pool = self.pools.remove(index);
        let _ = super::emit_frame(&FrameMessage::PoolRetired {
            document: pool.document,
            pool_generation: pool.generation,
        });
        super::record_span("retire_pool", started_ns, 5);
    }

    fn supersede_pools(&mut self, await_replacement: bool) {
        let now = Instant::now();
        for pool in &mut self.pools {
            if !pool.retirement.superseded() {
                pool.retirement = Retirement::AwaitingReplacement;
            }
            if !await_replacement {
                pool.retirement.begin_release(now);
            }
        }
    }

    fn begin_retirements(&mut self) {
        let now = Instant::now();
        for pool in &mut self.pools {
            pool.retirement.begin_release(now);
        }
    }

    fn retire_drained(&mut self) {
        for index in (0..self.pools.len()).rev() {
            if self.pools[index].retirement.superseded()
                && self.pools[index].ready
                && self.pool_drained(index)
            {
                self.retire(index);
            }
        }
    }

    fn retire_expired(&mut self) {
        self.retire_drained();
        let now = Instant::now();
        let expired = self
            .pools
            .iter()
            .find(|pool| pool.retirement.expired(now))
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
            !pool.retirement.superseded()
                && pool.width == spec.width
                && pool.height == spec.height
                && pool.format == spec.format
        }) {
            return;
        }
        self.supersede_pools(true);
        self.retire_drained();
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

fn capture_identity(info: &AcceleratedPaintInfo) -> Option<(u64, Option<u64>)> {
    Some((
        info.extra.timestamp,
        (info.extra.has_capture_counter != 0).then_some(info.extra.capture_counter),
    ))
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
    fn the_adapter_pin_reads_the_chromium_switch_shape() {
        let luid = parse_adapter_luid("-2,70274").expect("a high and low pair");
        assert_eq!((luid.HighPart, luid.LowPart), (-2, 70274));
        assert_eq!(
            parse_adapter_luid(" 0 , 65539 ").map(|luid| (luid.HighPart, luid.LowPart)),
            Some((0, 65539))
        );
        assert!(parse_adapter_luid("70274").is_none());
        assert!(parse_adapter_luid("0,-1").is_none());
        assert!(parse_adapter_luid("").is_none());
    }

    #[test]
    fn a_displayed_pool_waits_for_its_replacement_before_the_release_deadline_starts() {
        let resize = Instant::now();
        let replacement = resize + Duration::from_secs(30);
        let mut retirement = Retirement::Active;
        assert!(!retirement.superseded());
        assert!(!retirement.expired(replacement));
        retirement = Retirement::AwaitingReplacement;
        assert!(retirement.superseded());
        assert!(!retirement.expired(replacement));
        assert!(retirement.begin_release(replacement));
        assert!(!retirement.expired(replacement + Duration::from_millis(RETIRE_DEADLINE_MS - 1)));
        assert!(retirement.expired(replacement + Duration::from_millis(RETIRE_DEADLINE_MS)));
        assert!(!retirement.begin_release(replacement + Duration::from_millis(100)));
        assert!(retirement.expired(replacement + Duration::from_millis(RETIRE_DEADLINE_MS)));
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
