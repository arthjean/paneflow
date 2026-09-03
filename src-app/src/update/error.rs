use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateError {
    Network(#[allow(dead_code)] String),
    IntegrityMismatch {
        expected: String,
        got: String,
    },
    Fuse2Missing,
    DiskFull {
        path: PathBuf,
    },
    ReleaseAssetMissing {
        url: String,
    },
    InstallDeclined {
        message: String,
    },
    InstallFailed {
        log_path: PathBuf,
    },
    #[allow(dead_code)]
    EnvironmentBroken {
        message: String,
    },
    Timeout,
    Other(String),
}

impl UpdateError {
    pub fn user_message(&self) -> String {
        match self {
            UpdateError::Network(_) => {
                "Update failed: no connection. Retry when online.".to_string()
            }
            UpdateError::IntegrityMismatch { .. } => {
                "Update failed: downloaded file is corrupt or tampered. Retry or download manually."
                    .to_string()
            }
            UpdateError::Fuse2Missing => {
                "Update requires FUSE 2. Run: `./paneflow-*.AppImage --appimage-extract-and-run` - or install libfuse2."
                    .to_string()
            }
            UpdateError::DiskFull { path } => {
                if path.as_os_str().is_empty() {
                    "Update failed: disk full. Free space and retry.".to_string()
                } else {
                    format!(
                        "Update failed: disk full at `{}`. Free space and retry.",
                        path.display()
                    )
                }
            }
            UpdateError::ReleaseAssetMissing { url } => format!(
                "Update blocked: a required asset is no longer published ({url}). Please file a bug - PaneFlow needs a refreshed release pin."
            ),
            UpdateError::InstallDeclined { message } => message.clone(),
            UpdateError::InstallFailed { log_path } => {
                if log_path.as_os_str().is_empty() {
                    "Update install failed. Retry later, or update via your package manager directly.".to_string()
                } else {
                    format!(
                        "Update install failed. Verbose log saved to `{}` - attach it to a bug report.",
                        log_path.display()
                    )
                }
            }
            UpdateError::EnvironmentBroken { message } => message.clone(),
            UpdateError::Timeout => {
                "Update timed out. The download or install stalled - retry when your connection is stable."
                    .to_string()
            }
            UpdateError::Other(msg) => msg.clone(),
        }
    }

    pub fn classify(err: &anyhow::Error) -> Self {
        for cause in err.chain() {
            if let Some(tag) = cause.downcast_ref::<UpdateError>() {
                return tag.clone();
            }
            if let Some(mm) = cause.downcast_ref::<IntegrityMismatch>() {
                return UpdateError::IntegrityMismatch {
                    expected: mm.expected.clone(),
                    got: mm.got.clone(),
                };
            }
            if let Some(ureq::Error::Timeout(_)) = cause.downcast_ref::<ureq::Error>() {
                return UpdateError::Network(format!("{err:#}"));
            }
            if let Some(io) = cause.downcast_ref::<std::io::Error>()
                && is_disk_full(io)
            {
                return UpdateError::DiskFull {
                    path: PathBuf::new(),
                };
            }
        }
        let full = format!("{err:#}");
        let lower = full.to_ascii_lowercase();
        if lower.contains("libfuse.so.2")
            || lower.contains("libfuse2")
            || lower.contains("appimage-extract-and-run")
            || lower.contains("failed to exec fusermount")
        {
            return UpdateError::Fuse2Missing;
        }
        if lower.contains("no space left") || lower.contains("disk full") {
            return UpdateError::DiskFull {
                path: PathBuf::new(),
            };
        }
        if lower.contains("failed integrity check")
            || lower.contains("integrity check")
            || lower.contains("checksum")
            || lower.contains("hash mismatch")
        {
            return UpdateError::IntegrityMismatch {
                expected: String::new(),
                got: String::new(),
            };
        }
        if lower.contains("exceeded its deadline") {
            return UpdateError::Timeout;
        }
        if lower.contains("could not fetch integrity checksum")
            || lower.contains("could not download update")
            || lower.contains("could not download update tool")
            || lower.contains("try again when online")
            || lower.contains("could not resolve host")
            || lower.contains("could not connect")
            || lower.contains("failed to connect")
            || lower.contains("network is unreachable")
            || lower.contains("no such host")
            || lower.contains("timed out")
            || lower.contains("timeout")
        {
            return UpdateError::Network(full);
        }
        UpdateError::Other(full)
    }
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.user_message())
    }
}

