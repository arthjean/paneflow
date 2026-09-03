#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod agents;
pub mod api;
pub mod cli;
pub mod detect;
pub mod hooks;
pub mod io;
pub mod merge;

pub use api::{
    install_all, overall_state, status_all, uninstall_all, AgentResult, InstallKind, InstallReport,
    OverallState, StatusKind, StatusReport, UninstallKind, UninstallReport,
};
pub use cli::run_cli;
pub use hooks::run_hooks_cli;
