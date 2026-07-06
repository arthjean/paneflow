//! Structured classification of update failures (US-013) plus the `is_disk_full`
//! predicate. The main thread calls [`UpdateError::classify`] at the boundary to
//! bucket `anyhow::Error`s into renderable variants; the toast renderer picks
//! its copy per variant (`Network`, `IntegrityMismatch`, `Fuse2Missing`,
//! `DiskFull`, `Other`).
//!
//! Extracted from `self_update/mod.rs` per US-031. The `libc::ENOSPC` arm of
//! `is_disk_full` is gated `#[cfg(unix)]` per US-034 - on Windows the
//! `std::io::ErrorKind::StorageFull` branch alone is sufficient.

use std::path::PathBuf;

/// Structured, user-facing classification of an update failure (US-013).
///
/// Lower layers return `anyhow::Error`; the main thread calls
/// [`UpdateError::classify`] at the boundary to bucket the failure into one
/// of the variants the title bar knows how to render. Keep variants
/// *exhaustive from the renderer's perspective*: adding a new failure mode
/// without a matching toast string is a UX regression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateError {
    /// DNS/TCP/TLS/socket-level failure. The wrapped string is preserved for
    /// logs; the toast uses a fixed human message.
    Network(#[allow(dead_code)] String),
    /// SHA-256 of the downloaded asset did not match its `.sha256` sibling.
    IntegrityMismatch { expected: String, got: String },
    /// AppImage runtime needs libfuse2 and it's not installed. The toast
    /// suggests `--appimage-extract-and-run` as an immediate workaround.
    Fuse2Missing,
    /// ENOSPC on a write inside the update flow. `path` is best-effort - we
    /// don't always know which write failed, in which case this is an empty
    /// `PathBuf` and the toast renders without the "at {path}" clause.
    DiskFull { path: PathBuf },
    /// Pinned release asset returned 404 (US-005). Distinct from `Network`:
    /// the user isn't offline, the upstream dated tag or asset path
    /// disappeared - PaneFlow itself needs a new release with an updated
    /// pin. `url` is the missing asset so the user can report it verbatim.
    ReleaseAssetMissing { url: String },
    /// Install step was declined by the OS or user (US-009 / US-010 /
    /// US-002-pkexec). Distinct from `DiskFull` / `Other`: the update
    /// was downloaded and verified cleanly, but the actual installation
    /// couldn't proceed - typical causes are `/Applications/` being
    /// non-writable, SIP blocking the replace, Windows UAC cancel
    /// (msiexec 1602), polkit auth cancel on Linux (pkexec exit 126),
    /// or the running process holding a lock on the install path. The
    /// wrapped `message` is user-visible verbatim so the toast can be
    /// specific about the reason ("reinstall manually",
    /// "administrator required", "Authentication cancelled").
    InstallDeclined { message: String },
    /// A privileged installer returned a fatal error after the user
    /// consented and the OS attempted the install. Covers msiexec
    /// (US-010, typically 1603) and pkexec-wrapped `dnf`/`apt-get`
    /// (US-002-pkexec, any non-zero exit that isn't 126/127). Distinct
    /// from `InstallDeclined`: the transaction was started, not
    /// cancelled. Causes include disk corruption, a conflicting
    /// component, an orphaned prior install, a 404 mirror, or a
    /// conflicting package lock. `log_path` points at the `/l*v`
    /// verbose log for msiexec; for pkexec/dnf/apt there is no log
    /// file (stderr is dumped at `log::debug!` inside the runner), so
    /// the path is `PathBuf::new()` and `user_message` falls back to
    /// the "check the PaneFlow log" variant.
    InstallFailed { log_path: PathBuf },
    /// A critical OS tool the updater depends on was not found on
    /// PATH, or a required subsystem is missing. On Windows this
    /// means `msiexec.exe` is missing from `%SystemRoot%\System32\`
    /// (US-010); on Linux this means `pkexec` is not installed, or
    /// pkexec returned 127 because no polkit agent is running in the
    /// current session (US-002-pkexec). Either way the user's install
    /// is broken in a way PaneFlow can't self-fix. The wrapped
    /// `message` is user-visible verbatim so the toast can name the
    /// missing tool and suggest a reinstall / polkit-agent path.
    #[allow(dead_code)]
    EnvironmentBroken { message: String },
    /// A bounded external subprocess in the update flow (the AppImage
    /// `appimageupdatetool` zsync download, or any other installer tool)
    /// exceeded its wall-clock deadline and was killed (EP-002, U-002/U-015),
    /// or the whole `Downloading` state was reset by the worker watchdog.
    /// Distinct from `Network`: the transport may be fine but the tool hung
    /// (half-open TCP, a mirror that accepts then stalls, a wedged future).
    /// The download/install never completed; the user can retry.
    Timeout,
    /// Classifier couldn't bucket the error. The wrapped message is shown
    /// verbatim so the user sees *something* actionable instead of a
    /// generic "update failed".
    Other(String),
}

