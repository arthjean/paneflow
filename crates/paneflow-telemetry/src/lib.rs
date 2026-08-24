// Test-only allow for the CLAUDE.md-mandated clippy restrictions. Mirrors
// the `paneflow-app` belt: `clippy.toml`'s `allow-{unwrap,expect}-in-tests`
// keys cover the unwrap/expect family but not `clippy::panic`, which the
// `client.rs` test module uses to assert variant invariants
// (`panic!("expected Active variant")`).
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unwrap_in_result,
        clippy::panic
    )
)]

//! PaneFlow telemetry plumbing: consent-gated PostHog capture, a closed
//! event schema, anonymous per-installation ids, and canonical-tag invariants.
//!
//! Extracted from `paneflow-app` per US-003 of the cmux-port PRD so that
//! future workspace members can emit events without taking a dependency
//! on the desktop binary.
//!
//! Submodules:
//! - [`client`] - consent-gated `TelemetryClient`, queue, and batched flush.
//! - [`event`] - the closed, auditable event and property schema.
//! - [`id`] - anonymous, per-installation UUID v4 with first-run flag.
//! - [`tags`] - canonical-tag format invariant helper used by
//!   consumers that map their domain enums to PostHog properties.
//!
//! Subsystem invariants:
//! - No event is ever emitted unless the caller has resolved opt-in
//!   consent **and** the kill-switch env vars are absent
//!   (`PANEFLOW_NO_TELEMETRY`, `DO_NOT_TRACK`, `NO_TELEMETRY`).
//! - Event names and property shapes come only from [`event::TelemetryEvent`].
//!   Reserved PostHog processing controls are owned by the client and cannot
//!   be supplied by consumers.

pub mod client;
pub mod event;
pub mod id;
pub mod tags;
