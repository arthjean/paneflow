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
pub mod hook_assets;
pub mod hook_state;
pub mod integrations;
pub mod notifications;
pub mod protocol;
pub mod server;
pub mod state;
pub mod worker;

pub use bootstrap::{
    DRAIN_WAIT, OwnerLock, OwnerLockError, Probe, WorkerAdoption, WorkerBootstrapError,
    ensure_worker_running, needs_replacement, probe, stop_worker,
};
pub use hook_state::{ActivityEngine, HookState, Notice, Outcome};
pub use notifications::{ActivityLog, Notification};
pub use protocol::{
    REQUIRED_CORE_PROTOCOL, RESTART_RECOMMENDED, WORKER_PROTOCOL_VERSION, WorkerIdentity,
    advertised_capabilities, restart_recommendation,
};
pub use server::{ServerHandle, Worker};
pub use state::{ActivitySource, Health, Projection, SessionEntry, Status, WorkerState};
pub use worker::{RunningWorker, WorkerError, open, run};
