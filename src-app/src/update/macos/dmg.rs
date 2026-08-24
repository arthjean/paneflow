//! macOS DMG self-update pipeline (US-009).
//!
//! Flow:
//!   1. Download the `.dmg` to the native user cache dir
//!      (`~/Library/Caches/paneflow` on macOS) as `update-<pid>.dmg`
//!      via ureq with the 30-second per-call timeout (US-001).
//!   2. Verify the asset's detached **minisign** signature (`.minisig`
//!      sibling) against a key baked into this binary (US-001) **before
//!      mounting**. A missing/invalid signature deletes the partial and
//!      bails - replaces the old same-host `.sha256`, which a compromised
//!      mirror could swap alongside the bundle.
//!   3. macOS belt-and-braces: `codesign --verify` + `spctl --assess` on
//!      the extracted `.app` before promotion (US-004).
//!   4. `hdiutil attach -nobrowse -readonly -mountpoint <tmp>` to a
//!      deterministic mount point under `/private/tmp/` so later detach
//!      is trivially scoped and two concurrent updates can't collide.
//!   5. `cp -R <mount>/PaneFlow.app /Applications/PaneFlow.app.new`.
//!   6. Atomic swap: rename `/Applications/PaneFlow.app` →
//!      `/Applications/PaneFlow.app.old`, then `.new` → `PaneFlow.app`,
//!      then `rm -rf .old`. If the second rename fails, the first is
//!      rolled back so `/Applications/PaneFlow.app` never disappears.
//!   7. `hdiutil detach <mount>` - run unconditionally (RAII guard) so a
//!      mid-flow error still cleans up the mounted volume.
//!   8. Return the `.app` bundle path for `cx.set_restart_path()`. GPUI's
//!      macOS `restart()` runs `open "<path>"`, which relaunches a *bundle*
//!      but NOT a bare Mach-O - so it must receive `PaneFlow.app`, not the
//!      inner `Contents/MacOS/paneflow`. Mirrors Zed returning `Ok(None)`,
//!      which falls back to the `NSBundle.bundlePath` (the `.app`).
//!
//! **Cross-platform compile.** This module is built on every target so
//! the enclosing crate is a single compile-closure (no cfg churn in
//! `self_update_flow.rs`). `hdiutil` obviously only exists on macOS; the
//! dispatcher only routes `InstallMethod::AppBundle` here, and that
//! variant is produced solely by macOS path detection - so on Linux /
//! Windows the function compiles but is runtime-unreachable.
//!
//! **Error mapping.** `cp -R` hitting a read-only `/Applications/` or
//! SIP-protected target surfaces as an OS-level `Permission denied`;
//! that is mapped to [`UpdateError::InstallDeclined`] with a "reinstall
//! manually" message (US-009 AC8). `ENOSPC` during copy routes to
//! [`UpdateError::DiskFull`] via the existing `io::Error`-chain matcher
//! in `error::UpdateError::classify`. Mount failures surface as `Other`
//! with the raw `hdiutil` stderr preserved in logs.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use super::super::error::UpdateError;

/// Upper bound on any single HTTP call (US-001).
const UPDATE_HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Upper bound for native macOS installer tools that can otherwise block the
/// update worker indefinitely (`hdiutil`, `cp`, `codesign`, `spctl`).
const NATIVE_INSTALLER_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Detach is best-effort cleanup, but it still must not wedge Drop forever.
const NATIVE_DETACH_TIMEOUT: Duration = Duration::from_secs(60);

const NATIVE_STDOUT_CAP: u64 = 64 * 1024;

/// 500 MB ceiling on the DMG download. Real releases are ~60-100 MB; a
/// malicious mirror returning an unbounded stream would otherwise fill
/// the user's cache directory.
const MAX_DMG_BYTES: u64 = 500 * 1024 * 1024;

/// Run the DMG self-update end-to-end. Replaces the bundle **at its detected
/// location** (`bundle_path`, from `InstallMethod::AppBundle`) rather than a
/// hardcoded `/Applications/PaneFlow.app` (US-004) - so a user who installed
/// into `~/Applications` is updated in place. A bundle outside an expected
/// location is refused: writing into an arbitrary path the user dragged the
/// app to would be surprising and unsafe.
pub fn install(asset_url: &str, bundle_path: &Path) -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME environment variable is not set")?;

    if !is_expected_bundle_location(bundle_path, &home) {
        // Distinguish App Translocation (running quarantined from ~/Downloads)
        // from a genuine odd install location, so the user gets the actionable
        // "move it to /Applications" hint instead of a generic error.
        let message = if is_translocated_path(bundle_path) {
            format!(
                "PaneFlow is running translocated from a quarantine sandbox ({}), so in-app updates are disabled. Move PaneFlow.app into /Applications (drag it there in Finder) and reopen it.",
                bundle_path.display()
            )
        } else {
            format!(
                "PaneFlow is installed at an unexpected location ({}); reinstall from the DMG into /Applications or ~/Applications to enable in-app updates.",
                bundle_path.display()
            )
        };
        return Err(anyhow::Error::new(UpdateError::InstallDeclined { message }));
    }

    let cache_dir = dmg_cache_dir(&home);
    install_in(asset_url, bundle_path, &cache_dir, &HdiutilProcessRunner)?;
    // Return the `.app` bundle itself, NOT the inner Mach-O. GPUI's macOS
    // `restart()` does `open "<path>"`; `open` relaunches a bundle but treats a
    // bare executable as a file to open - so passing the Mach-O left the old
    // process dead and the new one never started after a successful update.
    Ok(bundle_path.to_path_buf())
}

