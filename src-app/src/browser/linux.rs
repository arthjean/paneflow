use std::collections::BTreeMap;
use std::os::fd::{AsRawFd, IntoRawFd, OwnedFd};
use std::path::PathBuf;
use std::sync::Arc;

use ash::vk;
use gpui::{DevicePixels, ExternalSurface, size};
use gpui_platform::gpui_wgpu::{ExternalSurfaceContext, ExternalWgpuSurface, wgpu};
use paneflow_browser_protocol::{
    BufferLayout, Document, FrameAck, FrameFormat, FrameMessage, POOL_BUFFERS, PlaneLayout,
};

use super::presentation::{FrameIdentity, PoolLayout, TextureImporter};

pub(super) mod external_sync;

pub const DRM_FORMAT_MOD_LINEAR: u64 = 0;

type Vulkan = wgpu::hal::api::Vulkan;

pub struct DmabufImporter {
    context: Arc<ExternalSurfaceContext>,
    explicit_modifiers: bool,
    acquired: BTreeMap<(u64, u8, u64), ExternalSurface>,
    pub device_name: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub extensions: Vec<String>,
    pub render_node: Option<PathBuf>,
}

fn texture_format(format: FrameFormat) -> (vk::Format, wgpu::TextureFormat) {
    match format {
        FrameFormat::Bgra8 => (vk::Format::B8G8R8A8_UNORM, wgpu::TextureFormat::Bgra8Unorm),
        FrameFormat::Rgba8 => (vk::Format::R8G8B8A8_UNORM, wgpu::TextureFormat::Rgba8Unorm),
    }
}

