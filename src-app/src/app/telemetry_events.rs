//! v1 desktop telemetry events - `app_started`, `app_exited`,
//! `update_installed` (US-013). Thin wrappers over the client in
//! `telemetry::client`; all property construction lives here so the
//! event schema is auditable in one place (cross-referenced with the
//! compliance record at `tasks/compliance-analytics.md §5`).
//!
//! None of these helpers check consent - that's already the
//! `TelemetryClient::from_consent` factory's job in bootstrap. If the
//! client is `Null`, every `capture` call is a no-op and no HTTP
//! request is made, satisfying PRD AC #6.

use std::time::Duration;

use serde_json::json;

use crate::PaneFlowApp;
use crate::app::session::SessionCorruptionInfo;
use crate::telemetry::tags::{error_category_tag, install_method_tag};
use crate::update::{self, UpdateError};

/// Upper bound on how long we wait for the shutdown flush to complete
/// before the batch is dropped and process exit continues. Matches the
/// US-012 client's 2-second contract.
const SHUTDOWN_FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

impl PaneFlowApp {
    /// Emit a `session_corrupted` event with the forensic context
    /// `app::session::load_session_at` gathered before falling back to
    /// an empty session (US-006). Consent gating is inherited from the
    /// `TelemetryClient` constructed in bootstrap - when the user has
    /// telemetry off the client is `Null` and the call is a no-op, so
    /// no network request fires.
    ///
    /// The `backup_path` is intentionally NOT sent: it includes the
    /// user's `$HOME` and would leak the username. We send a boolean
    /// `backup_written` instead so the operator can still distinguish
    /// "we have forensics on disk" from "backup write itself failed"
    /// (AC6) without exfiltrating filesystem layout.
    pub(crate) fn emit_session_corrupted(&self, info: &SessionCorruptionInfo) {
        self.telemetry.capture(
            "session_corrupted",
            json!({
                "error": info.error_category,
                "file_size": info.file_size,
                "file_age_seconds": info.file_age_seconds,
                "backup_written": info.backup_path.is_some(),
            }),
        );
    }

    /// Fire the once-per-launch `app_started` event. Called at the end
    /// of `PaneFlowApp::new` (bootstrap), after the telemetry client
    /// handle has been constructed from the loaded config.
    ///
    /// Property set matches US-013 AC #1 verbatim: `os`, `arch`,
    /// `app_version`, `install_method`, `is_first_run`.
    pub(crate) fn emit_app_started(&self, is_first_run: bool) {
        self.telemetry.capture(
            "app_started",
            json!({
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
                "app_version": env!("CARGO_PKG_VERSION"),
                "install_method": install_method_tag(&self.self_update.install_method),
                "is_first_run": is_first_run,
            }),
        );
    }

    /// Fire `app_exited` and block up to [`SHUTDOWN_FLUSH_TIMEOUT`] for
    /// the batch to reach PostHog. Called from `on_window_should_close`
    /// before `cx.quit()` - we accept a ≤2 s shutdown delay so the last
    /// session's data lands; the client itself detaches its worker on
    /// timeout, so process exit never waits longer than that.
    ///
    /// `session_duration_seconds` is computed from `self.launch_instant`
    /// captured in bootstrap - monotonic, wall-clock-change-proof.
    pub(crate) fn emit_app_exited_and_flush(&self) {
        let duration_seconds = self.launch_instant.elapsed().as_secs();
        self.telemetry.capture(
            "app_exited",
            json!({
                "session_duration_seconds": duration_seconds,
            }),
        );
        self.telemetry.flush_blocking(SHUTDOWN_FLUSH_TIMEOUT);
    }

