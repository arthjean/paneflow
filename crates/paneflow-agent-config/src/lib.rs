//! Shared, dependency-light coding-agent configuration primitives.
//!
//! Both the long-lived installer and the size-constrained shim depend on this
//! crate, so cross-process locking and Claude hook shapes have one canonical
//! implementation.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod claude_hooks;
mod hook_command;
pub mod io;
pub mod jsonc;
pub mod lease;
pub mod lock;

pub use io::{config_dir, home_dir, read_optional_text, write_json_atomic, write_text_atomic};
pub use lease::{ConfigLease, LastConfigLease};
pub use lock::{lock_config, with_config_lock, ConfigLock};
