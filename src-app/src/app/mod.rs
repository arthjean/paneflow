//! App-layer modules extracted from `main.rs`.
//!
//! See `tasks/prd-src-app-refactor.md` for the ongoing decomposition plan.

pub mod about_dialog;
pub mod actions;
pub mod agents_bottom_panel;
pub mod agents_diff;
pub mod agents_sidebar;
pub mod agents_view_actions;
pub mod attention_queue;
pub mod bootstrap;
pub mod broadcast;
pub mod composer;
pub mod constants;
pub mod custom_buttons_modal;
pub mod diff_sidebar;
pub mod diff_view_actions;
pub mod diff_view_helpers;
pub mod drag;
pub mod event_handlers;
pub mod files_sidebar;
pub mod files_tree;
pub mod fleet_search;
pub mod ipc_handler;
pub mod launch_pad;
pub mod notifications;
pub mod profile_menu;
pub mod project_ops;
pub mod self_update_flow;
pub mod session;
pub mod sessions_sidebar;
pub mod settings;
pub mod sidebar;
pub mod sidebar_actions_menu;
pub mod telemetry_events;
pub mod theme_picker;
pub mod workspace_ops;
