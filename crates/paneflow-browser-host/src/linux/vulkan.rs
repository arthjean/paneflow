use std::ffi::CStr;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};

use ash::vk;
use paneflow_browser_protocol::{FrameFailure, FrameFormat, PlaneLayout};

pub(super) const DRM_FORMAT_MOD_LINEAR: u64 = 0;
const COPY_TIMEOUT_NS: u64 = 1_000_000_000;

#[derive(Debug)]
pub(super) struct Failure {
    pub reason: FrameFailure,
    pub detail: String,
}

impl Failure {
    fn new(reason: FrameFailure, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: detail.into(),
        }
    }
}

fn vulkan_failure(operation: &str) -> impl Fn(vk::Result) -> Failure + '_ {
    move |error| Failure::new(FrameFailure::CopyFailed, format!("{operation}: {error:?}"))
}

pub(super) struct ExportedImage {
    pub image: vk::Image,
    pub memory: vk::DeviceMemory,
    pub fd: Option<OwnedFd>,
    pub layout: PlaneLayout,
}

pub(super) struct ImportSource<'a> {
    pub fd: RawFd,
    pub planes: &'a [PlaneLayout],
    pub modifier: u64,
    pub format: FrameFormat,
    pub coded_width: u32,
    pub coded_height: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

pub(super) struct Engine {
    _entry: ash::Entry,
    instance: ash::Instance,
    physical: vk::PhysicalDevice,
    device: ash::Device,
    queue: vk::Queue,
    queue_family: u32,
    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    fence: vk::Fence,
    memory_properties: vk::PhysicalDeviceMemoryProperties,
    external_fd: ash::khr::external_memory_fd::Device,
    foreign_family: u32,
    pub vendor_id: u32,
    pub device_id: u32,
    pub device_name: String,
    pub extensions: Vec<String>,
}

fn format_of(format: FrameFormat) -> vk::Format {
    match format {
        FrameFormat::Bgra8 => vk::Format::B8G8R8A8_UNORM,
        FrameFormat::Rgba8 => vk::Format::R8G8B8A8_UNORM,
    }
}

