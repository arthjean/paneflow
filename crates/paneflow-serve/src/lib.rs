#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unwrap_in_result,
        clippy::panic
    )
)]

pub mod activity;
pub mod bootstrap;
pub mod controller;
pub mod core_link;
pub mod hook_assets;
pub mod hook_state;
pub mod integrations;
pub mod notifications;
pub mod protocol;
pub mod server;
pub mod state;
pub mod worker;

pub use bootstrap::{DRAIN_WAIT, ensure_worker_running, stop_worker};
pub use controller::{Bootstrap, Controller};
pub use protocol::{RESTART_RECOMMENDED, WORKER_PROTOCOL_VERSION, advertised_capabilities};
pub use state::ActivitySource;
pub use worker::{WorkerError, open, run};
