//! Stable canonical tags for the v1 desktop events (US-013).
//!
//! The enums `InstallMethod` (`src-app/src/update/install_method.rs`) and
//! `UpdateError` (`src-app/src/update/error.rs`) exist to drive the
//! in-app updater UX - their variant names are tuned for the renderer,
//! not for analytics. This module is the one place the internal variants
//! flatten into closed telemetry enums. The domain mapping stays here because
//! it depends on `crate::update::*` types. Exhaustive matches force every new
//! updater variant to choose an explicit analytics value.

use crate::update::error::UpdateError;
use crate::update::install_method::{InstallMethod, PackageManager};
use paneflow_telemetry::event::{InstallMethod as TelemetryInstallMethod, UpdateErrorCategory};

/// Closed install-method value for desktop telemetry events.
pub fn install_method_value(method: &InstallMethod) -> TelemetryInstallMethod {
    match method {
        InstallMethod::SystemPackage { manager } => match manager {
            PackageManager::Apt => TelemetryInstallMethod::Deb,
            PackageManager::Dnf | PackageManager::Zypper => TelemetryInstallMethod::Rpm,
            PackageManager::RpmOstree => TelemetryInstallMethod::RpmOstree,
            PackageManager::Other => TelemetryInstallMethod::Other,
        },
        InstallMethod::AppImage { .. } => TelemetryInstallMethod::AppImage,
        InstallMethod::TarGz { .. } => TelemetryInstallMethod::TarGz,
        InstallMethod::AppBundle { .. } => TelemetryInstallMethod::Dmg,
        InstallMethod::WindowsMsi { .. } => TelemetryInstallMethod::Msi,
        // Sandboxed runtimes (Flatpak / Snap) and packager-baked
        // `PANEFLOW_UPDATE_EXPLANATION` builds report a coarse tag
        // - the in-app updater is disabled for these so finer-grained
        // attribution would only confuse downstream dashboards.
        InstallMethod::ExternallyManaged { .. } => TelemetryInstallMethod::ExternallyManaged,
        InstallMethod::Unknown => TelemetryInstallMethod::Unknown,
    }
}

/// Canonical error-category tag for the `error_category` property on
/// failed `update_installed` events (US-013 AC #4). Buckets every
/// internal failure variant into one of the four documented labels; any
/// variant that doesn't fit cleanly lands in `"unknown"` - a deliberate
/// coarse default so the PRD's four-bucket contract stays honest.
pub fn update_error_category(err: &UpdateError) -> UpdateErrorCategory {
    match err {
        UpdateError::Network(_) => UpdateErrorCategory::Network,
        UpdateError::ReleaseAssetMissing { .. } => UpdateErrorCategory::Network,
        // A stalled download/install buckets with network: the dominant cause
        // is a half-open TCP or a mirror that accepts then stalls (U-002).
        UpdateError::Timeout => UpdateErrorCategory::Network,
        UpdateError::IntegrityMismatch { .. } => UpdateErrorCategory::Signature,
        UpdateError::DiskFull { .. } => UpdateErrorCategory::Disk,
        UpdateError::Fuse2Missing
        | UpdateError::InstallDeclined { .. }
        | UpdateError::InstallFailed { .. }
        | UpdateError::EnvironmentBroken { .. }
        | UpdateError::Other(_) => UpdateErrorCategory::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn install_method_mapping_covers_every_variant() {
        let cases: &[(TelemetryInstallMethod, InstallMethod)] = &[
            (
                TelemetryInstallMethod::Deb,
                InstallMethod::SystemPackage {
                    manager: PackageManager::Apt,
                },
            ),
            (
                TelemetryInstallMethod::Rpm,
                InstallMethod::SystemPackage {
                    manager: PackageManager::Dnf,
                },
            ),
            (
                TelemetryInstallMethod::Rpm,
                InstallMethod::SystemPackage {
                    manager: PackageManager::Zypper,
                },
            ),
            (
                TelemetryInstallMethod::RpmOstree,
                InstallMethod::SystemPackage {
                    manager: PackageManager::RpmOstree,
                },
            ),
            (
                TelemetryInstallMethod::Other,
                InstallMethod::SystemPackage {
                    manager: PackageManager::Other,
                },
            ),
            (
                TelemetryInstallMethod::AppImage,
                InstallMethod::AppImage {
                    mount_point: PathBuf::new(),
                    source_path: PathBuf::new(),
                },
            ),
            (
                TelemetryInstallMethod::TarGz,
                InstallMethod::TarGz {
                    app_dir: PathBuf::new(),
                },
            ),
            (
                TelemetryInstallMethod::Dmg,
                InstallMethod::AppBundle {
                    bundle_path: PathBuf::new(),
                },
            ),
            (
                TelemetryInstallMethod::Msi,
                InstallMethod::WindowsMsi {
                    install_path: PathBuf::new(),
                },
            ),
            (
                TelemetryInstallMethod::ExternallyManaged,
                InstallMethod::ExternallyManaged {
                    explanation: "managed".to_string(),
                },
            ),
            (TelemetryInstallMethod::Unknown, InstallMethod::Unknown),
        ];
        for (expected, method) in cases {
            assert_eq!(
                install_method_value(method),
                *expected,
                "{method:?} should map to {expected:?}"
            );
        }
    }

    #[test]
    fn update_error_category_buckets_into_four_values() {
        assert_eq!(
            update_error_category(&UpdateError::Network("dns".into())),
            UpdateErrorCategory::Network
        );
        assert_eq!(
            update_error_category(&UpdateError::ReleaseAssetMissing {
                url: "https://example".into()
            }),
            UpdateErrorCategory::Network
        );
        assert_eq!(
            update_error_category(&UpdateError::IntegrityMismatch {
                expected: "a".into(),
                got: "b".into()
            }),
            UpdateErrorCategory::Signature
        );
        assert_eq!(
            update_error_category(&UpdateError::DiskFull {
                path: PathBuf::new()
            }),
            UpdateErrorCategory::Disk
        );
        assert_eq!(
            update_error_category(&UpdateError::Fuse2Missing),
            UpdateErrorCategory::Unknown
        );
        assert_eq!(
            update_error_category(&UpdateError::InstallDeclined { message: "".into() }),
            UpdateErrorCategory::Unknown
        );
        assert_eq!(
            update_error_category(&UpdateError::InstallFailed {
                log_path: PathBuf::new()
            }),
            UpdateErrorCategory::Unknown
        );
        assert_eq!(
            update_error_category(&UpdateError::EnvironmentBroken { message: "".into() }),
            UpdateErrorCategory::Unknown
        );
        assert_eq!(
            update_error_category(&UpdateError::Other("x".into())),
            UpdateErrorCategory::Unknown
        );
        assert_eq!(
            update_error_category(&UpdateError::Timeout),
            UpdateErrorCategory::Network
        );
    }
}
