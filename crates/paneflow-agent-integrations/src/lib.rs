#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod cli;
pub mod hooks;
pub mod integrations;
pub mod io;
pub mod legacy_mcp;
pub mod merge;

pub use cli::run_cli;
pub use hooks::run_hooks_cli;
pub use integrations::{
    adopt_and_refresh_installed, install_integration, install_target_integration,
    list_integrations, list_target_integrations, mirror_installed_integrations,
    purge_default_home_legacy_mcp, purge_target_legacy_mcp, refresh_target_integrations,
    remove_integration, remove_legacy_project_hooks, run_integrations_cli, state_label,
    InstallMode, IntegrationBinaries, IntegrationState, IntegrationStatus, IntegrationTarget,
};
