#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod claude_hooks;
mod hook_command;
pub mod io;
pub mod jsonc;
pub mod lease;
pub mod lock;
pub mod runtime_catalog;

#[cfg(test)]
#[path = "../build_support.rs"]
mod build_support;

pub use io::{
    claude_config_dir, config_dir, home_dir, read_optional_text, write_json_atomic,
    write_text_atomic,
};
pub use lease::{ConfigLease, LastConfigLease};
pub use lock::{lock_config, with_config_lock, ConfigLock};
pub use runtime_catalog::*;
