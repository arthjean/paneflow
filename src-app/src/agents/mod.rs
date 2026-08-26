//! Support modules for the CLI agents Paneflow launches in a PTY (see
//! [`crate::agent_launcher`]).
//!
//! What remains:
//! - [`notifications`] - desktop-notification routing and visibility gates.
//! - [`parent_guard`] - process-parent death guards for PTYs and agent CLIs.

pub mod notifications;
pub mod parent_guard;
