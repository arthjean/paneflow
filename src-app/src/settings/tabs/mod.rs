//! Settings page bodies.
//!
//! Each submodule defines the body of one settings section plus any action
//! handlers that belong only to that section, all `impl crate::PaneFlowApp`.
//! The nav rail + content shell live in `settings::chrome`.

pub mod ai_agent;
pub mod appearance;
pub mod general;
pub mod mcp;
pub mod shortcuts;
pub mod terminal;
pub mod workspaces;