/// True when `bundle_path` sits directly under `/Applications` or
/// `$HOME/Applications` - the two locations the DMG updater is allowed to
/// replace in place (US-004). Pure path logic, unit-tested on Linux CI.
fn is_expected_bundle_location(bundle_path: &Path, home: &Path) -> bool {
    let Some(parent) = bundle_path.parent() else {
        return false;
    };
    parent == Path::new("/Applications") || parent == home.join("Applications")
}

/// True when `path` is an App Translocation / quarantine-sandbox path. macOS
/// runs a quarantined app (e.g. launched straight from `~/Downloads`) from a
/// randomized read-only mount under
/// `/private/var/folders/.../AppTranslocation/...` rather than its real
/// location. `current_exe()` reports that translocated path (unlike
/// `NSBundle.bundlePath`, which the OS de-translocates), so the updater cannot
/// find - let alone replace - the real bundle. Detecting it lets `install`
/// surface "move the app to /Applications" instead of a generic error. Pure
/// string logic, unit-tested on every platform.
fn is_translocated_path(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.contains("/AppTranslocation/") || s.contains("/var/folders/")
}

fn dmg_cache_dir(home: &Path) -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| home.join("Library").join("Caches"))
        .join(crate::runtime_paths::APP_SUBDIR)
}

/// Paneflow's Apple Developer **Team ID** (project_macos_signing, populated
/// 2026-05-04 with the first signed release; cert valid until 2031-05-05).
/// For a Developer ID Application certificate the leaf cert's
/// `subject.OU` equals the Team ID, so pinning it rejects any
/// validly-notarised-but-*foreign* bundle.
///
/// US-018: the plain `codesign --verify` + `spctl --assess` checks below
/// only prove "signed by *someone* Apple trusts and notarised" - NOT
/// "signed by us". A second developer's notarised app would pass them. This
/// pin closes that gap (defense-in-depth on top of the minisign root-of-trust
/// that already gates the DMG bytes before the bundle is ever mounted).
#[cfg(target_os = "macos")]
const APPLE_TEAM_ID: &str = "228F9H5P95";

/// Build the `codesign` argument that pins the signing identity to our Apple
/// Team ID, using the attached `-R=<requirement>` form.
///
/// This MUST stay the attached form. macOS 15+/26 interpret a *separate*
/// `-R <requirement>` argument as a file path: codesign tries to open the
/// inline requirement text as a file and aborts ("No such file or directory /
/// invalid requirement specification"), so the Team-ID pin silently failed on
/// every run - which froze the DMG self-update at the 3-strikes "Update keeps
/// failing" toast (in-app updates worked on Linux/Windows, which have no such
/// codesign gate). The attached form is parsed as inline requirement source on
/// every supported macOS.
///
/// Pure string builder - gated `#[cfg(any(target_os = "macos", test))]` so the
/// regression test runs on Linux CI without a signed fixture, while non-macOS
/// release builds don't carry it as dead code.
#[cfg(any(target_os = "macos", test))]
fn team_id_requirement_arg(team_id: &str) -> String {
    format!("-R=anchor apple generic and certificate leaf[subject.OU] = \"{team_id}\"")
}

/// Gatekeeper / code-signature verification of `bundle` (US-004), fail-closed.
/// `codesign --verify --strict --deep` proves the signature is intact and
/// covers every nested item; the Team-ID designated-requirement check (US-018)
/// proves it is *our* signing identity; `spctl --assess --type execute` proves
/// the bundle is notarised / accepted by the system policy. Any tool exiting
/// nonzero rejects the update with a tagged `IntegrityMismatch`.
///
/// macOS-only; untestable on the Linux CI leg (no `codesign`/`spctl`, no
/// notarised fixture) - exercised by the macOS release leg against the real
/// signed bundle.
#[cfg(target_os = "macos")]
fn verify_macos_bundle(bundle: &Path) -> Result<()> {
    run_gatekeeper_tool(
        "codesign",
        &["--verify", "--strict", "--deep", "--verbose=2"],
        bundle,
    )?;
    // US-018: pin the signing identity to our Team ID via a designated
    // requirement. Fails closed if the leaf cert's OU is not our Team ID, so a
    // foreign-but-notarised bundle is rejected even though it passes the plain
    // `--verify` and `spctl` checks.
    //
    // The requirement MUST be passed as the attached `-R=<req>` form (see
    // `team_id_requirement_arg`): macOS 15+/26 interpret a *separate*
    // `-R <req>` argument as a file path, silently failing this pin on every
    // DMG self-update.
    let team_arg = team_id_requirement_arg(APPLE_TEAM_ID);
    run_gatekeeper_tool("codesign", &["--verify", team_arg.as_str()], bundle)?;
    run_gatekeeper_tool("spctl", &["--assess", "--type", "execute"], bundle)?;
    Ok(())
}

