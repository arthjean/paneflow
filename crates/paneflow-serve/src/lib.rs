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
pub mod core_link;
pub mod integrations;
pub mod protocol;
pub mod server;
pub mod state;
pub mod worker;

pub use bootstrap::{
    DRAIN_WAIT, OwnerLock, OwnerLockError, Probe, WorkerAdoption, WorkerBootstrapError,
    ensure_worker_running, needs_replacement, probe, stop_worker,
};
pub use protocol::{
    REQUIRED_CORE_PROTOCOL, RESTART_RECOMMENDED, WORKER_PROTOCOL_VERSION, WorkerIdentity,
    advertised_capabilities, restart_recommendation,
};
pub use server::{ServerHandle, Worker};
pub use state::{ActivitySource, Health, SessionEntry, WorkerState};
pub use worker::{RunningWorker, WorkerError, open, run};