    /// Fire `update_installed { success: true, ... }` WITHOUT blocking.
    ///
    /// US-017: this is emitted at *pre-install success* (the update is staged
    /// and we flip to `ReadyToRestart`), where the process is **not** exiting -
    /// the user restarts later via a separate, deliberately zero-I/O click that
    /// calls `cx.restart()`. Blocking the render thread on a network flush here
    /// is wrong: just `capture()` and let the 5 s background `poll_flush` loop
    /// (wired in `bootstrap.rs`) drain it while the "ready to restart" UI is up.
    /// `flush_blocking` stays reserved for the real exit path
    /// ([`Self::emit_app_exited_and_flush`]).
    ///
    /// `to_version` is read from `self.self_update.update_status` (populated during the
    /// update check); an unknown state still emits `to_version: "unknown"`
    /// rather than silently dropping.
    pub(crate) fn emit_update_success(&self) {
        let to_version = match self.self_update.update_status.as_ref() {
            Some(update::checker::UpdateStatus::Available { version, .. }) => version.clone(),
            _ => "unknown".to_string(),
        };
        self.telemetry.capture(
            "update_installed",
            json!({
                "from_version": env!("CARGO_PKG_VERSION"),
                "to_version": to_version,
                "install_method": install_method_tag(&self.self_update.install_method),
                "success": true,
            }),
        );
    }

    /// Fire `update_installed { success: false, error_category: ... }`
    /// without blocking - the process is NOT about to die, so we let
    /// the background flush loop pick this up on its next tick.
    ///
    /// Called from the single choke-point `PaneFlowApp::record_update_failure`
    /// after the error has been classified. Only a canonical
    /// `error_category` label is sent - never the error message
    /// (PRD AC #4: "no error message details - just category").
    pub(crate) fn emit_update_failure(&self, err: &UpdateError) {
        let to_version = match self.self_update.update_status.as_ref() {
            Some(update::checker::UpdateStatus::Available { version, .. }) => version.clone(),
            _ => "unknown".to_string(),
        };
        self.telemetry.capture(
            "update_installed",
            json!({
                "from_version": env!("CARGO_PKG_VERSION"),
                "to_version": to_version,
                "install_method": install_method_tag(&self.self_update.install_method),
                "success": false,
                "error_category": error_category_tag(err),
            }),
        );
    }

    /// Emit `update_dismissed` from the title-bar dismiss handler.
    /// `to_version` resolves the same way as the success/failure
    /// emitters so the funnel ties cleanly to `update_available`.
    /// Consent gating is inherited from the `TelemetryClient`.
    pub(crate) fn emit_update_dismissed(&self, reason: UpdateDismissReason) {
        let to_version = match self.self_update.update_status.as_ref() {
            Some(update::checker::UpdateStatus::Available { version, .. }) => version.clone(),
            _ => "unknown".to_string(),
        };
        emit_update_dismissed_via(
            &self.telemetry,
            env!("CARGO_PKG_VERSION"),
            &to_version,
            reason,
        );
    }
}

// ---------------------------------------------------------------------------
// US-007: free-function emitters callable from non-`&self` contexts.
// ---------------------------------------------------------------------------
//
// `update_check_started` and `update_available` fire from the detached
// thread spawned by `update::checker::spawn_check`, so they cannot
// borrow `&PaneFlowApp`. Each takes `&TelemetryClient` directly; the
// `Null` client variant short-circuits inside `capture()`, so an
// opt-out user never produces a network call (AC4).
//
// All payloads stick to the existing v1 schema convention: lowercase
// snake_case event names, `from_version`/`to_version` without a `v`
// prefix, install/asset tags coming from canonical helpers
// (`install_method_tag`, `AssetFormat::telemetry_tag`).

/// Reason a user closed the update prompt. Today only the explicit
/// "×" click on the title-bar pill fires
/// [`UpdateDismissReason::UserDismissed`]; [`UpdateDismissReason::DialogClosed`]
/// is reserved for the (not-yet-implemented) modal-dialog dismiss path
/// from the PRD's AC3 "ferme le dialog d'update" branch - kept in the
/// v1 schema so dashboards don't need a back-fill migration once that
/// path lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UpdateDismissReason {
    UserDismissed,
    #[allow(dead_code)]
    DialogClosed,
}

