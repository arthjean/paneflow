pub mod contract;
#[cfg(target_os = "macos")]
mod plugin;

#[cfg(target_os = "macos")]
pub use plugin::{CLASS_NAME, image_path, publish, publish_to, register};
