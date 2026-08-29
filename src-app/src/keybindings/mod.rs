//! Centralized keybinding management: defaults, user overrides, hot-reload.
//!
//! Uses Zed's clear → defaults → user-overrides pattern via GPUI's
//! `clear_key_bindings()` + `bind_keys()`.
//!
//! Module layout (US-022):
//! - [`registry`] - `ActionMeta` + `ACTIONS` table (one source of truth),
//!   plus [`ShortcutGroup`], the section each action lands in on the
//!   settings page
//! - [`defaults`] - `DEFAULTS` and `MACOS_ONLY_DEFAULTS` binding tables
//! - [`apply`] - registers bindings on the GPUI `App`
//! - [`display`] - formats keystrokes and builds the settings shortcut list

mod apply;
mod defaults;
mod display;
mod registry;

pub use apply::{apply_keybindings, keystrokes_conflict};
pub use display::{ShortcutEntry, effective_shortcuts, format_keystroke, is_bare_modifier};
pub use registry::ShortcutGroup;