impl UpdateError {
    /// Render the PRD-mandated toast copy for this variant. The strings are
    /// intentionally frozen - changing them in one place but not the other
    /// would break US-013 acceptance tests.
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
                // Empty path = pkexec/dnf/apt branch (US-002-pkexec) -
                // the CLI package managers don't write a self-contained
                // log file; their stderr is captured at `log::debug!`
                // by the runner instead. "PaneFlow log" would be
                // misleading here because ordinary users don't run
                // with `RUST_LOG=debug`. msiexec (US-010) always sets
                // a concrete `/l*v` path and hits the other branch.
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

    /// Bucket an `anyhow::Error` into a variant.
    ///
    /// Preference order (most specific first):
    ///   1. Downcast to `UpdateError` - lower layers can bail with a
    ///      pre-classified error for free.
    ///   2. Downcast to [`IntegrityMismatch`] - carries `expected`/`got`.
    ///   3. Walk the chain looking for `std::io::Error` with ENOSPC.
    ///   4. Substring-match on the formatted error chain for FUSE /
    ///      network / integrity / disk-full keywords.
    ///   5. Fall back to `Other` with the raw formatted message.
    pub fn classify(err: &anyhow::Error) -> Self {
        // Walk the full cause chain. `Error::downcast_ref` only inspects the
        // outermost error, which would miss a pre-classified tag wrapped by
        // `.context(...)` - probe every cause so callers are free to add
        // context without losing classification.
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
            // US-001: the update flow bounds every ureq call with a 30 s
            // global timeout. When it fires at the request/response layer,
            // the error surfaces as `ureq::Error::Timeout(_)` - treat it as
            // a network failure so the title bar renders the "no connection"
            // toast instead of the generic `Other` catch-all.
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
        // Integrity keywords are checked BEFORE network keywords so a
        // crafted error like "checksum timed out" cannot silently route a
        // genuine integrity failure into the Network toast. Typed
        // `IntegrityMismatch` downcasts happen earlier in the cause-chain
        // walk, so real SHA mismatches are unaffected - this ordering only
        // matters for stringly-typed errors coming from external tools.
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
            // US-001: body-stream timeouts arrive as io::Error wrapped by
            // std::io::copy - the typed downcast above misses them because
            // the chain no longer carries the original ureq::Error.
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

/// Tag error for SHA-256 verification failures. Attached via `anyhow::bail!`
/// so the outer classifier can [`downcast_ref`](anyhow::Error::downcast_ref)
/// to recover the exact `expected`/`got` digests - substring-parsing a
/// human message would lose them.
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

/// True if `err` represents ENOSPC. Covers both the typed `StorageFull`
/// variant (platform-independent, stable since Rust 1.83) and - on Unix -
/// the raw-errno fallback for older syscalls that surface `28` via
/// `raw_os_error()` without setting the typed kind.
///
/// US-034: the `libc::ENOSPC` arm is gated `#[cfg(unix)]` so Windows builds
/// can rely on `ErrorKind::StorageFull` alone.
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
        // EP-002 U-002: invoke_tool and the watchdog both surface
        // `UpdateError::Timeout` through `anyhow::Error::new(..)`; classify must
        // recover it (so record_update_failure tags it, not the `Other`
        // catch-all) and it must carry a non-empty user-facing message.
        let err = anyhow::Error::new(UpdateError::Timeout);
        assert_eq!(UpdateError::classify(&err), UpdateError::Timeout);
        assert!(!UpdateError::Timeout.user_message().is_empty());
    }

    #[test]
    fn classify_recovers_tag_through_context_wrapping() {
        // If a caller wraps `bail!(UpdateError::…)` with `.context(…)`, the
        // root error becomes the context string. The classifier must still
        // recover the tag by walking the chain instead of stopping at the
        // outermost layer.
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
        // On some platforms the typed kind isn't set but errno is 28.
        let err = anyhow::Error::new(std::io::Error::from_raw_os_error(libc::ENOSPC))
            .context("create cache dir");
        assert!(matches!(
            UpdateError::classify(&err),
            UpdateError::DiskFull { .. }
        ));
    }