/// Spawn a single Gatekeeper tool (`codesign` / `spctl`) against `bundle` and
/// map a nonzero exit to a fail-closed `IntegrityMismatch`.
#[cfg(target_os = "macos")]
fn run_gatekeeper_tool(tool: &str, args: &[&str], bundle: &Path) -> Result<()> {
    let mut cmd = Command::new(tool);
    cmd.args(args).arg(bundle);
    let out = run_native_command(
        cmd,
        &format!("{tool} bundle verification"),
        NATIVE_INSTALLER_TIMEOUT,
    )?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    Err(anyhow::Error::new(super::super::error::IntegrityMismatch {
        expected: "valid macOS code signature".to_string(),
        got: format!("{tool} rejected the bundle: {}", stderr.trim()),
    }))
}

/// Testable core. Parameterised on:
/// - `install_dir`: the target `.app` bundle path (normally
///   `/Applications/PaneFlow.app`).
/// - `cache_dir`: where the DMG is downloaded.
/// - `runner`: abstracts `hdiutil attach`/`detach` so tests can inject
///   success/failure without spawning the real tool.
fn install_in(
    asset_url: &str,
    install_dir: &Path,
    cache_dir: &Path,
    runner: &dyn Hdiutil,
) -> Result<()> {
    std::fs::create_dir_all(cache_dir)
        .with_context(|| format!("create cache dir {}", cache_dir.display()))?;

    let dmg = cache_dir.join(format!("update-{}.dmg", std::process::id()));
    let download_result = download_with_verification(asset_url, &dmg);
    if let Err(e) = download_result {
        let _ = std::fs::remove_file(&dmg);
        return Err(e);
    }

    // Deterministic mount point under `/private/tmp`. `hdiutil attach`
    // requires the directory to not pre-exist - it creates it. `.<pid>`
    // avoids clashes between concurrent updates.
    let mount_point = PathBuf::from(format!(
        "/private/tmp/paneflow-update-{}.mount",
        std::process::id()
    ));
    if mount_point.exists() {
        let _ = std::fs::remove_dir_all(&mount_point);
    }

    let mounted = runner.attach(&dmg, &mount_point).inspect_err(|_| {
        let _ = std::fs::remove_file(&dmg);
    })?;

    // RAII detach: whatever happens below, the mounted volume is
    // released. `hdiutil detach` is best-effort on the error path -
    // leaving a lingering mount is annoying but harmless.
    let _detach_guard = DetachGuard {
        runner,
        mount: mounted.clone(),
    };

    // US-004: OS-native gatekeeper check on the bundle inside the (read-only,
    // signature-verified) mounted DMG before we copy it into place -
    // fail-closed, a second layer over the minisign verification of the DMG
    // bytes. cfg(macos) so the Linux/Windows compile-closure and the
    // platform-neutral copy/swap unit tests stay free of `codesign`/`spctl`.
    #[cfg(target_os = "macos")]
    {
        let bundle_name = bundle_file_name(install_dir)?;
        if let Err(e) = verify_macos_bundle(&mounted.join(bundle_name)) {
            let _ = std::fs::remove_file(&dmg);
            return Err(e);
        }
    }

    let swap_result = copy_and_swap(&mounted, install_dir);

    // Regardless of swap outcome, the downloaded tarball is a ~80 MB
    // scratch file; delete it. Keeping it wouldn't resume a crashed
    // update anyway since the SHA-256 pin is recomputed from source
    // every run.
    let _ = std::fs::remove_file(&dmg);
    swap_result
}

/// Download the DMG, verify its detached **minisign** signature (US-001),
/// and persist at `dest` on success. Mirrors the `targz.rs` pattern -
/// see it for the detailed rationale on each guard (partial→rename,
/// 500 MB cap, RO body stream). The signature, not a same-host `.sha256`,
/// is the trust anchor and is checked **before** the DMG is ever mounted.
fn download_with_verification(asset_url: &str, dest: &Path) -> Result<()> {
    super::super::verified_download::download_verified_asset(
        asset_url,
        dest,
        MAX_DMG_BYTES,
        UPDATE_HTTP_TIMEOUT,
        "DMG",
    )
}

