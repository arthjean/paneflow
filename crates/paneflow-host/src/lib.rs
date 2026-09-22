#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unwrap_in_result,
        clippy::panic
    )
)]

pub mod agent;
pub mod bootstrap;
pub mod cancellation_scan;
pub mod client;
pub mod cold_text;
pub mod control;
pub mod endpoint;
pub mod env;
pub mod helpers;
pub mod hook_assets;
pub mod host;
pub mod maintenance;
pub mod manifest;
pub mod menu_prompt;
pub mod process;
pub mod protocol;
pub mod pty;
pub mod runtime;
pub mod runtime_observer;
pub mod screen_activity;
pub mod server;
pub mod session_input;
pub mod stream;
pub mod tail;
pub mod viewport_scan;

pub use paneflow_config::schema::{HostInstanceToken, SessionGeneration, SessionId, WorkspaceId};

pub use agent::{AgentEvent, AgentEventKind, AgentSnapshotEntry};
pub use bootstrap::{
    BootstrapError, HostAdoption, Probe, ensure_host_running, probe, stop_incompatible_host,
};
pub use client::{Attachment, HostClient, HostClientError, OutputEnd};
pub use host::{
    CreateSession, HostError, INACTIVE_ROWS_PER_WORKSPACE, MAX_PENDING_LAUNCHES, SessionHost,
    SessionReconnection, SessionRow, SessionSummary, SessionText, ShutdownReport,
    UnresolvedSession,
};
pub use manifest::{
    FinalOutput, HookRecord, HostedSessionRuntime, SessionLaunch, SessionLifecycle, SessionManifest,
};
pub use process::{ProcessIdentity, ProcessVerdict};
pub use protocol::{ClientHello, EngineIdentity, HOST_PROTOCOL_VERSION, HostIdentity};
pub use runtime::{Checkpoint, ViewportScan};
pub use runtime_observer::RuntimeObservation;
pub use server::{ServerHandle, serve};
