use std::time::Duration;

use crate::PaneFlowApp;
use crate::app::session::SessionCorruptionInfo;
use crate::telemetry::event::{
    Architecture, OperatingSystem, SessionErrorCategory, TelemetryEvent, TelemetryVersion,
    UpdateAssetFormat,
};
use crate::telemetry::tags::{install_method_value, update_error_category};
use crate::update::{self, UpdateError};

const SHUTDOWN_FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

impl PaneFlowApp {
    pub(crate) fn emit_session_corrupted(&self, info: &SessionCorruptionInfo) {
        let Some(error) = SessionErrorCategory::from_tag(info.error_category) else {
            log::debug!(
                "telemetry: unknown session error category {:?}; event dropped",
                info.error_category
            );
            return;
        };
        self.telemetry.capture(TelemetryEvent::session_corrupted(
            error,
            info.file_size,
            info.file_age_seconds,
            info.backup_path.is_some(),
        ));
    }

    pub(crate) fn emit_app_started(&self, is_first_run: bool) {
        let Some(app_version) = telemetry_version(env!("CARGO_PKG_VERSION")) else {
            return;
        };
        self.telemetry.capture(TelemetryEvent::app_started(
            OperatingSystem::current(),
            Architecture::current(),
            app_version,
            install_method_value(&self.self_update.install_method),
            is_first_run,
        ));
    }

    pub(crate) fn emit_app_exited_and_flush(&self) {
        self.telemetry.capture(TelemetryEvent::app_exited(
            self.launch_instant.elapsed().as_secs(),
        ));
        self.telemetry.flush_blocking(SHUTDOWN_FLUSH_TIMEOUT);
    }

    pub(crate) fn emit_update_success(&self) {
        let Some(from_version) = telemetry_version(env!("CARGO_PKG_VERSION")) else {
            return;
        };
        let to_version = match self.self_update.update_status.as_ref() {
            Some(update::checker::UpdateStatus::Available { version, .. }) => {
                TelemetryVersion::parse(version)
            }
            _ => None,
        };
        self.telemetry.capture(TelemetryEvent::update_installed(
            from_version,
            to_version,
            install_method_value(&self.self_update.install_method),
        ));
    }

    pub(crate) fn emit_update_failure(&self, error: &UpdateError) {
        let Some(from_version) = telemetry_version(env!("CARGO_PKG_VERSION")) else {
            return;
        };
        let to_version = match self.self_update.update_status.as_ref() {
            Some(update::checker::UpdateStatus::Available { version, .. }) => {
                TelemetryVersion::parse(version)
            }
            _ => None,
        };
        self.telemetry
            .capture(TelemetryEvent::update_install_failed(
                from_version,
                to_version,
                install_method_value(&self.self_update.install_method),
                update_error_category(error),
            ));
    }

    pub(crate) fn emit_update_dismissed(&self) {
        let to_version = match self.self_update.update_status.as_ref() {
            Some(update::checker::UpdateStatus::Available { version, .. }) => version.as_str(),
            _ => "unknown",
        };
        emit_update_dismissed_via(&self.telemetry, env!("CARGO_PKG_VERSION"), to_version);
    }
}

fn telemetry_version(value: &str) -> Option<TelemetryVersion> {
    let version = TelemetryVersion::parse(value);
    if version.is_none() {
        log::debug!("telemetry: invalid release version; event dropped");
    }
    version
}

pub(crate) fn emit_update_check_started(
    telemetry: &crate::telemetry::client::TelemetryClient,
    current_version: &str,
) {
    let Some(current_version) = telemetry_version(current_version) else {
        return;
    };
    telemetry.capture(TelemetryEvent::update_check_started(current_version));
}

pub(crate) fn emit_update_available(
    telemetry: &crate::telemetry::client::TelemetryClient,
    from_version: &str,
    to_version: &str,
    asset_format: UpdateAssetFormat,
) {
    let (Some(from_version), Some(to_version)) = (
        telemetry_version(from_version),
        telemetry_version(to_version),
    ) else {
        return;
    };
    telemetry.capture(TelemetryEvent::update_available(
        from_version,
        to_version,
        asset_format,
    ));
}

pub(crate) fn emit_update_dismissed_via(
    telemetry: &crate::telemetry::client::TelemetryClient,
    from_version: &str,
    to_version: &str,
) {
    let Some(from_version) = telemetry_version(from_version) else {
        return;
    };
    telemetry.capture(TelemetryEvent::update_dismissed(
        from_version,
        TelemetryVersion::parse(to_version),
    ));
}

#[cfg(test)]
mod tests {
    #[test]
    fn distinct_id_is_an_anonymous_uuid_v4() {
        let id = paneflow_telemetry::id::ephemeral_id("telemetry test");
        let bytes = id.as_bytes();
        assert_eq!(id.len(), 36);
        assert_eq!(bytes[14], b'4');
        assert!(!id.contains('/') && !id.contains('\\') && !id.contains('@'));
    }
}