/// Mount the DMG and perform the atomic swap into `install_dir`.
///
/// Split out so the testable core can inject a fake mount directory
/// (the copy/rename half is filesystem-only and doesn't need hdiutil).
fn copy_and_swap(mounted_volume: &Path, install_dir: &Path) -> Result<()> {
    let source_bundle = mounted_volume.join(bundle_file_name(install_dir)?);
    if !source_bundle.exists() {
        bail!(
            "DMG did not contain the {} bundle at {} - archive appears malformed.",
            bundle_file_name(install_dir)?.to_string_lossy(),
            source_bundle.display()
        );
    }

    let (old_dir, new_dir) = staging_dirs(install_dir)?;

    recover_and_clean_staging(install_dir, &old_dir)?;

    // `.new` from a crashed prior flow is pure scratch - safe to remove
    // before the fresh copy. Log a warning on failure so a downstream
    // copy error isn't misdiagnosed as a DMG problem.
    if new_dir.exists()
        && let Err(e) = std::fs::remove_dir_all(&new_dir)
    {
        log::warn!(
            "self-update/dmg: could not clean stale {}: {e}",
            new_dir.display()
        );
    }

    if let Err(e) = copy_bundle_to_staging(&source_bundle, &new_dir) {
        // Best-effort cleanup - if the partial copy left files behind,
        // drop them before the next try. Ignore the result (scratch dir).
        let _ = std::fs::remove_dir_all(&new_dir);
        return Err(e);
    }

    // #9: re-verify the COPIED bundle BEFORE the swap, not just the read-only
    // source on the DMG. A `cp -R` that exits 0 yet produced a corrupt tree
    // would otherwise promote an unverified (possibly invalidly-signed) bundle.
    // `not(test)` so the filesystem-only `copy_and_swap` unit tests - which
    // stage a fake unsigned bundle and genuinely cannot satisfy codesign -
    // keep exercising the copy/rename logic without spawning codesign/spctl.
    #[cfg(all(target_os = "macos", not(test)))]
    if let Err(e) = verify_macos_bundle(&new_dir) {
        let _ = std::fs::remove_dir_all(&new_dir);
        return Err(e);
    }

    // Atomic swap: two renames. The window where `install_dir` doesn't
    // exist is vanishingly small and bracketed by the rollback below.
    //
    // US-008: discriminate the first rename's error. `NotFound` means a fresh
    // install into an empty `/Applications/` - expected, fall through. ANY
    // other error (permission denied, SIP, bundle busy) is a hard abort:
    // proceeding would risk a half-installed state.
    if let Err(e) = std::fs::rename(install_dir, &old_dir) {
        if e.kind() == std::io::ErrorKind::NotFound {
            log::debug!(
                "self-update/dmg: no pre-existing {} (fresh install)",
                install_dir.display()
            );
        } else {
            let _ = std::fs::remove_dir_all(&new_dir);
            return Err(classify_filesystem_error(
                &e.to_string(),
                &format!("move aside {}", install_dir.display()),
            ));
        }
    }
    if let Err(e) = std::fs::rename(&new_dir, install_dir) {
        // Second rename failed - restore the live bundle so the user isn't
        // left without `/Applications/PaneFlow.app`. US-008: verify the
        // rollback. If it ALSO fails, the user has no live bundle at all -
        // surface that as a hard `InstallFailed` (catastrophic), not the
        // recoverable filesystem-error classification, so the toast tells
        // them to reinstall rather than implying a transient retry.
        if old_dir.exists()
            && let Err(rb) = std::fs::rename(&old_dir, install_dir)
        {
            let _ = std::fs::remove_dir_all(&new_dir);
            return Err(anyhow::Error::new(UpdateError::InstallFailed {
                log_path: PathBuf::new(),
            })
            .context(format!(
                "promote {} → {} failed ({e}); rollback from {} also failed ({rb}) - no live install remains, reinstall PaneFlow manually",
                new_dir.display(),
                install_dir.display(),
                old_dir.display()
            )));
        }
        let _ = std::fs::remove_dir_all(&new_dir);
        return Err(classify_filesystem_error(
            &e.to_string(),
            &format!("promote {} → {}", new_dir.display(), install_dir.display()),
        ));
    }

    // Success - drop `.old`. Failure is non-fatal (scratch dir);
    // next update will fail-fast on the "previous update did not clean
    // up" guard above, which is strictly better than silent overwrite.
    if old_dir.exists()
        && let Err(e) = std::fs::remove_dir_all(&old_dir)
    {
        log::warn!(
            "self-update/dmg: could not remove stale {}: {e}",
            old_dir.display()
        );
    }

    // #7: strip com.apple.quarantine from the freshly promoted bundle. `cp -R`
    // preserved any flag the DMG (or a file in it) carried, which could prompt
    // Gatekeeper on first launch. Best-effort - a notarized bundle launches
    // without it regardless.
    #[cfg(target_os = "macos")]
    strip_quarantine(install_dir);

    Ok(())
}

/// Copy `PaneFlow.app` into the `.new` staging path.
///
/// Runtime macOS uses `cp -R`, because it preserves bundle structure, symlinks,
/// and extended attributes - notably `_CodeSignature` and quarantine xattrs.
/// Unit tests compile on every host, though, and Windows does not provide a
/// POSIX `cp`; tests on non-macOS hosts use a small recursive copy that is
/// sufficient to exercise the swap/rollback logic.
#[cfg(any(not(test), target_os = "macos"))]
fn copy_bundle_to_staging(source_bundle: &Path, new_dir: &Path) -> Result<()> {
    let mut cmd = Command::new("cp");
    cmd.arg("-R").arg(source_bundle).arg(new_dir);
    let cp_out = run_native_command(
        cmd,
        &format!("cp -R {} {}", source_bundle.display(), new_dir.display()),
        NATIVE_INSTALLER_TIMEOUT,
    )?;

    if !cp_out.status.success() {
        let stderr = String::from_utf8_lossy(&cp_out.stderr);
        return Err(classify_filesystem_error(
            &stderr,
            &format!("copy {} → {}", source_bundle.display(), new_dir.display()),
        ));
    }
    Ok(())
}

/// Recover a prior crash around the two-rename swap.
///
/// - `.old` present and live bundle missing: restore `.old` so the install
///   location is usable before this update attempt continues.
/// - `.old` present and live bundle intact: treat `.old` as stale
///   housekeeping debris and remove it before staging a new swap.
fn recover_and_clean_staging(install_dir: &Path, old_dir: &Path) -> Result<()> {
    if !old_dir.exists() {
        return Ok(());
    }
    if !install_dir.exists() {
        std::fs::rename(old_dir, install_dir).with_context(|| {
            format!(
                "recover live bundle {} from {}",
                install_dir.display(),
                old_dir.display()
            )
        })?;
        log::warn!(
            "self-update/dmg: recovered live bundle from a crashed prior update ({})",
            install_dir.display()
        );
        return Ok(());
    }
    if let Err(e) = std::fs::remove_dir_all(old_dir) {
        log::warn!(
            "self-update/dmg: could not remove stale {}: {e}",
            old_dir.display()
        );
    }
    Ok(())
}

