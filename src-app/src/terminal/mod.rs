//! Terminal state and view - PTY management and GPUI view wrapper.
//!
//! Manages the alacritty_terminal `Term`, its `tty` + `EventLoop` (EP-002), and
//! periodic sync. The TerminalView creates a TerminalElement for cell-by-cell
//! rendering.

pub mod blink;
pub mod element;
mod input;
mod listener;
mod pty_session;
mod search;
mod service_detector;
pub mod shell;
pub mod types;
pub mod view;

pub use listener::{SpikeTermSize, ZedListener};
pub use pty_session::{PtyNotifier, TerminalState};
pub use service_detector::ServiceInfo;
pub use view::{TerminalEvent, TerminalView};

#[cfg(debug_assertions)]
pub(crate) use view::probe_enabled;
