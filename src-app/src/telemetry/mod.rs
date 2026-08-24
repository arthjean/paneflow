//! Desktop telemetry subsystem (opt-in, anonymous).
//!
//! US-003: the reusable plumbing now lives in the `paneflow-telemetry`
//! workspace crate. This module re-exports the client and closed event schema,
//! and provides thin
//! domain shims for `id` (path resolution wrapper) and `tags` (the
//! `InstallMethod`/`UpdateError` mapping that depends on `crate::update`
//! types and therefore stays inside the desktop binary).
//!
//! Submodules:
//! - `client` (US-012) - consent-gated capture and flushing.
//! - `event` - the closed, runtime-validated event schema.
//! - `id` (US-010) - desktop shim that resolves `runtime_paths::data_dir()`
//!   then delegates to `paneflow_telemetry::id::telemetry_id_at`.
//! - `tags` (US-013) - domain mapping for `InstallMethod` and `UpdateError`
//!   (kept here because those types live in `crate::update::*`); the
//!   canonical-tag format invariant moved to `paneflow_telemetry::tags`.
//!
//! The `PaneFlowApp`-bound `emit_*` wrappers live in `crate::app::telemetry_events`;
//! this module owns the plumbing (client re-export, id resolution, tag
//! flattening) while the capture-site helpers stay close to the rest of
//! the app surface.
//!
//! Invariants shared across the subsystem:
//! - No event is ever emitted unless `config.telemetry.enabled == Some(true)`
//!   **and** no kill-switch env var is set (`PANEFLOW_NO_TELEMETRY`,
//!   `DO_NOT_TRACK`, `NO_TELEMETRY`).
//! - No PII, paths, or terminal content is accepted by the closed event schema.

pub use paneflow_telemetry::{client, event};

pub mod id;
pub mod tags;