impl std::error::Error for UpdateError {}

#[derive(Debug, Clone)]
pub struct IntegrityMismatch {
    pub expected: String,
    pub got: String,
}

impl std::fmt::Display for IntegrityMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Downloaded file failed integrity check. Retry or download manually. (expected {}, got {})",
            self.expected, self.got
        )
    }
}

impl std::error::Error for IntegrityMismatch {}

pub fn is_disk_full(err: &std::io::Error) -> bool {
    if matches!(err.kind(), std::io::ErrorKind::StorageFull) {
        return true;
    }
    #[cfg(unix)]
    {
        if err.raw_os_error() == Some(libc::ENOSPC) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn io_err(kind: std::io::ErrorKind) -> std::io::Error {
        std::io::Error::new(kind, "synthetic")
    }

    #[test]
    fn classify_direct_update_error_roundtrips() {
        let tagged = UpdateError::Fuse2Missing;
        let err = anyhow::Error::new(tagged);
        assert_eq!(UpdateError::classify(&err), UpdateError::Fuse2Missing);
    }

    #[test]
    fn timeout_variant_roundtrips_and_has_user_copy() {
        let err = anyhow::Error::new(UpdateError::Timeout);
        assert_eq!(UpdateError::classify(&err), UpdateError::Timeout);
        assert!(!UpdateError::Timeout.user_message().is_empty());
    }

    #[test]
    fn classify_recovers_tag_through_context_wrapping() {
        let err = anyhow::Error::new(UpdateError::Network("ureq hit EOF".into()))
            .context("fetch release asset")
            .context("self-update/targz");
        assert!(matches!(
            UpdateError::classify(&err),
            UpdateError::Network(_)
        ));
    }

    #[test]
    fn classify_extracts_integrity_mismatch_digests() {
        let mm = IntegrityMismatch {
            expected: "a".repeat(64),
            got: "b".repeat(64),
        };
        let err = anyhow::Error::new(mm)
            .context("download asset")
            .context("self-update/targz");
        match UpdateError::classify(&err) {
            UpdateError::IntegrityMismatch { expected, got } => {
                assert_eq!(expected, "a".repeat(64));
                assert_eq!(got, "b".repeat(64));
            }
            other => panic!("expected IntegrityMismatch, got {other:?}"),
        }
    }

    #[test]
    fn classify_disk_full_via_storage_full_kind() {
        let err = anyhow::Error::new(io_err(std::io::ErrorKind::StorageFull))
            .context("write chunk to disk");
        assert!(matches!(
            UpdateError::classify(&err),
            UpdateError::DiskFull { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn classify_disk_full_via_raw_errno() {
        let err = anyhow::Error::new(std::io::Error::from_raw_os_error(libc::ENOSPC))
            .context("create cache dir");
        assert!(matches!(
            UpdateError::classify(&err),
            UpdateError::DiskFull { .. }
        ));
    }

    #[test]
    fn classify_disk_full_via_substring_fallback() {
        let err = anyhow::anyhow!("extract tar.gz into scratch dir: No space left on device");
        assert!(matches!(
            UpdateError::classify(&err),
            UpdateError::DiskFull { .. }
        ));
    }

    #[test]
    fn classify_network_via_context_message() {
        let err = anyhow::anyhow!("Could not download update. Try again when online.");
        assert!(matches!(
            UpdateError::classify(&err),
            UpdateError::Network(_)
        ));
    }

    #[test]
    fn classify_network_via_resolve_host() {
        let err = anyhow::anyhow!("curl: (6) Could not resolve host: github.com");
        assert!(matches!(
            UpdateError::classify(&err),
            UpdateError::Network(_)
        ));
    }

    #[test]
    fn classify_fuse2_missing_variants() {
        for msg in [
            "error while loading shared libraries: libfuse.so.2",
            "failed to exec fusermount: No such file or directory",
            "try running with --appimage-extract-and-run",
            "libfuse2 is not installed",
        ] {
            let err = anyhow::Error::msg(msg.to_string());
            assert!(
                matches!(UpdateError::classify(&err), UpdateError::Fuse2Missing),
                "msg {msg:?} → {:?}",
                UpdateError::classify(&err)
            );
        }
    }

    #[test]
    fn classify_integrity_via_keyword_fallback() {
        let err = anyhow::anyhow!("zsync2: checksum verification failed");
        assert!(matches!(
            UpdateError::classify(&err),
            UpdateError::IntegrityMismatch { .. }
        ));
    }

    #[test]
    fn classify_ureq_timeout_variant_as_network() {
        let err = anyhow::Error::new(ureq::Error::Timeout(ureq::Timeout::Global))
            .context("update checker main loop");
        assert!(matches!(
            UpdateError::classify(&err),
            UpdateError::Network(_)
        ));
    }

    #[test]
    fn classify_integrity_keyword_shadowed_by_timeout_substring() {
        let err = anyhow::anyhow!("zsync2: checksum timed out waiting for block");
        assert!(matches!(
            UpdateError::classify(&err),
            UpdateError::IntegrityMismatch { .. }
        ));
    }

    #[test]
    fn classify_ureq_timeout_via_substring_fallback() {
        let err = anyhow::anyhow!("stream tarball to disk: request timed out");
        assert!(matches!(
            UpdateError::classify(&err),
            UpdateError::Network(_)
        ));
        let err = anyhow::anyhow!("ureq: timeout");
        assert!(matches!(
            UpdateError::classify(&err),
            UpdateError::Network(_)
        ));
    }

    #[test]
    fn classify_process_deadline_as_timeout() {
        let err = anyhow::anyhow!("process exceeded its deadline and was killed");
        assert!(matches!(UpdateError::classify(&err), UpdateError::Timeout));
    }

    #[test]
    fn classify_other_for_unclassifiable_error() {
        let err = anyhow::anyhow!("some totally unexpected garbage");
        match UpdateError::classify(&err) {
            UpdateError::Other(msg) => assert!(msg.contains("unexpected garbage")),
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn user_message_matches_prd_copy_network() {
        assert_eq!(
            UpdateError::Network("any".into()).user_message(),
            "Update failed: no connection. Retry when online."
        );
    }

    #[test]
    fn user_message_matches_prd_copy_integrity() {
        assert_eq!(
            UpdateError::IntegrityMismatch {
                expected: "a".into(),
                got: "b".into(),
            }
            .user_message(),
            "Update failed: downloaded file is corrupt or tampered. Retry or download manually."
        );
    }

    #[test]
    fn user_message_matches_prd_copy_fuse2() {
        let got = UpdateError::Fuse2Missing.user_message();
        assert!(got.contains("FUSE 2"));
        assert!(got.contains("--appimage-extract-and-run"));
        assert!(got.contains("libfuse2"));
    }

    #[test]
    fn user_message_disk_full_includes_path_when_set() {
        let err = UpdateError::DiskFull {
            path: PathBuf::from("/home/u/.cache/paneflow"),
        };
        let msg = err.user_message();
        assert!(msg.contains("disk full"));
        assert!(msg.contains("/home/u/.cache/paneflow"));
        assert!(msg.contains("Free space and retry"));
    }

    #[test]
    fn user_message_disk_full_omits_path_when_empty() {
        let err = UpdateError::DiskFull {
            path: PathBuf::new(),
        };
        let msg = err.user_message();
        assert!(msg.contains("disk full"));
        assert!(!msg.contains("at `"));
    }

    #[test]
    fn user_message_other_passes_through_raw() {
        let err = UpdateError::Other("raw detail".into());
        assert_eq!(err.user_message(), "raw detail");
    }

    #[test]
    fn classify_release_asset_missing_roundtrips() {
        let tagged = UpdateError::ReleaseAssetMissing {
            url: "https://example.test/asset.AppImage".into(),
        };
        let err = anyhow::Error::new(tagged.clone()).context("self-update/appimage");
        assert_eq!(UpdateError::classify(&err), tagged);
    }

    #[test]
    fn user_message_release_asset_missing_includes_url() {
        let err = UpdateError::ReleaseAssetMissing {
            url: "https://example.test/tool.AppImage".into(),
        };
        let msg = err.user_message();
        assert!(
            msg.contains("https://example.test/tool.AppImage"),
            "got: {msg}"
        );
        assert!(msg.contains("no longer published"), "got: {msg}");
    }

    #[test]
    fn classify_install_declined_roundtrips() {
        let tagged = UpdateError::InstallDeclined {
            message: "Unable to replace /Applications/PaneFlow.app - reinstall manually".into(),
        };
        let err = anyhow::Error::new(tagged.clone()).context("self-update/dmg");
        assert_eq!(UpdateError::classify(&err), tagged);
    }

    #[test]
    fn user_message_install_declined_passes_through() {
        let err = UpdateError::InstallDeclined {
            message: "Unable to replace /Applications/PaneFlow.app - reinstall manually".into(),
        };
        assert_eq!(
            err.user_message(),
            "Unable to replace /Applications/PaneFlow.app - reinstall manually"
        );
    }

    #[test]
    fn classify_install_failed_roundtrips() {
        let tagged = UpdateError::InstallFailed {
            log_path: PathBuf::from("C:\\Users\\u\\AppData\\Local\\Temp\\paneflow-msi-1234.log"),
        };
        let err = anyhow::Error::new(tagged.clone()).context("self-update/msi");
        assert_eq!(UpdateError::classify(&err), tagged);
    }

    #[test]
    fn user_message_install_failed_includes_log_path() {
        let err = UpdateError::InstallFailed {
            log_path: PathBuf::from("C:\\Temp\\paneflow-msi-9.log"),
        };
        let msg = err.user_message();
        assert!(msg.contains("C:\\Temp\\paneflow-msi-9.log"), "got: {msg}");
        assert!(msg.contains("Update install failed"), "got: {msg}");
    }

    #[test]
    fn classify_environment_broken_roundtrips() {
        let tagged = UpdateError::EnvironmentBroken {
            message: "msiexec.exe not found on PATH - Windows system install appears broken".into(),
        };
        let err = anyhow::Error::new(tagged.clone()).context("self-update/msi");
        assert_eq!(UpdateError::classify(&err), tagged);
    }

    #[test]
    fn user_message_environment_broken_passes_through() {
        let err = UpdateError::EnvironmentBroken {
            message: "msiexec.exe not found on PATH".into(),
        };
        assert_eq!(err.user_message(), "msiexec.exe not found on PATH");
    }

    #[test]
    fn classify_recovers_install_declined_through_pkexec_context() {
        let tagged = UpdateError::InstallDeclined {
            message: "Authentication cancelled".into(),
        };
        let err = anyhow::Error::new(tagged.clone()).context("pkexec exited with code 126");
        assert_eq!(UpdateError::classify(&err), tagged);
    }

    #[test]
    fn classify_recovers_environment_broken_through_pkexec_context() {
        let tagged = UpdateError::EnvironmentBroken {
            message: "pkexec returned 127 (no polkit agent or command missing)".into(),
        };
        let err = anyhow::Error::new(tagged.clone()).context("pkexec exited with code 127");
        assert_eq!(UpdateError::classify(&err), tagged);
    }

    #[test]
    fn classify_recovers_install_failed_through_pkexec_context() {
        let tagged = UpdateError::InstallFailed {
            log_path: PathBuf::new(),
        };
        let err = anyhow::Error::new(tagged.clone()).context("pkexec exited with code 1");
        assert_eq!(UpdateError::classify(&err), tagged);
    }

    #[test]
    fn user_message_install_failed_handles_empty_path() {
        let err = UpdateError::InstallFailed {
            log_path: PathBuf::new(),
        };
        let msg = err.user_message();
        assert!(msg.contains("Update install failed"), "got: {msg}");
        assert!(
            !msg.contains("``"),
            "empty-backticks leaked into user-visible copy: {msg}"
        );
        assert!(
            !msg.contains("`` -"),
            "empty-path placeholder leaked into user-visible copy: {msg}"
        );
        assert!(
            msg.to_ascii_lowercase().contains("package manager"),
            "empty-path branch should point the user at their package manager: {msg}"
        );
        assert!(
            !msg.contains("Verbose log saved"),
            "empty-path branch must not advertise a log file that does not exist: {msg}"
        );
    }

    #[test]
    fn classify_recovers_other_through_pkexec_signal_context() {
        let tagged = UpdateError::Other("package manager killed by signal 9".into());
        let err = anyhow::Error::new(tagged.clone()).context("pkexec killed by signal 9");
        assert_eq!(UpdateError::classify(&err), tagged);
    }
}