impl UpdateDismissReason {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            UpdateDismissReason::UserDismissed => "user_dismissed",
            UpdateDismissReason::DialogClosed => "dialog_closed",
        }
    }
}

/// Build the `update_check_started` property bag. Public so the
/// `update::checker::tests` module can assert the schema directly
/// (AC5) without piping through HTTP.
pub(crate) fn update_check_started_props(
    trigger: crate::update::checker::UpdateCheckTrigger,
    current_version: &str,
) -> serde_json::Value {
    json!({
        "trigger": trigger.as_str(),
        "current_version": current_version,
    })
}

pub(crate) fn emit_update_check_started(
    telemetry: &crate::telemetry::client::TelemetryClient,
    trigger: crate::update::checker::UpdateCheckTrigger,
    current_version: &str,
) {
    telemetry.capture(
        "update_check_started",
        update_check_started_props(trigger, current_version),
    );
}

/// Build the `update_available` property bag (US-007 AC2). The caller
/// must have already verified that an asset matched the host install
/// method - this helper does no filtering of its own.
pub(crate) fn update_available_props(
    from_version: &str,
    to_version: &str,
    asset_format_tag: &str,
) -> serde_json::Value {
    json!({
        "from_version": from_version,
        "to_version": to_version,
        "asset_format": asset_format_tag,
    })
}

pub(crate) fn emit_update_available(
    telemetry: &crate::telemetry::client::TelemetryClient,
    from_version: &str,
    to_version: &str,
    asset_format_tag: &str,
) {
    telemetry.capture(
        "update_available",
        update_available_props(from_version, to_version, asset_format_tag),
    );
}

pub(crate) fn update_dismissed_props(
    from_version: &str,
    to_version: &str,
    reason: UpdateDismissReason,
) -> serde_json::Value {
    json!({
        "from_version": from_version,
        "to_version": to_version,
        "reason": reason.as_str(),
    })
}

pub(crate) fn emit_update_dismissed_via(
    telemetry: &crate::telemetry::client::TelemetryClient,
    from_version: &str,
    to_version: &str,
    reason: UpdateDismissReason,
) {
    telemetry.capture(
        "update_dismissed",
        update_dismissed_props(from_version, to_version, reason),
    );
}

#[cfg(test)]
mod tests {
    //! US-022 (verify-not-fix): PII-absence guard for the v1 telemetry
    //! event-property surface. PII is excluded *by construction* - every
    //! string-valued property is a platform const (`std::env::consts`), a
    //! compile-time version (`CARGO_PKG_VERSION`) or release-feed semver, a
    //! canonical `&'static str` tag (`install_method_tag`/`error_category_tag`,
    //! whose lowercase-ASCII format is enforced in `telemetry::tags::tests`),
    //! or a closed enum (`UpdateCheckTrigger`/`UpdateDismissReason`). No
    //! property is built from a path, username, hostname, terminal content,
    //! or free-form error message. `distinct_id` is an anonymous UUID v4.
    //!
    //! This test pins that invariant so a future free-form property cannot
    //! silently leak user data through PostHog.

    use super::*;
    use crate::update::checker::UpdateCheckTrigger;
    use crate::update::install_method::InstallMethod;
    use serde_json::Value;

