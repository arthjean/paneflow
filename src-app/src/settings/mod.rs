//! Embedded settings - Codex-style settings rendered *inside* the main window
//! (grouped nav rail + content panel) rather than a separate GPUI window.
//!
//! Layout:
//! - `chrome`     - the nav rail (`render_settings_nav`) + content panel
//!   (`render_settings_content_panel`) + section dispatch, all on `PaneFlowApp`.
//! - `components` - shared UI primitives (cards, toggles, section headers).
//! - `nav_header` - fixed Settings title + close action for the nav rail.
//! - `tabs`       - per-section bodies (`general`, `appearance`, `shortcuts`,
//!   `terminal`, `ai_agent`, `mcp`), each `impl PaneFlowApp`.
//!
//! The Settings button (`PaneFlowApp::open_settings_window`, in `app::settings`)
//! sets `settings_section = Some(General)`; `main.rs` then swaps the left rail
//! for the nav and the content area for the panel. There is no standalone
//! settings window anymore.

pub mod chrome;
pub mod components;
mod nav_header;
pub mod tabs;
