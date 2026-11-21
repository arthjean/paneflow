#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod command;
pub mod counters;
pub mod directory_env;
#[cfg(any(test, feature = "test-support"))]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test fixtures fail the test that calls them"
)]
pub mod fixture;
pub mod repo;
pub mod worktree;
pub mod worktree_plan;
