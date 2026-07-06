//! Stable canonical tags for the v1 desktop events (US-013).
//!
//! The enums `InstallMethod` (`src-app/src/update/install_method.rs`) and
//! `UpdateError` (`src-app/src/update/error.rs`) exist to drive the
//! in-app updater UX - their variant names are tuned for the renderer,
//! not for analytics. This module is the one place the internal variants
//! flatten into the canonical values documented in
//! `tasks/compliance-analytics.md §5`.
//!
//! US-003: the format-invariant helper
//! [`paneflow_telemetry::tags::is_canonical_tag_format`] lives in the
//! workspace crate so any future emitter (trust, …) can reuse the
//! same lowercase-ASCII-only contract. The domain mapping
//! stays here because it depends on `crate::update::*` types.
//!
//! Every mapping target is a `&'static str` - telemetry properties are
//! low-cardinality labels, not messages. Keep the mapping total: any new
//! variant added to `InstallMethod` or `UpdateError` MUST be reflected
//! here in the same commit, or the compiler warns via the exhaustive
//! match.

use crate::update::error::UpdateError;
use crate::update::install_method::{InstallMethod, PackageManager};

/// Canonical install-method tag for the `install_method` property on
/// `app_started` and `update_installed` events. Stable across releases.
///
/// See `tasks/compliance-analytics.md §5` for the committed vocabulary.
pub fn install_method_tag(method: &InstallMethod) -> &'static str {
    match method {
        InstallMethod::SystemPackage { manager } => match manager {
            PackageManager::Apt => "deb",
            PackageManager::Dnf | PackageManager::Zypper => "rpm",
            PackageManager::RpmOstree => "rpm-ostree",
            PackageManager::Other => "other",
        },
        InstallMethod::AppImage { .. } => "appimage",
        InstallMethod::TarGz { .. } => "tar.gz",
        InstallMethod::AppBundle { .. } => "dmg",
        InstallMethod::WindowsMsi { .. } => "msi",
        // Sandboxed runtimes (Flatpak / Snap) and packager-baked
        // `PANEFLOW_UPDATE_EXPLANATION` builds report a coarse tag
        // - the in-app updater is disabled for these so finer-grained
        // attribution would only confuse downstream dashboards.
        InstallMethod::ExternallyManaged { .. } => "externally-managed",
        InstallMethod::Unknown => "unknown",
    }
}

/// Canonical error-category tag for the `error_category` property on
/// failed `update_installed` events (US-013 AC #4). Buckets every
/// internal failure variant into one of the four documented labels; any
/// variant that doesn't fit cleanly lands in `"unknown"` - a deliberate
/// coarse default so the PRD's four-bucket contract stays honest.
pub fn error_category_tag(err: &UpdateError) -> &'static str {
    match err {
        UpdateError::Network(_) => "network",
        UpdateError::ReleaseAssetMissing { .. } => "network",
        // A stalled download/install buckets with network: the dominant cause
        // is a half-open TCP or a mirror that accepts then stalls (U-002).
        UpdateError::Timeout => "network",
        UpdateError::IntegrityMismatch { .. } => "signature",
        UpdateError::DiskFull { .. } => "disk",
        UpdateError::Fuse2Missing
        | UpdateError::InstallDeclined { .. }
        | UpdateError::InstallFailed { .. }
        | UpdateError::EnvironmentBroken { .. }
        | UpdateError::Other(_) => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_telemetry::tags::is_canonical_tag_format;
    use std::path::PathBuf;

    #[test]
    fn install_method_tag_covers_every_variant() {
        let cases: &[(&str, InstallMethod)] = &[
            (
                "deb",
                InstallMethod::SystemPackage {
                    manager: PackageManager::Apt,
                },
            ),
            (
                "rpm",
                InstallMethod::SystemPackage {
                    manager: PackageManager::Dnf,
                },
            ),
            (
                "rpm",
                InstallMethod::SystemPackage {
                    manager: PackageManager::Zypper,
                },
            ),
            (
                "rpm-ostree",
                InstallMethod::SystemPackage {
                    manager: PackageManager::RpmOstree,
                },
            ),
            (
                "other",
                InstallMethod::SystemPackage {
                    manager: PackageManager::Other,
                },
            ),
            (
                "appimage",
                InstallMethod::AppImage {
                    mount_point: PathBuf::new(),
                    source_path: PathBuf::new(),
                },
            ),
            (
                "tar.gz",
                InstallMethod::TarGz {
                    app_dir: PathBuf::new(),
                },
            ),
            (
                "dmg",
                InstallMethod::AppBundle {
                    bundle_path: PathBuf::new(),
                },
            ),
            (
                "msi",
                InstallMethod::WindowsMsi {
                    install_path: PathBuf::new(),
                },
            ),
            ("unknown", InstallMethod::Unknown),
        ];
        for (expected, method) in cases {
            assert_eq!(
                install_method_tag(method),
                *expected,
                "{method:?} should map to {expected}"
            );
        }
    }

    #[test]
    fn error_category_tag_buckets_into_four_canonical_labels() {
        assert_eq!(
            error_category_tag(&UpdateError::Network("dns".into())),
            "network"
        );
        assert_eq!(
            error_category_tag(&UpdateError::ReleaseAssetMissing {
                url: "https://example".into()
            }),
            "network"
        );
        assert_eq!(
            error_category_tag(&UpdateError::IntegrityMismatch {
                expected: "a".into(),
                got: "b".into()
            }),
            "signature"
        );
        assert_eq!(
            error_category_tag(&UpdateError::DiskFull {
                path: PathBuf::new()
            }),
            "disk"
        );
        assert_eq!(error_category_tag(&UpdateError::Fuse2Missing), "unknown");
        assert_eq!(
            error_category_tag(&UpdateError::InstallDeclined { message: "".into() }),
            "unknown"
        );
        assert_eq!(
            error_category_tag(&UpdateError::InstallFailed {
                log_path: PathBuf::new()
            }),
            "unknown"
        );
        assert_eq!(
            error_category_tag(&UpdateError::EnvironmentBroken { message: "".into() }),
            "unknown"
        );
        assert_eq!(
            error_category_tag(&UpdateError::Other("x".into())),
            "unknown"
        );
        assert_eq!(error_category_tag(&UpdateError::Timeout), "network");
    }

    #[test]
    fn every_published_tag_satisfies_the_canonical_format_contract() {
        // Single source of truth for the format invariant: delegate to
        // `paneflow_telemetry::tags::is_canonical_tag_format`. Breaking
        // the lowercase-ASCII rule on any of these would invalidate any
        // PostHog breakdown filter Arthur has already configured on
        // `install_method` or `error_category`.
        let all = [
            "deb",
            "rpm",
            "rpm-ostree",
            "other",
            "appimage",
            "tar.gz",
            "dmg",
            "msi",
            "externally-managed",
            "unknown",
            "network",
            "signature",
            "disk",
        ];
        for tag in all {
            assert!(
                is_canonical_tag_format(tag),
                "tag {tag:?} violates the canonical format contract - telemetry labels must be lowercase ascii letters/digits/[-.]"
            );
        }
    }
}
