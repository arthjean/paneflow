use std::sync::Arc;
use std::time::{Duration, Instant};

use ash::vk;
use gpui::ExternalSurface;
use gpui_platform::gpui_wgpu::{ExternalSurfaceContext, ExternalWgpuSurface, wgpu};

type Vulkan = wgpu::hal::api::Vulkan;

#[derive(Clone, Copy)]
pub(crate) enum Transfer {
    Initialize,
    Acquire,
    Release,
}

fn texture(surface: &ExternalSurface) -> Result<&wgpu::Texture, String> {
    surface
        .handle()
        .downcast_ref::<ExternalWgpuSurface>()
        .map(ExternalWgpuSurface::texture)
        .ok_or_else(|| "external surface has no wgpu texture".into())
}

pub(crate) fn barrier(
    context: &ExternalSurfaceContext,
    surfaces: &[ExternalSurface],
    transfer: Transfer,
) -> Result<wgpu::CommandBuffer, String> {
    let device = unsafe { context.device.as_hal::<Vulkan>() }.ok_or("Vulkan device unavailable")?;
    if !device
        .enabled_device_extensions()
        .contains(&ash::ext::queue_family_foreign::NAME)
    {
        return Err("Vulkan external ownership requires VK_EXT_queue_family_foreign".into());
    }
    let queue_family = device.queue_family_index();
    let range = vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .level_count(1)
        .layer_count(1);
    let mut barriers = Vec::with_capacity(surfaces.len());
    for surface in surfaces {
        let image = unsafe { texture(surface)?.as_hal::<Vulkan>() }
            .ok_or("external surface has no Vulkan image")?;
        let (old, new, source_family, target_family, source_access, target_access) = match transfer
        {
            Transfer::Initialize => (
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::GENERAL,
                vk::QUEUE_FAMILY_FOREIGN_EXT,
                queue_family,
                vk::AccessFlags::empty(),
                vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE,
            ),
            Transfer::Acquire => (
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::QUEUE_FAMILY_FOREIGN_EXT,
                queue_family,
                vk::AccessFlags::empty(),
                vk::AccessFlags::SHADER_READ,
            ),
            Transfer::Release => (
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::ImageLayout::GENERAL,
                queue_family,
                vk::QUEUE_FAMILY_FOREIGN_EXT,
                vk::AccessFlags::SHADER_READ,
                vk::AccessFlags::empty(),
            ),
        };
        barriers.push(
            vk::ImageMemoryBarrier::default()
                .image(unsafe { image.raw_handle() })
                .subresource_range(range)
                .old_layout(old)
                .new_layout(new)
                .src_queue_family_index(source_family)
                .dst_queue_family_index(target_family)
                .src_access_mask(source_access)
                .dst_access_mask(target_access),
        );
    }
    let mut encoder = context
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("browser-external-ownership"),
        });
    unsafe {
        encoder.as_hal_mut::<Vulkan, _, _>(|encoder| {
            let encoder = encoder.ok_or("Vulkan command encoder unavailable")?;
            device.raw_device().cmd_pipeline_barrier(
                encoder.raw_handle(),
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &barriers,
            );
            Ok::<(), String>(())
        })?;
    }
    drop(device);
    Ok(encoder.finish())
}

pub(crate) fn initialize(
    context: Arc<ExternalSurfaceContext>,
    surfaces: Vec<ExternalSurface>,
) -> Result<impl Future<Output = Result<(), String>> + Send, String> {
    let validation_scope = context
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    let memory_scope = context
        .device
        .push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let internal_scope = context.device.push_error_scope(wgpu::ErrorFilter::Internal);
    let acquired = barrier(&context, &surfaces, Transfer::Initialize)?;
    let released = barrier(&context, &surfaces, Transfer::Release)?;
    let textures: Vec<_> = surfaces.iter().map(texture).collect::<Result<_, _>>()?;
    let mut encoder = context
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("browser-pool-tracker-initialization"),
        });
    encoder.transition_resources(
        std::iter::empty(),
        textures.iter().map(|texture| wgpu::TextureTransition {
            texture: *texture,
            selector: None,
            state: wgpu::TextureUses::RESOURCE,
        }),
    );
    context.queue.submit([acquired, encoder.finish(), released]);
    let internal = internal_scope.pop();
    let out_of_memory = memory_scope.pop();
    let validation = validation_scope.pop();
    let (done, completed) = std::sync::mpsc::channel();
    context.queue.on_submitted_work_done(move || {
        drop(surfaces);
        let _ = done.send(());
    });
    Ok(async move {
        smol::unblock(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                context
                    .device
                    .poll(wgpu::PollType::Poll)
                    .map_err(|error| error.to_string())?;
                match completed.recv_timeout(Duration::from_millis(1)) {
                    Ok(()) => return Ok::<(), String>(()),
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        return Err("pool initialization callback disconnected".into());
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                        if Instant::now() >= deadline =>
                    {
                        return Err("pool initialization GPU deadline exceeded".into());
                    }
                    Err(_) => (),
                }
            }
        })
        .await?;
        if let Some(error) = [internal.await, out_of_memory.await, validation.await]
            .into_iter()
            .flatten()
            .next()
        {
            return Err(format!("pool initialization rejected: {error}"));
        }
        Ok(())
    })
}
