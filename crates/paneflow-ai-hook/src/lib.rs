#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unwrap_in_result,
        clippy::panic
    )
)]

mod event;
mod runtime;
mod transport;

pub const MAX_STDIN_BYTES: usize = 16 * 1024 * 1024;

pub fn run() {
    runtime::dispatch();
}