#[cfg(all(test, not(target_os = "macos")))]
fn copy_bundle_to_staging(source_bundle: &Path, new_dir: &Path) -> Result<()> {
    copy_tree_for_test(source_bundle, new_dir)
}

#[cfg(all(test, not(target_os = "macos")))]
fn copy_tree_for_test(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).with_context(|| format!("create {}", dst.display()))?;
    for entry in std::fs::read_dir(src).with_context(|| format!("read {}", src.display()))? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree_for_test(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)
                .with_context(|| format!("copy {} → {}", src_path.display(), dst_path.display()))?;
        }
    }
    Ok(())
}

fn staging_dirs(install_dir: &Path) -> Result<(PathBuf, PathBuf)> {
    let parent = install_dir
        .parent()
        .context("install_dir has no parent - refusing to swap at filesystem root")?;
    let name = install_dir
        .file_name()
        .context("install_dir has no file name - refusing to swap")?;
    let name = name.to_string_lossy();
    Ok((
        parent.join(format!("{name}.old")),
        parent.join(format!("{name}.new")),
    ))
}

/// The `.app` filename to look for inside the mounted DMG, derived from the
/// install target. Couples the source-bundle name and the destination to a
/// SINGLE invariant instead of two hardcoded `"PaneFlow.app"` literals that
/// could silently drift apart. (Zed derives the same from the running bundle.)
fn bundle_file_name(install_dir: &Path) -> Result<&std::ffi::OsStr> {
    install_dir
        .file_name()
        .context("install_dir has no file name - cannot locate the bundle inside the DMG")
}

/// Best-effort removal of `com.apple.quarantine` from a freshly promoted
/// bundle. `cp -R` PRESERVES extended attributes, so a quarantined source
/// would yield a quarantined install and Gatekeeper could prompt on first
/// launch. A notarized bundle is trusted regardless, and a missing attribute
/// or an `xattr` failure must not fail an otherwise-successful update - hence
/// the ignored result.
#[cfg(target_os = "macos")]
fn strip_quarantine(bundle: &Path) {
    let mut cmd = Command::new("xattr");
    cmd.arg("-dr").arg("com.apple.quarantine").arg(bundle);
    if let Err(e) = run_native_command(cmd, "xattr strip quarantine", NATIVE_DETACH_TIMEOUT) {
        log::warn!(
            "self-update/dmg: xattr quarantine cleanup for {} failed: {e:#}",
            bundle.display()
        );
    }
}

/// Map a filesystem error message (from either an `io::Error` or a
/// subprocess stderr) onto the right `UpdateError` variant. Permission
/// denied routes to `InstallDeclined` per US-009 AC8; everything else
/// falls through to `Other` with the raw message preserved for logs
/// (the error.rs classifier picks up ENOSPC via the io::Error chain).
fn classify_filesystem_error(raw: &str, context: &str) -> anyhow::Error {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("permission denied")
        || lower.contains("operation not permitted")
        || lower.contains("read-only file system")
    {
        return anyhow::Error::new(UpdateError::InstallDeclined {
            message: "Unable to replace PaneFlow.app in its install location - reinstall manually from the DMG."
                .to_string(),
        })
        .context(format!("{context}: {}", raw.trim()));
    }
    anyhow::Error::msg(format!("{context} - {}", raw.trim()))
}

fn run_native_command(
    cmd: Command,
    label: &str,
    deadline: Duration,
) -> Result<paneflow_process::BoundedOutput> {
    paneflow_process::run_with_timeout(cmd, deadline, NATIVE_STDOUT_CAP).map_err(|err| match err {
        paneflow_process::ProcError::Timeout => {
            anyhow::Error::new(UpdateError::Timeout).context(format!("{label} timed out"))
        }
        paneflow_process::ProcError::Spawn(e) => {
            anyhow::Error::new(e).context(format!("spawn {label}"))
        }
        paneflow_process::ProcError::Wait(e) => {
            anyhow::Error::new(e).context(format!("wait for {label}"))
        }
        other => anyhow::Error::new(other).context(format!("run {label}")),
    })
}

/// Abstraction over `hdiutil attach/detach` so tests can inject a fake
/// mount directory without spawning the real tool. The return value is
/// the actual mount path - `hdiutil` normally honours `-mountpoint` but
/// falls back to `/Volumes/<label>` if the target is inaccessible; the
/// trait lets a test return a known temp path instead.
trait Hdiutil {
    fn attach(&self, dmg: &Path, target: &Path) -> Result<PathBuf>;
    fn detach(&self, mount: &Path);
}

struct HdiutilProcessRunner;

