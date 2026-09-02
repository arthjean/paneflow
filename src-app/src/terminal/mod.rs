#[cfg(test)]
pub(crate) mod bench_corpus;
pub mod blink;
mod clipboard_gate;
pub mod element;
mod ghostty_session;
#[cfg(test)]
mod ghostty_stress;
mod input;
pub mod kitty;
mod marks;
#[cfg(test)]
mod perf_bench;
#[cfg(all(test, target_os = "linux"))]
mod portable_pty_probe;
mod pty_session;
mod search;
mod service_detector;
pub mod shell;
pub mod types;
pub mod view;

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
