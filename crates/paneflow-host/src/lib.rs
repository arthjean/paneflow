#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unwrap_in_result,
        clippy::panic
    )
)]

pub mod bootstrap;
pub mod client;
pub mod endpoint;
pub mod env;
pub mod host;
pub mod manifest;
pub mod process;
pub mod protocol;
pub mod runtime;
pub mod server;
pub mod tail;
mod wire;

pub use paneflow_config::schema::{HostInstanceToken, SessionGeneration, SessionId, WorkspaceId};

pub use bootstrap::{BootstrapError, HostAdoption, Probe, ensure_host_running, probe};
pub use client::{Attachment, HostClient, HostClientError, OutputEnd};
pub use host::{CreateSession, HostError, SessionHost, SessionReconnection, SessionSummary};
pub use manifest::{AgentSummary, SessionLaunch, SessionLifecycle, SessionManifest};
pub use protocol::{ClientHello, EngineIdentity, HOST_PROTOCOL_VERSION, HostIdentity};
pub use runtime::Checkpoint;
pub use server::{ServerHandle, serve};