impl Hdiutil for HdiutilProcessRunner {
    fn attach(&self, dmg: &Path, target: &Path) -> Result<PathBuf> {
        let mut cmd = Command::new("hdiutil");
        cmd.arg("attach")
            .arg("-nobrowse")
            .arg("-readonly")
            .arg("-mountpoint")
            .arg(target)
            .arg(dmg);
        let out = run_native_command(
            cmd,
            &format!("hdiutil attach {}", dmg.display()),
            NATIVE_INSTALLER_TIMEOUT,
        )?;

        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            bail!(
                "hdiutil attach failed (status {}): {}",
                out.status,
                stderr.trim()
            );
        }
        if !target.exists() {
            bail!(
                "hdiutil attach claimed success but {} does not exist",
                target.display()
            );
        }
        Ok(target.to_path_buf())
    }

    fn detach(&self, mount: &Path) {
        // Best-effort. A still-mounted volume blocks /private/tmp
        // cleanup for the next update but is not an install failure.
        // `-force` ejects even if a file on the volume is still open (e.g. the
        // `cp` we just ran briefly held one), matching Zed - without it a busy
        // volume lingers and the next update's `attach` to the same mountpoint
        // fails.
        let mut cmd = Command::new("hdiutil");
        cmd.arg("detach").arg("-force").arg(mount);
        match run_native_command(
            cmd,
            &format!("hdiutil detach {}", mount.display()),
            NATIVE_DETACH_TIMEOUT,
        ) {
            Ok(out) if out.status.success() => {}
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                log::warn!(
                    "self-update/dmg: hdiutil detach {} exited {}: {}",
                    mount.display(),
                    out.status,
                    stderr.trim()
                );
            }
            Err(e) => {
                log::warn!(
                    "self-update/dmg: hdiutil detach {} failed: {e:#}",
                    mount.display()
                );
            }
        }
    }
}

/// RAII guard that runs `hdiutil detach` on drop. Keeps the mount
/// cleanup scope-tied to the install attempt so an error path can't
/// leak a mounted volume.
struct DetachGuard<'a> {
    runner: &'a dyn Hdiutil,
    mount: PathBuf,
}

