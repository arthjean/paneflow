#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unwrap_in_result,
        clippy::panic
    )
)]

pub mod action_runs;
pub mod activity;
pub mod agent_asks;
pub mod agent_caller;
pub mod agent_prompts;
mod authority;
pub mod bootstrap;
pub mod config_runtime;
pub mod controller;
pub mod core_link;
pub mod event_log;
pub mod handles;
pub mod integrations;
pub mod launcher;
pub mod layout_authority;
pub mod notifications;
pub mod prompts;
pub mod protocol;
pub mod registry;
pub mod script_backend;
pub mod server;
pub mod state;
pub mod structural;
pub mod sweep;
pub mod typing;
pub mod usage;
pub mod worker;
mod worktrees;

pub use bootstrap::{DRAIN_WAIT, ensure_worker_running, stop_worker};
pub use controller::{Bootstrap, Controller};
pub use protocol::{RESTART_RECOMMENDED, WORKER_PROTOCOL_VERSION, advertised_capabilities};
pub use state::ActivitySource;
pub use worker::{Launching, WorkerError, open, open_with, run, run_with};
