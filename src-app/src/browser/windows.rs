use std::sync::Arc;

use gpui::{DevicePixels, ExternalSurface, size};
use gpui_platform::gpui_windows::{ExternalSurfaceContext, ExternalSurfaceFormat};
use paneflow_browser_protocol::{FrameFormat, POOL_BUFFERS};
use windows_sys::Win32::Foundation::CloseHandle;

use super::presentation::{NativeFrameHandles, PoolLayout, TextureImporter};

pub struct D3dImporter {
    context: Arc<ExternalSurfaceContext>,
}

impl D3dImporter {
    pub fn new(context: Arc<ExternalSurfaceContext>) -> Self {
        Self { context }
    }

    pub fn close_handles(handles: NativeFrameHandles) {
        for handle in handles {
            if handle != 0 {
                unsafe {
                    let _ = CloseHandle(handle as *mut std::ffi::c_void);
                }
            }
        }
    }
}

impl TextureImporter for D3dImporter {
    type Texture = ExternalSurface;

    fn import(
        &mut self,
        pool: &PoolLayout,
        handles: NativeFrameHandles,
    ) -> Result<Vec<Self::Texture>, String> {
        if pool.modifier != 0 {
            Self::close_handles(handles);
            return Err("Windows external surfaces do not support modifiers".to_string());
        }
        if handles.len() != usize::from(POOL_BUFFERS) {
            Self::close_handles(handles);
            return Err("Windows frame pool did not contain three shared handles".to_string());
        }
        let format = match pool.format {
            FrameFormat::Bgra8 => ExternalSurfaceFormat::Bgra8,
            FrameFormat::Rgba8 => ExternalSurfaceFormat::Rgba8,
        };
        let mut remaining = handles.into_iter();
        let mut textures = Vec::with_capacity(usize::from(POOL_BUFFERS));
        while let Some(handle) = remaining.next() {
            let surface =
                match self
                    .context
                    .import_shared_texture(handle, pool.width, pool.height, format)
                {
                    Ok(surface) => surface,
                    Err(error) => {
                        Self::close_handles(remaining.collect());
                        return Err(format!("importing Windows shared texture: {error:#}"));
                    }
                };
            textures.push(ExternalSurface::new(
                pool.generation,
                size(
                    DevicePixels(pool.width as i32),
                    DevicePixels(pool.height as i32),
                ),
                Arc::new(surface),
            ));
        }
        Ok(textures)
    }
}