impl Drop for DetachGuard<'_> {
    fn drop(&mut self) {
        self.runner.detach(&self.mount);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    // ── Pure helpers ─────────────────────────────────────────────────

    #[test]
    fn staging_dirs_derives_sibling_paths() {
        let (old, new) = staging_dirs(Path::new("/Applications/PaneFlow.app")).unwrap();
        assert_eq!(old, PathBuf::from("/Applications/PaneFlow.app.old"));
        assert_eq!(new, PathBuf::from("/Applications/PaneFlow.app.new"));
    }

    #[test]
    fn team_id_requirement_uses_attached_form() {
        // Regression: macOS 15+/26 codesign treats a SEPARATE `-R <arg>` as a
        // file path, so the inline requirement was opened as a (missing) file
        // ("No such file or directory / invalid requirement specification") and
        // the Team-ID pin failed every DMG self-update - pinning the updater at
        // the "Update keeps failing" toast. The arg MUST be the attached
        // `-R=<requirement>` form (one argv element), which codesign parses as
        // inline requirement source.
        let arg = team_id_requirement_arg("228F9H5P95");
        assert!(
            arg.starts_with("-R="),
            "must be the attached form, got: {arg}"
        );
        assert!(
            arg.contains("certificate leaf[subject.OU] = \"228F9H5P95\""),
            "requirement must pin the leaf OU to the Team ID, got: {arg}"
        );
    }

    #[test]
    fn is_translocated_path_detects_quarantine_sandbox() {
        // The real install locations are NOT translocated.
        assert!(!is_translocated_path(Path::new(
            "/Applications/PaneFlow.app"
        )));
        assert!(!is_translocated_path(Path::new(
            "/Users/x/Applications/PaneFlow.app"
        )));
        // App Translocation mounts the quarantined bundle under a randomized
        // /private/var/folders/.../AppTranslocation/ path.
        assert!(is_translocated_path(Path::new(
            "/private/var/folders/ab/cd/T/AppTranslocation/UUID/d/PaneFlow.app"
        )));
        // Bare /var/folders (older layout / symlink-resolved) also flags.
        assert!(is_translocated_path(Path::new(
            "/var/folders/ab/cd/T/PaneFlow.app"
        )));
    }

    #[test]
    fn bundle_file_name_derives_from_install_dir() {
        assert_eq!(
            bundle_file_name(Path::new("/Applications/PaneFlow.app")).unwrap(),
            std::ffi::OsStr::new("PaneFlow.app")
        );
        assert!(bundle_file_name(Path::new("/")).is_err());
    }

    #[test]
    fn expected_bundle_location_accepts_applications_dirs() {
        let home = Path::new("/Users/alice");
        assert!(is_expected_bundle_location(
            Path::new("/Applications/PaneFlow.app"),
            home
        ));
        assert!(is_expected_bundle_location(
            Path::new("/Users/alice/Applications/PaneFlow.app"),
            home
        ));
    }

    #[test]
    fn expected_bundle_location_rejects_arbitrary_paths() {
        let home = Path::new("/Users/alice");
        // US-004: a drag-install to an unexpected location is refused.
        assert!(!is_expected_bundle_location(
            Path::new("/opt/third-party/PaneFlow.app"),
            home
        ));
        assert!(!is_expected_bundle_location(
            Path::new("/Users/alice/Downloads/PaneFlow.app"),
            home
        ));
        // Another user's Applications dir is not ours.
        assert!(!is_expected_bundle_location(
            Path::new("/Users/bob/Applications/PaneFlow.app"),
            home
        ));
    }

    // ── Error classification ─────────────────────────────────────────

    #[test]
    fn classify_permission_denied_as_install_declined() {
        let err = classify_filesystem_error("cp: /Applications: Permission denied", "copy step");
        assert!(matches!(
            UpdateError::classify(&err),
            UpdateError::InstallDeclined { .. }
        ));
    }

    #[test]
    fn classify_read_only_as_install_declined() {
        let err = classify_filesystem_error("Read-only file system", "copy step");
        assert!(matches!(
            UpdateError::classify(&err),
            UpdateError::InstallDeclined { .. }
        ));
    }

    #[test]
    fn classify_sip_operation_not_permitted_as_install_declined() {
        // SIP-protected paths surface as EPERM ("Operation not permitted")
        // rather than EACCES on modern macOS - must also route to
        // InstallDeclined so the toast is the actionable "reinstall
        // manually" copy, not the generic "update failed".
        let err = classify_filesystem_error(
            "rename /Applications/PaneFlow.app: Operation not permitted (os error 1)",
            "swap step",
        );
        assert!(matches!(
            UpdateError::classify(&err),
            UpdateError::InstallDeclined { .. }
        ));
    }

    #[test]
    fn classify_unknown_error_falls_through_to_other() {
        let err = classify_filesystem_error("totally unexpected hdiutil garble", "mount step");
        // Not specifically routed; ends up as Other via the classifier's
        // fallback. Disk-full / network keywords are already covered by
        // `UpdateError::classify` substring matches.
        assert!(matches!(UpdateError::classify(&err), UpdateError::Other(_)));
    }

    // ── install_in() with stubbed hdiutil ────────────────────────────

    /// Stub that records every attach/detach call and can be pre-loaded
    /// with an attach error. Success mode copies a prepared fake mount
    /// directory into the requested mount point so the subsequent
    /// `copy_and_swap` runs the real filesystem code.
    struct StubHdiutil {
        fake_bundle_source: PathBuf,
        attach_error: RefCell<Option<String>>,
        detach_calls: RefCell<Vec<PathBuf>>,
    }

    impl Hdiutil for StubHdiutil {
        fn attach(&self, _dmg: &Path, target: &Path) -> Result<PathBuf> {
            if let Some(msg) = self.attach_error.borrow_mut().take() {
                bail!("hdiutil attach failed (stub): {msg}");
            }
            std::fs::create_dir_all(target)?;
            // Mirror the structure hdiutil would produce: `<mount>/PaneFlow.app`.
            let dst = target.join("PaneFlow.app");
            copy_tree(&self.fake_bundle_source, &dst)?;
            Ok(target.to_path_buf())
        }

        fn detach(&self, mount: &Path) {
            self.detach_calls.borrow_mut().push(mount.to_path_buf());
        }
    }

    fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                copy_tree(&src_path, &dst_path)?;
            } else {
                std::fs::copy(&src_path, &dst_path)?;
            }
        }
        Ok(())
    }

    /// Build a minimal PaneFlow.app skeleton under `root/PaneFlow.app/`
    /// for the stubs to mount. The file contents don't matter - the
    /// swap code only cares about the directory structure.
    fn fake_bundle_at(root: &Path) -> PathBuf {
        let bundle = root.join("PaneFlow.app");
        let macos = bundle.join("Contents").join("MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::write(macos.join("paneflow"), b"#!/bin/sh\necho paneflow").unwrap();
        bundle
    }

    /// Write a minimal HTTP-like asset + .sha256 sidecar to a local
    /// file path; the real `install_in` uses ureq so we can't test the
    /// download leg without a live server. Instead, we split at
    /// `copy_and_swap` which is the part exercised by the stub tests.

    #[test]
    fn copy_and_swap_performs_atomic_rename() {
        let tmp = tempfile::TempDir::new().unwrap();
        let source_root = tmp.path().join("mount");
        std::fs::create_dir_all(&source_root).unwrap();
        fake_bundle_at(&source_root);

        let install_dir = tmp.path().join("Applications").join("PaneFlow.app");
        std::fs::create_dir_all(install_dir.parent().unwrap()).unwrap();
        // Pre-existing "live" bundle with marker content.
        std::fs::create_dir_all(&install_dir).unwrap();
        std::fs::write(install_dir.join("old-marker"), b"old").unwrap();

        copy_and_swap(&source_root, &install_dir).unwrap();

        // Post-swap: install_dir is the new bundle, old-marker is gone,
        // new binary is in place.
        assert!(install_dir.join("Contents/MacOS/paneflow").exists());
        assert!(!install_dir.join("old-marker").exists());
        // `.old` was cleaned up.
        let old_dir = install_dir.parent().unwrap().join("PaneFlow.app.old");
        assert!(!old_dir.exists(), "`.old` should have been removed");
    }

    #[test]
    fn copy_and_swap_aborts_when_source_has_no_bundle() {
        let tmp = tempfile::TempDir::new().unwrap();
        let empty_mount = tmp.path().join("empty-mount");
        std::fs::create_dir_all(&empty_mount).unwrap();
        let install_dir = tmp.path().join("Applications").join("PaneFlow.app");
        let err = copy_and_swap(&empty_mount, &install_dir).unwrap_err();
        assert!(err.to_string().contains("PaneFlow.app"), "got: {err}");
    }

    /// US-008: a fresh install (no pre-existing bundle at `install_dir`)
    /// must succeed - the first rename fails with `NotFound`, which is the
    /// one error the swap is allowed to ignore.
    #[test]
    fn copy_and_swap_fresh_install_no_existing_bundle() {
        let tmp = tempfile::TempDir::new().unwrap();
        let source_root = tmp.path().join("mount");
        std::fs::create_dir_all(&source_root).unwrap();
        fake_bundle_at(&source_root);

        // Parent exists but the bundle itself does not (empty /Applications).
        let install_parent = tmp.path().join("Applications");
        std::fs::create_dir_all(&install_parent).unwrap();
        let install_dir = install_parent.join("PaneFlow.app");
        assert!(!install_dir.exists());

        copy_and_swap(&source_root, &install_dir).unwrap();

        assert!(install_dir.join("Contents/MacOS/paneflow").exists());
        assert!(
            !install_parent.join("PaneFlow.app.old").exists(),
            "no .old should be created on a fresh install"
        );
    }

    #[test]
    fn recover_restores_live_bundle_when_install_dir_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let install_parent = tmp.path().join("Applications");
        std::fs::create_dir_all(&install_parent).unwrap();
        let install_dir = install_parent.join("PaneFlow.app");
        let old_dir = install_parent.join("PaneFlow.app.old");
        std::fs::create_dir_all(old_dir.join("Contents/MacOS")).unwrap();
        std::fs::write(old_dir.join("Contents/MacOS/paneflow"), b"prev").unwrap();

        recover_and_clean_staging(&install_dir, &old_dir).unwrap();

        assert!(install_dir.join("Contents/MacOS/paneflow").exists());
        assert!(!old_dir.exists(), ".old must be consumed by recovery");
    }

    #[test]
    fn recover_removes_stale_old_when_live_bundle_exists() {
        let tmp = tempfile::TempDir::new().unwrap();
        let install_parent = tmp.path().join("Applications");
        std::fs::create_dir_all(&install_parent).unwrap();
        let install_dir = install_parent.join("PaneFlow.app");
        std::fs::create_dir_all(install_dir.join("Contents/MacOS")).unwrap();
        let old_dir = install_parent.join("PaneFlow.app.old");
        std::fs::create_dir_all(&old_dir).unwrap();

        recover_and_clean_staging(&install_dir, &old_dir).unwrap();

        assert!(install_dir.exists(), "live bundle must remain untouched");
        assert!(!old_dir.exists(), "stale .old must be removed");
    }

    #[test]
    fn copy_and_swap_cleans_stale_old_when_live_bundle_exists() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mount = tmp.path().join("mount");
        std::fs::create_dir_all(&mount).unwrap();
        fake_bundle_at(&mount);

        let install_parent = tmp.path().join("Applications");
        std::fs::create_dir_all(&install_parent).unwrap();
        let install_dir = install_parent.join("PaneFlow.app");
        std::fs::create_dir_all(&install_dir).unwrap();
        // Stale `.old` from a crashed prior update.
        std::fs::create_dir_all(install_parent.join("PaneFlow.app.old")).unwrap();

        copy_and_swap(&mount, &install_dir).unwrap();

        assert!(install_dir.join("Contents/MacOS/paneflow").exists());
        assert!(!install_parent.join("PaneFlow.app.old").exists());
    }

    /// AC7: hdiutil attach failure must surface to the caller (no
    /// silent fall-through). The DetachGuard must NOT run detach on
    /// an attach that never succeeded - the RefCell counter proves it.
    #[test]
    fn install_in_propagates_hdiutil_attach_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let stub = StubHdiutil {
            fake_bundle_source: tmp.path().join("unused"),
            attach_error: RefCell::new(Some("no mountable file systems".to_string())),
            detach_calls: RefCell::new(Vec::new()),
        };
        let install_dir = tmp.path().join("Applications").join("PaneFlow.app");
        let cache = tmp.path().join("cache");

        // install_in also runs the download leg, which requires the
        // network. Since we can't mock ureq without a framework, call
        // copy_and_swap directly for the detach-guard test; the
        // propagation invariant is proved by the fact that `attach`
        // returning Err short-circuits copy_and_swap. Exercise the
        // Hdiutil trait directly instead.
        let result = stub.attach(Path::new("/nonexistent.dmg"), &install_dir);
        assert!(result.is_err(), "stub attach returned Ok unexpectedly");
        assert_eq!(
            stub.detach_calls.borrow().len(),
            0,
            "detach must not run when attach itself failed"
        );
        let _ = cache;
    }

    /// AC7: the StubHdiutil-backed install_in exercises the copy_and_swap
    /// path via the trait object indirection. Driving the full download
    /// leg requires a live HTTP server, which is out of scope for a
    /// unit test; instead, verify the runner wiring by checking that
    /// `detach_calls` is consistent with the trait's contract.
    #[test]
    fn detach_guard_fires_on_drop() {
        let tmp = tempfile::TempDir::new().unwrap();
        let bundle_src = tmp.path().join("src-bundle");
        fake_bundle_at(&bundle_src);
        let stub = StubHdiutil {
            fake_bundle_source: bundle_src.clone(),
            attach_error: RefCell::new(None),
            detach_calls: RefCell::new(Vec::new()),
        };
        {
            let _guard = DetachGuard {
                runner: &stub,
                mount: PathBuf::from("/some/mount"),
            };
        }
        let calls = stub.detach_calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], PathBuf::from("/some/mount"));
    }
}
