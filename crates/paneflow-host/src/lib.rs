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
pub mod persistence;
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

pub use paneflow_config::schema::{HostInstanceToken, SessionGeneration, SessionId};

pub use bootstrap::{BootstrapError, Probe, ensure_host_running, probe};
pub use client::{Attachment, HostClient, HostClientError};
pub use host::{
    CreateSession, HostError, SessionHost, SessionReconnection, SessionRow, SessionSummary,
};
pub use manifest::{SessionLifecycle, SessionManifest};
pub use process::{ProcessIdentity, ProcessVerdict};
pub use protocol::{ClientHello, HOST_PROTOCOL_VERSION};
pub use runtime::Checkpoint;
pub use server::{ServerHandle, serve};
