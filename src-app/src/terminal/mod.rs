//! Terminal state and view - PTY management and GPUI view wrapper.
//!
//! Manages the alacritty_terminal `Term`, its `tty` + `EventLoop` (EP-002), and
//! periodic sync. The TerminalView creates a TerminalElement for cell-by-cell
//! rendering.

#[cfg(test)]
pub(crate) mod backend_corpus;
pub mod blink;
pub mod element;
#[cfg(ghostty_native)]
mod ghostty_session;
#[cfg(all(test, ghostty_native))]
mod ghostty_stress;
mod input;
mod listener;
mod marks;
#[cfg(all(test, target_os = "linux"))]
mod portable_pty_probe;
mod pty_session;
mod search;
mod service_detector;
pub mod shell;
pub mod types;
pub mod view;

pub use listener::{SpikeTermSize, ZedListener};
pub(crate) use pty_session::TerminalSessionBackend;
pub use pty_session::TerminalState;
#[cfg(test)]
pub(crate) use pty_session::{
    start_render_content_timing_probe, take_render_content_lock_durations,
};
pub use service_detector::ServiceInfo;
pub use view::{TerminalEvent, TerminalView};

#[cfg(debug_assertions)]
pub(crate) use view::probe_enabled;
