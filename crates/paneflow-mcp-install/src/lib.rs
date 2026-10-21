#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod agents;
pub mod api;
pub mod cli;
pub mod detect;
pub mod hooks;
pub mod integrations;
pub mod io;
pub mod merge;

pub use api::{
    install_all, overall_state, status_all, AgentResult, InstallKind, InstallReport, OverallState,
    StatusKind, StatusReport, UninstallKind,
};
pub use cli::run_cli;
pub use hooks::run_hooks_cli;
pub use integrations::{
    adopt_and_refresh_installed, install_integration, install_target_integration,
    list_integrations, list_target_integrations, mirror_installed_integrations,
    refresh_target_integrations, remove_integration, remove_legacy_project_hooks,
    run_integrations_cli, state_label, InstallMode, IntegrationBinaries, IntegrationState,
    IntegrationStatus, IntegrationTarget,
};
