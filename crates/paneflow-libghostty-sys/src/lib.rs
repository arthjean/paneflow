#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

#[cfg(target_os = "linux")]
#[path = "../../../native/libghostty/bindings.rs"]
mod bindings;

#[cfg(target_os = "linux")]
pub use bindings::*;

pub const EXPECTED_API_VERSION: &str = env!("PANEFLOW_GHOSTTY_API_VERSION");
pub const GHOSTTY_APP_VERSION: &str = env!("PANEFLOW_GHOSTTY_APP_VERSION");
pub const GHOSTTY_XTVERSION: &str = concat!("ghostty ", env!("PANEFLOW_GHOSTTY_APP_VERSION"));
