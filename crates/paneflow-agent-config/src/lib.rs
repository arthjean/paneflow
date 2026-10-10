#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod claude_hooks;
pub mod delivery;
mod hook_command;
#[cfg(feature = "config-io")]
pub mod io;
pub mod jsonc;
#[cfg(feature = "config-io")]
pub mod lock;
pub mod runtime_catalog;

#[cfg(test)]
#[path = "../build_support.rs"]
mod build_support;
#[cfg(test)]
mod packaging_tests;

#[cfg(feature = "config-io")]
pub use io::{
    absolute_env_dir, claude_config_dir, codex_home, codex_home_from, home_dir, ClaudePaths,
};
#[cfg(feature = "config-io")]
pub use lock::{lock_config, lock_config_in, ConfigLock};
pub use runtime_catalog::*;

pub const CLAUDE_PREASSIGN_SESSION_ID_ENV: &str = "PANEFLOW_CLAUDE_PREASSIGN_SESSION_ID";