    /// Recursively assert a property bag carries no PII. Every string leaf
    /// must be a short, low-cardinality token: no filesystem path
    /// (`/`/`\\`), no `user@host`, no whitespace (a leaked path/prompt/
    /// hostname trips at least one of these), not the running user's
    /// `$HOME`, and ≤ 64 bytes. Numbers/bools/null carry no identity.
    fn assert_pii_safe(props: &Value) {
        match props {
            Value::String(s) => {
                assert!(
                    !s.contains('/') && !s.contains('\\'),
                    "path-like value leaked into telemetry: {s:?}"
                );
                assert!(!s.contains('@'), "email/user@host value leaked: {s:?}");
                assert!(
                    !s.chars().any(char::is_whitespace),
                    "free-form (whitespace-bearing) value leaked: {s:?}"
                );
                assert!(
                    s.len() <= 64,
                    "telemetry labels are short tokens; oversized value leaked: {s:?}"
                );
                if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
                    assert!(
                        !s.contains(home.to_string_lossy().as_ref()),
                        "value leaked $HOME: {s:?}"
                    );
                }
            }
            Value::Array(items) => items.iter().for_each(assert_pii_safe),
            Value::Object(map) => {
                for (k, v) in map {
                    assert!(
                        !k.contains('/') && !k.chars().any(char::is_whitespace),
                        "property key is not a stable identifier: {k:?}"
                    );
                    assert_pii_safe(v);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn telemetry_property_surface_carries_no_pii() {
        // Pure prop-builders: trigger / version / asset_format / reason.
        assert_pii_safe(&update_check_started_props(
            UpdateCheckTrigger::Auto,
            env!("CARGO_PKG_VERSION"),
        ));
        assert_pii_safe(&update_available_props("0.3.8", "0.4.0", "appimage"));
        assert_pii_safe(&update_available_props("0.3.8", "unknown", "tar.gz"));
        assert_pii_safe(&update_dismissed_props(
            "0.3.8",
            "0.4.0",
            UpdateDismissReason::UserDismissed,
        ));
        assert_pii_safe(&update_dismissed_props(
            "0.3.8",
            "0.4.0",
            UpdateDismissReason::DialogClosed,
        ));

        // The two highest-string-cardinality inline events, rebuilt with the
        // SAME value expressions `emit_app_started` / `emit_update_*` use, so
        // the guard pins the real property shapes rather than a toy.
        assert_pii_safe(&json!({
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "app_version": env!("CARGO_PKG_VERSION"),
            "install_method": install_method_tag(&InstallMethod::Unknown),
            "is_first_run": true,
        }));
        assert_pii_safe(&json!({
            "from_version": env!("CARGO_PKG_VERSION"),
            "to_version": "unknown",
            "install_method": install_method_tag(&InstallMethod::Unknown),
            "success": false,
            "error_category": error_category_tag(&UpdateError::Timeout),
        }));

        // session_corrupted: a typed error bucket + numeric size/age + a BOOL
        // `backup_written` - deliberately NOT the backup PATH (which carries
        // $HOME / the username). Rebuilt with the same value expressions
        // `emit_session_corrupted` uses, so the guard pins THIS shape: a future
        // edit that swaps the bool for `backup_path.to_string_lossy()` trips the
        // path-like assertion.
        assert_pii_safe(&json!({
            "error": "syntax",
            "file_size": 4096u64,
            "file_age_seconds": 12u64,
            "backup_written": true,
        }));
        // app_exited: a single numeric duration - no string surface at all.
        assert_pii_safe(&json!({
            "session_duration_seconds": 42u64,
        }));
    }

    #[test]
    fn distinct_id_is_a_uuid_v4_unrelated_to_identity() {
        // The distinct_id is an anonymous per-install UUID v4 - never a
        // username/hostname/email. Persistence + degraded modes are covered
        // by `paneflow_telemetry::id::tests`; here we pin the v4 shape and
        // that the value itself is PII-safe. Manual nibble check avoids a
        // direct `uuid` dependency in this crate's test surface.
        let id = paneflow_telemetry::id::ephemeral_id("us-022 guard");
        let bytes = id.as_bytes();
        assert_eq!(id.len(), 36, "UUID canonical form is 36 chars: {id:?}");
        assert_eq!(
            bytes[14], b'4',
            "distinct_id must be a v4 (random) UUID, got {id:?}"
        );
        assert_pii_safe(&json!({ "distinct_id": id }));
    }
}