impl Engine {
    pub fn new(expected_gpu: Option<(u32, u32)>) -> Result<Self, Failure> {
        let entry = unsafe { ash::Entry::load() }.map_err(|error| {
            Failure::new(FrameFailure::CopyFailed, format!("vulkan loader: {error}"))
        })?;
        let application = vk::ApplicationInfo::default()
            .application_name(c"paneflow-browser-host")
            .api_version(vk::make_api_version(0, 1, 2, 0));
        let instance = unsafe {
            entry.create_instance(
                &vk::InstanceCreateInfo::default().application_info(&application),
                None,
            )
        }
        .map_err(vulkan_failure("create_instance"))?;
        let physical_devices = unsafe { instance.enumerate_physical_devices() }
            .map_err(vulkan_failure("enumerate_physical_devices"))?;
        let mut selected = None;
        let mut seen = Vec::new();
        for physical in physical_devices {
            let properties = unsafe { instance.get_physical_device_properties(physical) };
            let name = properties
                .device_name_as_c_str()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            seen.push(format!(
                "{name} {:#06x}:{:#06x}",
                properties.vendor_id, properties.device_id
            ));
            let discrete = properties.device_type == vk::PhysicalDeviceType::DISCRETE_GPU;
            let matches = match expected_gpu {
                Some((vendor, device)) => {
                    properties.vendor_id == vendor && properties.device_id == device
                }
                None => true,
            };
            let replace = selected.as_ref().is_none_or(
                |(_, current, _): &(_, vk::PhysicalDeviceProperties, _)| {
                    expected_gpu.is_none()
                        && discrete
                        && current.device_type != vk::PhysicalDeviceType::DISCRETE_GPU
                },
            );
            if matches && replace {
                selected = Some((physical, properties, name));
            }
        }
        let Some((physical, properties, device_name)) = selected else {
            return Err(Failure::new(
                FrameFailure::WrongDevice,
                format!("no Vulkan device matches {expected_gpu:?}; available: {seen:?}"),
            ));
        };
        let available = unsafe { instance.enumerate_device_extension_properties(physical) }
            .map_err(vulkan_failure("enumerate_device_extension_properties"))?;
        let has = |name: &CStr| {
            available.iter().any(|extension| {
                extension
                    .extension_name_as_c_str()
                    .is_ok_and(|extension_name| extension_name == name)
            })
        };
        let required: [&CStr; 4] = [
            ash::khr::external_memory_fd::NAME,
            ash::ext::external_memory_dma_buf::NAME,
            ash::ext::image_drm_format_modifier::NAME,
            ash::ext::queue_family_foreign::NAME,
        ];
        for name in required {
            if !has(name) {
                return Err(Failure::new(
                    FrameFailure::UnsupportedModifier,
                    format!("{} lacks {}", device_name, name.to_string_lossy()),
                ));
            }
        }
        let mut enabled: Vec<&CStr> = required.to_vec();
        let foreign_family = vk::QUEUE_FAMILY_FOREIGN_EXT;
        for optional in [
            ash::khr::image_format_list::NAME,
            ash::khr::bind_memory2::NAME,
            ash::khr::sampler_ycbcr_conversion::NAME,
        ] {
            if has(optional) {
                enabled.push(optional);
            }
        }
        let families = unsafe { instance.get_physical_device_queue_family_properties(physical) };
        let queue_family = families
            .iter()
            .position(|family| family.queue_flags.contains(vk::QueueFlags::GRAPHICS))
            .ok_or_else(|| Failure::new(FrameFailure::CopyFailed, "no graphics queue family"))?
            as u32;
        let priorities = [1.0_f32];
        let queue_info = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family)
            .queue_priorities(&priorities)];
        let enabled_pointers: Vec<*const std::os::raw::c_char> =
            enabled.iter().map(|name| name.as_ptr()).collect();
        let device = unsafe {
            instance.create_device(
                physical,
                &vk::DeviceCreateInfo::default()
                    .queue_create_infos(&queue_info)
                    .enabled_extension_names(&enabled_pointers),
                None,
            )
        }
        .map_err(vulkan_failure("create_device"))?;
        let queue = unsafe { device.get_device_queue(queue_family, 0) };
        let command_pool = unsafe {
            device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(queue_family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )
        }
        .map_err(vulkan_failure("create_command_pool"))?;
        let command_buffer = unsafe {
            device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
        }
        .map_err(vulkan_failure("allocate_command_buffers"))?[0];
        let fence = unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) }
            .map_err(vulkan_failure("create_fence"))?;
        let memory_properties = unsafe { instance.get_physical_device_memory_properties(physical) };
        let external_fd = ash::khr::external_memory_fd::Device::new(&instance, &device);
        Ok(Self {
            _entry: entry,
            instance,
            physical,
            device,
            queue,
            queue_family,
            command_pool,
            command_buffer,
            fence,
            memory_properties,
            external_fd,
            foreign_family,
            vendor_id: properties.vendor_id,
            device_id: properties.device_id,
            device_name,
            extensions: enabled
                .iter()
                .map(|name| name.to_string_lossy().into_owned())
                .collect(),
        })
    }

    fn memory_type(&self, bits: u32, flags: vk::MemoryPropertyFlags) -> Option<u32> {
        (0..self.memory_properties.memory_type_count).find(|index| {
            bits & (1 << index) != 0
                && self.memory_properties.memory_types[*index as usize]
                    .property_flags
                    .contains(flags)
        })
    }

    fn modifier_supports(
        &self,
        format: vk::Format,
        modifier: u64,
        usage: vk::ImageUsageFlags,
        needs_export: bool,
    ) -> bool {
        let mut modifier_info = vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::default()
            .drm_format_modifier(modifier)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let mut external_info = vk::PhysicalDeviceExternalImageFormatInfo::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let info = vk::PhysicalDeviceImageFormatInfo2::default()
            .format(format)
            .ty(vk::ImageType::TYPE_2D)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(usage)
            .push_next(&mut modifier_info)
            .push_next(&mut external_info);
        let mut external_properties = vk::ExternalImageFormatProperties::default();
        let mut properties =
            vk::ImageFormatProperties2::default().push_next(&mut external_properties);
        let supported = unsafe {
            self.instance.get_physical_device_image_format_properties2(
                self.physical,
                &info,
                &mut properties,
            )
        }
        .is_ok();
        let features = external_properties
            .external_memory_properties
            .external_memory_features;
        supported
            && features.contains(vk::ExternalMemoryFeatureFlags::IMPORTABLE)
            && (!needs_export || features.contains(vk::ExternalMemoryFeatureFlags::EXPORTABLE))
    }

    pub fn choose_export_modifier(&self, format: FrameFormat) -> Result<u64, Failure> {
        let vk_format = format_of(format);
        let mut modifier_list = vk::DrmFormatModifierPropertiesListEXT::default();
        let mut properties = vk::FormatProperties2::default().push_next(&mut modifier_list);
        unsafe {
            self.instance.get_physical_device_format_properties2(
                self.physical,
                vk_format,
                &mut properties,
            )
        };
        let count = modifier_list.drm_format_modifier_count as usize;
        let mut entries = vec![vk::DrmFormatModifierPropertiesEXT::default(); count];
        let mut modifier_list = vk::DrmFormatModifierPropertiesListEXT::default()
            .drm_format_modifier_properties(&mut entries);
        let mut properties = vk::FormatProperties2::default().push_next(&mut modifier_list);
        unsafe {
            self.instance.get_physical_device_format_properties2(
                self.physical,
                vk_format,
                &mut properties,
            )
        };
        let usage = vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED;
        let needed = vk::FormatFeatureFlags::TRANSFER_DST | vk::FormatFeatureFlags::SAMPLED_IMAGE;
        let candidates: Vec<u64> = entries
            .iter()
            .filter(|entry| {
                entry.drm_format_modifier_plane_count == 1
                    && entry.drm_format_modifier_tiling_features.contains(needed)
            })
            .map(|entry| entry.drm_format_modifier)
            .collect();
        for modifier in std::iter::once(DRM_FORMAT_MOD_LINEAR).chain(candidates.iter().copied()) {
            if candidates.contains(&modifier)
                && self.modifier_supports(vk_format, modifier, usage, true)
            {
                return Ok(modifier);
            }
        }
        Err(Failure::new(
            FrameFailure::UnsupportedModifier,
            format!(
                "{} exports no single-plane DMA-BUF modifier for {format:?}; candidates {candidates:?}",
                self.device_name
            ),
        ))
    }

    pub fn create_exported_images(
        &self,
        width: u32,
        height: u32,
        format: FrameFormat,
        modifier: u64,
        count: usize,
    ) -> Result<Vec<ExportedImage>, Failure> {
        let mut images = Vec::with_capacity(count);
        for _ in 0..count {
            match self.allocate_exported_image(width, height, format, modifier) {
                Ok(image) => images.push(image),
                Err(failure) => {
                    for image in images {
                        self.destroy_exported_image(image);
                    }
                    return Err(failure);
                }
            }
        }
        if let Err(failure) = self.initialize_exports(&images) {
            for image in images {
                self.destroy_exported_image(image);
            }
            return Err(failure);
        }
        Ok(images)
    }

    fn allocate_exported_image(
        &self,
        width: u32,
        height: u32,
        format: FrameFormat,
        modifier: u64,
    ) -> Result<ExportedImage, Failure> {
        let modifiers = [modifier];
        let mut modifier_list =
            vk::ImageDrmFormatModifierListCreateInfoEXT::default().drm_format_modifiers(&modifiers);
        let mut external = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let create = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format_of(format))
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut modifier_list)
            .push_next(&mut external);
        let image = unsafe { self.device.create_image(&create, None) }
            .map_err(vulkan_failure("create_image(export)"))?;
        let requirements = unsafe { self.device.get_image_memory_requirements(image) };
        let Some(memory_type) = self
            .memory_type(
                requirements.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
            .or_else(|| {
                self.memory_type(
                    requirements.memory_type_bits,
                    vk::MemoryPropertyFlags::empty(),
                )
            })
        else {
            unsafe { self.device.destroy_image(image, None) };
            return Err(Failure::new(
                FrameFailure::CopyFailed,
                "no memory type for exported image",
            ));
        };
        let mut export = vk::ExportMemoryAllocateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
        let allocate = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type)
            .push_next(&mut export)
            .push_next(&mut dedicated);
        let memory = match unsafe { self.device.allocate_memory(&allocate, None) } {
            Ok(memory) => memory,
            Err(error) => {
                unsafe { self.device.destroy_image(image, None) };
                return Err(vulkan_failure("allocate_memory(export)")(error));
            }
        };
        if let Err(error) = unsafe { self.device.bind_image_memory(image, memory, 0) } {
            unsafe {
                self.device.free_memory(memory, None);
                self.device.destroy_image(image, None);
            }
            return Err(vulkan_failure("bind_image_memory(export)")(error));
        }
        let fd = match unsafe {
            self.external_fd.get_memory_fd(
                &vk::MemoryGetFdInfoKHR::default()
                    .memory(memory)
                    .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT),
            )
        } {
            Ok(fd) => unsafe { OwnedFd::from_raw_fd(fd) },
            Err(error) => {
                unsafe {
                    self.device.destroy_image(image, None);
                    self.device.free_memory(memory, None);
                }
                return Err(vulkan_failure("get_memory_fd")(error));
            }
        };
        let subresource = unsafe {
            self.device.get_image_subresource_layout(
                image,
                vk::ImageSubresource {
                    aspect_mask: vk::ImageAspectFlags::MEMORY_PLANE_0_EXT,
                    mip_level: 0,
                    array_layer: 0,
                },
            )
        };
        Ok(ExportedImage {
            image,
            memory,
            fd: Some(fd),
            layout: PlaneLayout {
                stride: subresource.row_pitch as u32,
                offset: subresource.offset,
                size: requirements.size,
            },
        })
    }

    pub fn destroy_exported_image(&self, image: ExportedImage) {
        unsafe {
            self.device.destroy_image(image.image, None);
            self.device.free_memory(image.memory, None);
        }
    }

    pub fn copy_frame(
        &self,
        source: &ImportSource<'_>,
        target: &ExportedImage,
    ) -> Result<u64, Failure> {
        let link = std::fs::read_link(format!("/proc/self/fd/{}", source.fd))
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !link.contains("dmabuf") {
            return Err(Failure::new(
                FrameFailure::InvalidHandle,
                format!(
                    "the CEF descriptor is not a DMA-BUF ({link}); the GPU process renders in software"
                ),
            ));
        }
        let buffer_size = unsafe { libc::lseek(source.fd, 0, libc::SEEK_END) };
        let reset = unsafe { libc::lseek(source.fd, 0, libc::SEEK_SET) };
        if buffer_size <= 0 || reset != 0 {
            return Err(Failure::new(
                FrameFailure::InvalidHandle,
                "the kernel did not provide a valid DMA-BUF allocation size",
            ));
        }
        let buffer_size = buffer_size as u64;
        if source.planes.is_empty()
            || source.planes.iter().any(|plane| {
                plane.size == 0
                    || plane.stride == 0
                    || plane.offset >= buffer_size
                    || plane.size > buffer_size - plane.offset
            })
        {
            return Err(Failure::new(
                FrameFailure::InvalidHandle,
                "the CEF plane metadata exceeds the DMA-BUF allocation",
            ));
        }
        let format = format_of(source.format);
        if !self.modifier_supports(
            format,
            source.modifier,
            vk::ImageUsageFlags::TRANSFER_SRC,
            false,
        ) {
            return Err(Failure::new(
                FrameFailure::UnsupportedModifier,
                format!(
                    "{} cannot import modifier {:#x}",
                    self.device_name, source.modifier
                ),
            ));
        }
        let plane_layouts: Vec<vk::SubresourceLayout> = source
            .planes
            .iter()
            .map(|plane| vk::SubresourceLayout {
                offset: plane.offset,
                size: 0,
                row_pitch: u64::from(plane.stride),
                array_pitch: 0,
                depth_pitch: 0,
            })
            .collect();
        let mut explicit = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
            .drm_format_modifier(source.modifier)
            .plane_layouts(&plane_layouts);
        let mut external = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let create = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width: source.coded_width,
                height: source.coded_height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(vk::ImageUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut explicit)
            .push_next(&mut external);
        let image = unsafe { self.device.create_image(&create, None) }
            .map_err(vulkan_failure("create_image(import)"))?;
        let duplicated = unsafe { libc::fcntl(source.fd, libc::F_DUPFD_CLOEXEC, 0) };
        if duplicated < 0 {
            unsafe { self.device.destroy_image(image, None) };
            return Err(Failure::new(
                FrameFailure::InvalidHandle,
                "dup of the CEF plane descriptor failed",
            ));
        }
        let duplicated = unsafe { OwnedFd::from_raw_fd(duplicated) };
        let mut fd_properties = vk::MemoryFdPropertiesKHR::default();
        match unsafe {
            self.external_fd.get_memory_fd_properties(
                vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
                duplicated.as_raw_fd(),
                &mut fd_properties,
            )
        } {
            Ok(()) => (),
            Err(error) => {
                unsafe { self.device.destroy_image(image, None) };
                return Err(Failure::new(
                    FrameFailure::InvalidHandle,
                    format!("get_memory_fd_properties: {error:?}"),
                ));
            }
        };
        let requirements = unsafe { self.device.get_image_memory_requirements(image) };
        let bits = requirements.memory_type_bits & fd_properties.memory_type_bits;
        if buffer_size < requirements.size {
            unsafe { self.device.destroy_image(image, None) };
            return Err(Failure::new(
                FrameFailure::InvalidHandle,
                format!(
                    "the kernel DMA-BUF allocation spans {buffer_size} bytes but the import needs {} bytes (memory types {bits:#x})",
                    requirements.size
                ),
            ));
        }
        let Some(memory_type) = self.memory_type(bits, vk::MemoryPropertyFlags::empty()) else {
            unsafe { self.device.destroy_image(image, None) };
            return Err(Failure::new(
                FrameFailure::WrongDevice,
                "the CEF DMA-BUF has no memory type on the selected device",
            ));
        };
        let mut import = vk::ImportMemoryFdInfoKHR::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
            .fd(duplicated.into_raw_fd());
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
        let allocate = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type)
            .push_next(&mut import)
            .push_next(&mut dedicated);
        let memory = match unsafe { self.device.allocate_memory(&allocate, None) } {
            Ok(memory) => memory,
            Err(error) => {
                unsafe {
                    libc::close(import.fd);
                    self.device.destroy_image(image, None);
                }
                return Err(Failure::new(
                    FrameFailure::InvalidHandle,
                    format!("allocate_memory(import): {error:?}"),
                ));
            }
        };
        let result = unsafe { self.device.bind_image_memory(image, memory, 0) }
            .map_err(vulkan_failure("bind_image_memory(import)"))
            .and_then(|()| self.blit(image, source, target));
        unsafe {
            self.device.destroy_image(image, None);
            self.device.free_memory(memory, None);
        }
        result
    }

    fn initialize_exports(&self, images: &[ExportedImage]) -> Result<(), Failure> {
        if images.is_empty() {
            return Ok(());
        }
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        let prepare: Vec<_> = images
            .iter()
            .map(|image| {
                vk::ImageMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::empty())
                    .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .old_layout(vk::ImageLayout::UNDEFINED)
                    .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(image.image)
                    .subresource_range(range)
            })
            .collect();
        let release: Vec<_> = images
            .iter()
            .map(|image| {
                vk::ImageMemoryBarrier::default()
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags::empty())
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::GENERAL)
                    .src_queue_family_index(self.queue_family)
                    .dst_queue_family_index(self.foreign_family)
                    .image(image.image)
                    .subresource_range(range)
            })
            .collect();
        unsafe {
            self.device
                .reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty())
                .map_err(vulkan_failure("reset_export_initialization"))?;
            self.device
                .begin_command_buffer(
                    self.command_buffer,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(vulkan_failure("begin_export_initialization"))?;
            self.device.cmd_pipeline_barrier(
                self.command_buffer,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &prepare,
            );
            for image in images {
                self.device.cmd_clear_color_image(
                    self.command_buffer,
                    image.image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &vk::ClearColorValue {
                        float32: [0.0, 0.0, 0.0, 1.0],
                    },
                    &[range],
                );
            }
            self.device.cmd_pipeline_barrier(
                self.command_buffer,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &release,
            );
        }
        self.finish_commands().map(|_| ())
    }

    fn blit(
        &self,
        source_image: vk::Image,
        source: &ImportSource<'_>,
        target: &ExportedImage,
    ) -> Result<u64, Failure> {
        let device = &self.device;
        let command = self.command_buffer;
        unsafe {
            device
                .reset_command_buffer(command, vk::CommandBufferResetFlags::empty())
                .map_err(vulkan_failure("reset_command_buffer"))?;
            device
                .begin_command_buffer(
                    command,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(vulkan_failure("begin_command_buffer"))?;
        }
        let color = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        let acquire_source = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
            .old_layout(vk::ImageLayout::GENERAL)
            .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .src_queue_family_index(self.foreign_family)
            .dst_queue_family_index(self.queue_family)
            .image(source_image)
            .subresource_range(color);
        let release_source = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::TRANSFER_READ)
            .dst_access_mask(vk::AccessFlags::empty())
            .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .new_layout(vk::ImageLayout::GENERAL)
            .src_queue_family_index(self.queue_family)
            .dst_queue_family_index(self.foreign_family)
            .image(source_image)
            .subresource_range(color);
        let prepare_target = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .old_layout(vk::ImageLayout::GENERAL)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_queue_family_index(self.foreign_family)
            .dst_queue_family_index(self.queue_family)
            .image(target.image)
            .subresource_range(color);
        let layers = vk::ImageSubresourceLayers::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .layer_count(1);
        let region = vk::ImageCopy::default()
            .src_subresource(layers)
            .src_offset(vk::Offset3D {
                x: source.x as i32,
                y: source.y as i32,
                z: 0,
            })
            .dst_subresource(layers)
            .dst_offset(vk::Offset3D { x: 0, y: 0, z: 0 })
            .extent(vk::Extent3D {
                width: source.width,
                height: source.height,
                depth: 1,
            });
        let release_target = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::empty())
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::GENERAL)
            .src_queue_family_index(self.queue_family)
            .dst_queue_family_index(self.foreign_family)
            .image(target.image)
            .subresource_range(color);
        unsafe {
            device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[acquire_source, prepare_target],
            );
            device.cmd_copy_image(
                command,
                source_image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                target.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[region],
            );
            device.cmd_pipeline_barrier(
                command,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[release_source, release_target],
            );
        }
        self.finish_commands()
    }

    fn finish_commands(&self) -> Result<u64, Failure> {
        let device = &self.device;
        let command = self.command_buffer;
        unsafe {
            device
                .end_command_buffer(command)
                .map_err(vulkan_failure("end_command_buffer"))?;
            let commands = [command];
            let submit = vk::SubmitInfo::default().command_buffers(&commands);
            device
                .queue_submit(self.queue, &[submit], self.fence)
                .map_err(vulkan_failure("queue_submit"))?;
            let waited = device.wait_for_fences(&[self.fence], true, COPY_TIMEOUT_NS);
            if waited.is_err() {
                let _ = device.device_wait_idle();
            }
            device
                .reset_fences(&[self.fence])
                .map_err(vulkan_failure("reset_fences"))?;
            match waited {
                Ok(()) => Ok(super::now_ns()),
                Err(vk::Result::TIMEOUT) => Err(Failure::new(
                    FrameFailure::CopyFailed,
                    "the frame copy fence exceeded one second",
                )),
                Err(error) => Err(vulkan_failure("wait_for_fences")(error)),
            }
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_fence(self.fence, None);
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsRawFd;

    use super::*;

    #[test]
    fn a_frame_exported_by_this_device_round_trips_through_import_and_blit() {
        let Ok(engine) = Engine::new(None) else {
            eprintln!("no Vulkan device with DMA-BUF support; skipping");
            return;
        };
        let modifier = engine.choose_export_modifier(FrameFormat::Bgra8).unwrap();
        let images = engine
            .create_exported_images(64, 48, FrameFormat::Bgra8, modifier, 3)
            .unwrap();
        let mut images = images.into_iter();
        let mut source_image = images.next().unwrap();
        let fd = source_image.fd.take().unwrap();
        let planes = [source_image.layout];
        let source = ImportSource {
            fd: fd.as_raw_fd(),
            planes: &planes,
            modifier,
            format: FrameFormat::Bgra8,
            coded_width: 64,
            coded_height: 48,
            x: 0,
            y: 0,
            width: 64,
            height: 48,
        };
        for target in images {
            let outcome = engine.copy_frame(&source, &target);
            assert!(
                outcome.is_ok(),
                "{}: {:?}",
                engine.device_name,
                outcome.err().map(|f| f.detail)
            );
            let again = engine.copy_frame(&source, &target);
            assert!(again.is_ok(), "{:?}", again.err().map(|f| f.detail));
            engine.destroy_exported_image(target);
        }
        engine.destroy_exported_image(source_image);
    }
}