impl DmabufImporter {
    pub fn new(context: Arc<ExternalSurfaceContext>) -> Result<Self, String> {
        let info = context.adapter.get_info();
        if info.backend != wgpu::Backend::Vulkan {
            return Err(format!(
                "the window renders through {:?}; DMA-BUF import needs Vulkan",
                info.backend
            ));
        }
        let (extensions, render_node) = {
            let hal = unsafe { context.device.as_hal::<Vulkan>() }
                .ok_or("the window device exposes no Vulkan handle")?;
            let extensions: Vec<String> = hal
                .enabled_device_extensions()
                .iter()
                .map(|name| name.to_string_lossy().into_owned())
                .collect();
            let instance = hal.shared_instance().raw_instance();
            let physical = hal.raw_physical_device();
            let supported = unsafe { instance.enumerate_device_extension_properties(physical) }
                .unwrap_or_default()
                .iter()
                .any(|extension| {
                    extension
                        .extension_name_as_c_str()
                        .is_ok_and(|name| name == ash::ext::physical_device_drm::NAME)
                });
            let render_node = supported.then(|| {
                let mut drm = vk::PhysicalDeviceDrmPropertiesEXT::default();
                let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut drm);
                unsafe { instance.get_physical_device_properties2(physical, &mut properties) };
                (drm.has_render == vk::TRUE)
                    .then(|| PathBuf::from(format!("/dev/dri/renderD{}", drm.render_minor)))
            });
            (extensions, render_node.flatten())
        };
        let has = |name: &std::ffi::CStr| {
            extensions
                .iter()
                .any(|ext| ext == name.to_str().unwrap_or(""))
        };
        if !has(ash::khr::external_memory_fd::NAME) || !has(ash::ext::external_memory_dma_buf::NAME)
        {
            return Err(format!(
                "{} lacks VK_KHR_external_memory_fd or VK_EXT_external_memory_dma_buf",
                info.name
            ));
        }
        Ok(Self {
            context,
            acquired: BTreeMap::new(),
            explicit_modifiers: has(ash::ext::image_drm_format_modifier::NAME),
            device_name: info.name,
            vendor_id: info.vendor,
            device_id: info.device,
            extensions,
            render_node,
        })
    }

    pub fn pool_importer(&self) -> Self {
        Self {
            context: self.context.clone(),
            explicit_modifiers: self.explicit_modifiers,
            acquired: BTreeMap::new(),
            device_name: self.device_name.clone(),
            vendor_id: self.vendor_id,
            device_id: self.device_id,
            extensions: self.extensions.clone(),
            render_node: self.render_node.clone(),
        }
    }

    pub fn acquire(
        &mut self,
        identity: FrameIdentity,
        surface: ExternalSurface,
    ) -> Result<(), String> {
        let key = (identity.pool_generation, identity.buffer, identity.sequence);
        if self.acquired.contains_key(&key) {
            return Err("duplicate frame acquisition".into());
        }
        let command = external_sync::barrier(
            &self.context,
            std::slice::from_ref(&surface),
            external_sync::Transfer::Acquire,
        )?;
        self.context.queue.submit([command]);
        let retained = surface.clone();
        self.context
            .queue
            .on_submitted_work_done(move || drop(retained));
        self.acquired.insert(key, surface);
        Ok(())
    }

    pub fn release_surfaces(&mut self, acks: &[FrameAck]) -> Vec<ExternalSurface> {
        acks.iter()
            .filter_map(|ack| match ack {
                FrameAck::Release {
                    pool_generation,
                    buffer,
                    sequence,
                    ..
                } => self
                    .acquired
                    .remove(&(*pool_generation, *buffer, *sequence)),
                FrameAck::PoolReady { .. } | FrameAck::PoolRejected { .. } => None,
            })
            .collect()
    }

    pub fn gpu(&self) -> (u32, u32) {
        (self.vendor_id, self.device_id)
    }

    fn import_one(
        &self,
        pool: &PoolLayout,
        plane: &PlaneLayout,
        fd: OwnedFd,
    ) -> Result<ExternalSurface, String> {
        let (vk_format, wgpu_format) = texture_format(pool.format);
        let hal = unsafe { self.context.device.as_hal::<Vulkan>() }
            .ok_or("the window device exposes no Vulkan handle")?;
        let raw = hal.raw_device().clone();
        let instance = hal.shared_instance().raw_instance();
        let physical = hal.raw_physical_device();
        let plane_layouts = [vk::SubresourceLayout {
            offset: plane.offset,
            size: 0,
            row_pitch: u64::from(plane.stride),
            array_pitch: 0,
            depth_pitch: 0,
        }];
        let mut explicit = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
            .drm_format_modifier(pool.modifier)
            .plane_layouts(&plane_layouts);
        let mut external = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let tiling = if self.explicit_modifiers {
            vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT
        } else if pool.modifier == DRM_FORMAT_MOD_LINEAR {
            vk::ImageTiling::LINEAR
        } else {
            return Err(format!(
                "modifier {:#x} needs VK_EXT_image_drm_format_modifier on {}",
                pool.modifier, self.device_name
            ));
        };
        let mut create = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk_format)
            .extent(vk::Extent3D {
                width: pool.width,
                height: pool.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(tiling)
            .usage(vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut external);
        if self.explicit_modifiers {
            create = create.push_next(&mut explicit);
        }
        let image = unsafe { raw.create_image(&create, None) }
            .map_err(|error| format!("vkCreateImage(dmabuf): {error:?}"))?;
        let destroy_image = |raw: &ash::Device| unsafe { raw.destroy_image(image, None) };
        if tiling == vk::ImageTiling::LINEAR {
            let layout = unsafe {
                raw.get_image_subresource_layout(
                    image,
                    vk::ImageSubresource {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        mip_level: 0,
                        array_layer: 0,
                    },
                )
            };
            if layout.row_pitch != u64::from(plane.stride) || layout.offset != plane.offset {
                destroy_image(&raw);
                return Err(format!(
                    "linear layout mismatch: device pitch {} offset {} versus host pitch {} offset {}",
                    layout.row_pitch, layout.offset, plane.stride, plane.offset
                ));
            }
        }
        let requirements = unsafe { raw.get_image_memory_requirements(image) };
        let external_fd = ash::khr::external_memory_fd::Device::new(instance, &raw);
        let mut fd_properties = vk::MemoryFdPropertiesKHR::default();
        if let Err(error) = unsafe {
            external_fd.get_memory_fd_properties(
                vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
                fd.as_raw_fd(),
                &mut fd_properties,
            )
        } {
            destroy_image(&raw);
            return Err(format!("vkGetMemoryFdPropertiesKHR: {error:?}"));
        }
        let memory_properties = unsafe { instance.get_physical_device_memory_properties(physical) };
        let bits = requirements.memory_type_bits & fd_properties.memory_type_bits;
        let memory_type = (0..memory_properties.memory_type_count)
            .filter(|index| bits & (1 << index) != 0)
            .max_by_key(|index| {
                memory_properties.memory_types[*index as usize]
                    .property_flags
                    .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
            });
        let Some(memory_type) = memory_type else {
            destroy_image(&raw);
            return Err("the host DMA-BUF has no memory type on the window device".to_string());
        };
        let mut import = vk::ImportMemoryFdInfoKHR::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
            .fd(fd.into_raw_fd());
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
        let allocate = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type)
            .push_next(&mut import)
            .push_next(&mut dedicated);
        let memory = match unsafe { raw.allocate_memory(&allocate, None) } {
            Ok(memory) => memory,
            Err(error) => {
                unsafe { libc::close(import.fd) };
                destroy_image(&raw);
                return Err(format!("vkAllocateMemory(import): {error:?}"));
            }
        };
        if let Err(error) = unsafe { raw.bind_image_memory(image, memory, 0) } {
            unsafe { raw.free_memory(memory, None) };
            destroy_image(&raw);
            return Err(format!("vkBindImageMemory(import): {error:?}"));
        }
        let extent = wgpu::Extent3d {
            width: pool.width,
            height: pool.height,
            depth_or_array_layers: 1,
        };
        let hal_descriptor = wgpu::hal::TextureDescriptor {
            label: Some("browser-dmabuf"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu_format,
            usage: wgpu::TextureUses::RESOURCE,
            memory_flags: wgpu::hal::MemoryFlags::empty(),
            view_formats: Vec::new(),
        };
        let release_device = raw.clone();
        let drop_callback: wgpu::hal::DropCallback = Box::new(move || unsafe {
            release_device.destroy_image(image, None);
            release_device.free_memory(memory, None);
        });
        let hal_texture = unsafe {
            hal.texture_from_raw(
                image,
                &hal_descriptor,
                Some(drop_callback),
                wgpu::hal::vulkan::TextureMemory::External,
            )
        };
        drop(hal);
        let texture = unsafe {
            self.context.device.create_texture_from_hal::<Vulkan>(
                hal_texture,
                &wgpu::TextureDescriptor {
                    label: Some("browser-dmabuf"),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu_format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
            )
        };
        Ok(ExternalSurface::new(
            pool.generation,
            size(
                DevicePixels(pool.width as i32),
                DevicePixels(pool.height as i32),
            ),
            Arc::new(ExternalWgpuSurface::new(texture)),
        ))
    }
}

impl TextureImporter for DmabufImporter {
    type Texture = ExternalSurface;

    fn requires_initialization(&self) -> bool {
        true
    }

    fn import(
        &mut self,
        pool: &PoolLayout,
        fds: Vec<OwnedFd>,
    ) -> Result<Vec<ExternalSurface>, String> {
        if pool.buffers.iter().any(|buffer| buffer.planes.len() != 1) {
            return Err("multi-plane pool buffers are not supported by the consumer".to_string());
        }
        if fds.len() != pool.buffers.len() {
            return Err(format!(
                "pool {} carries {} descriptors for {} buffers",
                pool.generation,
                fds.len(),
                pool.buffers.len()
            ));
        }
        pool.buffers
            .iter()
            .zip(fds)
            .map(|(buffer, fd)| self.import_one(pool, &buffer.planes[0], fd))
            .collect()
    }
}

pub struct PatternProducer {
    context: Arc<ExternalSurfaceContext>,
    document: Document,
    width: u32,
    height: u32,
    generation: u64,
    sequence: u64,
    pools: BTreeMap<u64, Vec<wgpu::Texture>>,
    held: BTreeMap<(u64, u8), u64>,
    scratch: Vec<u8>,
}

impl PatternProducer {
    pub fn new(context: Arc<ExternalSurfaceContext>, document: Document) -> Self {
        Self {
            context,
            document,
            width: 0,
            height: 0,
            generation: 0,
            sequence: 0,
            pools: BTreeMap::new(),
            held: BTreeMap::new(),
            scratch: Vec::new(),
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Vec<FrameMessage> {
        let width = width.clamp(1, 16384);
        let height = height.clamp(1, 16384);
        if width == self.width && height == self.height {
            return Vec::new();
        }
        let mut messages = Vec::new();
        if self.generation != 0 {
            messages.push(FrameMessage::PoolRetired {
                document: self.document.clone(),
                pool_generation: self.generation,
            });
            self.held
                .retain(|(generation, _), _| *generation != self.generation);
            self.pools.remove(&self.generation);
        }
        self.width = width;
        self.height = height;
        self.generation += 1;
        let stride = width * 4;
        messages.push(FrameMessage::PoolCreated {
            document: self.document.clone(),
            pool_generation: self.generation,
            width,
            height,
            format: FrameFormat::Bgra8,
            modifier: DRM_FORMAT_MOD_LINEAR,
            shared_handles: Vec::new(),
            buffers: (0..POOL_BUFFERS)
                .map(|slot| BufferLayout {
                    slot,
                    planes: vec![PlaneLayout {
                        stride,
                        offset: 0,
                        size: u64::from(stride) * u64::from(height),
                    }],
                })
                .collect(),
        });
        messages
    }

    pub fn produce(&mut self, now_ns: u64) -> Option<FrameMessage> {
        let textures = self.pools.get(&self.generation)?;
        let slot =
            (0..POOL_BUFFERS).find(|slot| !self.held.contains_key(&(self.generation, *slot)))?;
        let texture = textures.get(usize::from(slot))?;
        self.sequence += 1;
        let sequence = self.sequence;
        let stride = self.width as usize * 4;
        self.scratch.resize(stride * self.height as usize, 0);
        let phase = (sequence % 240) as u32;
        for y in 0..self.height as usize {
            for x in 0..self.width as usize {
                let offset = y * stride + x * 4;
                let diagonal = ((x as u32 + y as u32 + phase * 4) / 32).is_multiple_of(2);
                let (b, g, r) = if diagonal {
                    (200, 120 + (phase % 120) as u8, 40)
                } else {
                    (30, 30, 30)
                };
                self.scratch[offset] = b;
                self.scratch[offset + 1] = g;
                self.scratch[offset + 2] = r;
                self.scratch[offset + 3] = 255;
            }
        }
        self.context.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.scratch,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride as u32),
                rows_per_image: Some(self.height),
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        self.held.insert((self.generation, slot), sequence);
        Some(FrameMessage::Frame {
            document: self.document.clone(),
            pool_generation: self.generation,
            buffer: slot,
            sequence,
            callback_ns: now_ns,
            ready_ns: now_ns,
            capture_timestamp_us: now_ns / 1000,
            capture_counter: None,
            dirty: None,
        })
    }

    pub fn release(&mut self, ack: &FrameAck) {
        let FrameAck::Release {
            pool_generation,
            buffer,
            sequence,
            ..
        } = ack
        else {
            return;
        };
        if self.held.get(&(*pool_generation, *buffer)) == Some(sequence) {
            self.held.remove(&(*pool_generation, *buffer));
        }
    }
}

impl TextureImporter for PatternProducer {
    type Texture = ExternalSurface;

    fn import(
        &mut self,
        pool: &PoolLayout,
        _fds: Vec<OwnedFd>,
    ) -> Result<Vec<ExternalSurface>, String> {
        let (_, format) = texture_format(pool.format);
        let mut textures = Vec::with_capacity(pool.buffers.len());
        let mut surfaces = Vec::with_capacity(pool.buffers.len());
        for _ in &pool.buffers {
            let texture = self
                .context
                .device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("browser-pattern"),
                    size: wgpu::Extent3d {
                        width: pool.width,
                        height: pool.height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
            surfaces.push(ExternalSurface::new(
                pool.generation,
                size(
                    DevicePixels(pool.width as i32),
                    DevicePixels(pool.height as i32),
                ),
                Arc::new(ExternalWgpuSurface::new(texture.clone())),
            ));
            textures.push(texture);
        }
        self.pools.insert(pool.generation, textures);
        Ok(surfaces)
    }
}
