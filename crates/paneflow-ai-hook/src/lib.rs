#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unwrap_in_result,
        clippy::panic
    )
)]
//! PaneFlow AI-hook callback runtime.

mod event;
mod runtime;
mod transport;

/// Maximum hook payload accepted from stdin.
pub const MAX_STDIN_BYTES: usize = 16 * 1024 * 1024;

/// Run one hook invocation. Failures are diagnosed only through the optional
/// hook log and never escape to the invoking AI CLI.
pub fn run() {
    runtime::dispatch();
}