    #[test]
    fn classify_disk_full_via_substring_fallback() {
        // When the io::Error is already stringified (e.g., came out of a
        // subprocess stderr), we fall back to text matching.
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
        // No downcast available (e.g., message came from an external tool).
        let err = anyhow::anyhow!("zsync2: checksum verification failed");
        assert!(matches!(
            UpdateError::classify(&err),
            UpdateError::IntegrityMismatch { .. }
        ));
    }

    #[test]
    fn classify_ureq_timeout_variant_as_network() {
        // US-001 AC4: a direct `ureq::Error::Timeout(_)` in the cause chain
        // is classified as `Network`, not `Other`. The context is
        // intentionally neutral - it contains no pre-existing network
        // keyword - so this test actually exercises the typed-downcast arm
        // and would fail if that arm were removed.
        let err = anyhow::Error::new(ureq::Error::Timeout(ureq::Timeout::Global))
            .context("update checker main loop");
        assert!(matches!(
            UpdateError::classify(&err),
            UpdateError::Network(_)
        ));
    }

    #[test]
    fn classify_integrity_keyword_shadowed_by_timeout_substring() {
        // Regression: a crafted error message that mixes an integrity
        // keyword with a timeout keyword must classify as IntegrityMismatch,
        // not Network. Before the substring reordering, "checksum timed out"
        // hit the Network arm first and suppressed the corruption toast.
        let err = anyhow::anyhow!("zsync2: checksum timed out waiting for block");
        assert!(matches!(
            UpdateError::classify(&err),
            UpdateError::IntegrityMismatch { .. }
        ));
    }

    #[test]
    fn classify_ureq_timeout_via_substring_fallback() {
        // US-001: when the timeout surfaces mid-body (std::io::copy wraps
        // it as io::Error, so the typed downcast above misses it), the
        // "timed out" / "timeout" substring fallback must still route it
        // to `Network` instead of `Other`.
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
        // US-005 AC8: a 404 on the pinned appimageupdatetool asset must
        // surface as ReleaseAssetMissing (not silently reclassified as
        // Network or Other).
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
        // US-009 AC8: InstallDeclined survives a downcast through the
        // cause chain so its user-visible message isn't flattened into
        // the generic Other bucket.
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
        // US-010 AC7: InstallFailed carries its verbose log path through
        // the cause chain so the toast can name it verbatim for bug
        // reports - flattening to Other would lose the path.
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
        // US-010 AC9: EnvironmentBroken survives the cause chain so the
        // "msiexec not found" toast is specific rather than generic.
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

    // ─── US-003 (pkexec): verify classify() walks the anyhow chain that
    // `linux/system_package::run_update` actually produces. The runner
    // returns `Err(anyhow::Error::new(UpdateError::X).context("pkexec
    // exited with code N"))` - the outer layer is the plain context
    // string, NOT an `UpdateError`. Without chain-walking, the
    // classifier would collapse every pkexec failure into `Other` and
    // the toast would render the raw context string instead of the
    // variant-specific copy.

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
        // The pkexec/dnf/apt branch passes `PathBuf::new()` because
        // CLI package managers don't emit a standalone log file.
        let tagged = UpdateError::InstallFailed {
            log_path: PathBuf::new(),
        };
        let err = anyhow::Error::new(tagged.clone()).context("pkexec exited with code 1");
        assert_eq!(UpdateError::classify(&err), tagged);
    }

    #[test]
    fn user_message_install_failed_handles_empty_path() {
        // US-002-pkexec: dnf/apt return no standalone log file; the
        // runner passes `PathBuf::new()`. The renderer must not emit
        // the raw "`{}`" (empty-backticks) form it uses for msiexec.
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
        // US-001 signal-kill path: `classify_exit` produces
        // `UpdateError::Other(format!("package manager killed by signal {sig}"))`
        // and `run_update` wraps it with `.context("pkexec killed by signal {sig}")`.
        // The chain walk must recover the inner `Other` variant so the
        // toast shows the signal message rather than the context wrapper.
        let tagged = UpdateError::Other("package manager killed by signal 9".into());
        let err = anyhow::Error::new(tagged.clone()).context("pkexec killed by signal 9");
        assert_eq!(UpdateError::classify(&err), tagged);
    }
}
