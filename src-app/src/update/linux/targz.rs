//! tar.gz self-update via atomic directory swap (US-011).
//!
//! Flow:
//!   1. Download the tar.gz asset to `$HOME/.cache/paneflow/update-<pid>.tar.gz`.
//!   2. Verify the asset's detached **minisign** signature (`.minisig`
//!      sibling) against a public key baked into this binary (US-001).
//!      A missing or invalid signature deletes the download and aborts -
//!      the same-host `.sha256` it replaced gave no real trust (a mirror
//!      that swaps the tarball swaps the checksum too).
//!   3. Extract into `<parent>/paneflow.app.new/` (a sibling of the live
//!      install dir; same filesystem so the swap rename is atomic).
//!   4. Atomic swap via two `rename(2)` calls:
//!      `app_dir` → `app_dir.old`, then `app_dir.new` → `app_dir`, then
//!      `rm -rf app_dir.old`. The window between the two renames is
//!      brief. A pre-existing `app_dir.old` (crashed prior update) is a
//!      hard abort - we do NOT blindly overwrite.
//!   5. Return `<app_dir>/bin/paneflow` - the caller passes this to
//!      `cx.set_restart_path()` so GPUI's launcher execs the new binary.
//!
//! **Invariant - writes stay inside $HOME.** The download lives in
//! `$HOME/.cache/paneflow/`; extraction and swap happen inside
//! `<app_dir>` and its parent. No code path writes to `/usr`, `/opt`,
//! `/bin`, or any absolute path outside `$HOME`. This is essential for
//! immutable distros (Silverblue, SteamOS, MicroOS). Enforced by the
//! test `never_writes_outside_its_roots` which runs the whole flow under
//! a `tempdir`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};

/// 500 MB ceiling on the downloaded tarball. The real release is ~30 MB;
/// a malicious mirror returning an unbounded stream would otherwise fill
/// `$HOME/.cache`.
const MAX_TARBALL_BYTES: u64 = 500 * 1024 * 1024;

/// Upper bound on any single HTTP call (US-001). 30 seconds is long enough
/// for a cold-start tethered connection, short enough that a zombie thread
/// never forms on a dropped TCP half-open.
const UPDATE_HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Run the tar.gz self-update end-to-end. Resolves `$HOME`, delegates to
/// [`run_update_in`], then returns the path of the post-swap binary
/// (suitable for `cx.set_restart_path()`).
pub fn run_update(asset_url: &str) -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME environment variable is not set")?;
    let app_dir = home.join(".local").join("paneflow.app");
    // US-010: honour `XDG_CACHE_HOME` (mirrors `appimage::cache_path_for`)
    // so a user who redirects their cache dir isn't forced back to
    // `~/.cache`. Falls back to `$HOME/.cache` when unset.
    let cache_base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".cache"));
    let cache_dir = cache_base.join(crate::runtime_paths::APP_SUBDIR);
    run_update_in(asset_url, &app_dir, &cache_dir)?;
    Ok(app_dir.join("bin").join("paneflow"))
}

/// Testable core. Parameterised on `app_dir` + `cache_dir` so the whole
/// flow can run under a `tempdir` in unit tests without touching the real
/// `$HOME`.
fn run_update_in(asset_url: &str, app_dir: &Path, cache_dir: &Path) -> Result<()> {
    let (old_dir, new_dir) = staging_dirs(app_dir)?;
    let parent = app_dir
        .parent()
        .context("app_dir has no parent directory - refusing to swap at filesystem root")?;

    // US-008: serialise concurrent updates. Two PaneFlow instances both
    // triggering an update would otherwise race on the fixed-name `.old` /
    // `.new` staging dirs and the two-rename swap. The lock is an OS-advisory
    // flock that auto-releases when the handle drops - including on process
    // death - so a crash never leaves a stale lock behind. Held for the whole
    // staging+swap below.
    let _update_lock = acquire_update_lock(parent)?;

    // US-008: crash recovery + stale-`.old` cleanup. Replaces the old hard
    // `bail!` on any `.old`: if a prior update died mid-swap (live `app_dir`
    // renamed to `.old`, but `.new → app_dir` never ran) the live install is
    // restored; an otherwise-leftover `.old` (failed housekeeping, live
    // install intact) is removed so we don't bail forever.
    recover_and_clean_staging(app_dir, &old_dir)?;

    // And if `.new` survived a crash, clean it up so extract() doesn't
    // merge into a stale tree. `.new` is pure scratch - safe to remove, but
    // log a warning if the cleanup itself fails so the downstream extract
    // error isn't misdiagnosed as a tarball problem.
    if new_dir.exists()
        && let Err(e) = std::fs::remove_dir_all(&new_dir)
    {
        log::warn!(
            "self-update/targz: could not clean stale {}: {e}",
            new_dir.display()
        );
    }

    std::fs::create_dir_all(cache_dir)
        .with_context(|| format!("create cache dir {}", cache_dir.display()))?;

    let tarball = cache_dir.join(format!("update-{}.tar.gz", std::process::id()));
    let download_result = download_with_verification(asset_url, &tarball);
    if let Err(e) = download_result {
        let _ = std::fs::remove_file(&tarball);
        return Err(e);
    }

    let extract_result = extract_and_swap(&tarball, app_dir, &new_dir, &old_dir);
    // Tarball cleanup is best-effort - keeping a stale ~30 MB file around
    // on disk is strictly preferable to failing the update over it.
    let _ = std::fs::remove_file(&tarball);
    extract_result
}

fn staging_dirs(app_dir: &Path) -> Result<(PathBuf, PathBuf)> {
    let parent = app_dir
        .parent()
        .context("app_dir has no parent directory - refusing to swap at filesystem root")?;
    let name = app_dir
        .file_name()
        .context("app_dir has no file name - refusing to swap")?;
    let name = name.to_string_lossy();
    Ok((
        parent.join(format!("{name}.old")),
        parent.join(format!("{name}.new")),
    ))
}

/// Acquire an exclusive, OS-advisory lock so two PaneFlow instances can't
/// stage+swap concurrently (US-008). The lock auto-releases when the returned
/// handle drops - including on process death - so a crash never leaves a
/// stale lock that would block future updates.
///
/// Unix-only (`flock`). The tar.gz install method is Linux-only at runtime
/// (the dispatcher routes only `InstallMethod::TarGz` here, and that is never
/// produced on Windows), so the non-Unix arm is a compile-only no-op.
fn acquire_update_lock(parent: &Path) -> Result<Option<std::fs::File>> {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let lock_path = parent.join(".paneflow-update.lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("open update lock {}", lock_path.display()))?;
        // SAFETY: `flock` on a freshly opened, owned fd. `LOCK_NB` makes it
        // non-blocking - it returns `EWOULDBLOCK` if another instance holds
        // the lock rather than hanging the update thread.
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
                bail!(
                    "Another PaneFlow update is already in progress. Wait for it to finish, then retry."
                );
            }
            return Err(err).context("flock update lock");
        }
        Ok(Some(file))
    }
    #[cfg(not(unix))]
    {
        let _ = parent;
        Ok(None)
    }
}

/// Recover from a crash mid-swap and clean a stale `.old` (US-008). Replaces
/// the old hard `bail!` on any pre-existing `.old`:
///
/// - `.old` present, live `app_dir` **missing** → the prior update died
///   between the two swap renames; restore the live install by renaming
///   `.old` back to `app_dir`.
/// - `.old` present, live `app_dir` intact → leftover from a failed
///   housekeeping `rm` (the previous version); remove it best-effort so we
///   don't bail forever.
/// - neither → no-op.
fn recover_and_clean_staging(app_dir: &Path, old_dir: &Path) -> Result<()> {
    if !old_dir.exists() {
        return Ok(());
    }
    if !app_dir.exists() {
        std::fs::rename(old_dir, app_dir).with_context(|| {
            format!(
                "recover live install {} ← {}",
                app_dir.display(),
                old_dir.display()
            )
        })?;
        log::warn!(
            "self-update/targz: recovered live install from a crashed prior update ({})",
            app_dir.display()
        );
        return Ok(());
    }
    if let Err(e) = std::fs::remove_dir_all(old_dir) {
        log::warn!(
            "self-update/targz: could not remove stale {}: {e}",
            old_dir.display()
        );
    }
    Ok(())
}

/// Download the asset, verify its detached **minisign** signature, and
/// persist to `dest`. The signature - not a same-host `.sha256` - is the
/// trust anchor (US-001): verification runs against a public key baked into
/// this binary, so a compromised mirror or MITM cannot make us extract a
/// tampered tarball.
///
/// On any failure the partial download is removed by the caller - we
/// don't want a half-written tarball to masquerade as a cached update on
/// the next run.
fn download_with_verification(asset_url: &str, dest: &Path) -> Result<()> {
    super::super::verified_download::download_verified_asset(
        asset_url,
        dest,
        MAX_TARBALL_BYTES,
        UPDATE_HTTP_TIMEOUT,
        "tarball",
    )
}

/// Extract `tarball` into `new_dir` (cleaning any stale tree first), then
/// execute the two-rename swap. On any failure after extraction, `.new`
/// is cleaned up and `app_dir` is left untouched.
fn extract_and_swap(tarball: &Path, app_dir: &Path, new_dir: &Path, old_dir: &Path) -> Result<()> {
    // The tarball is laid out with `paneflow.app/` as the single top
    // entry (see scripts/bundle-tarball.sh), so we extract into `new_dir`'s
    // parent and then rename the produced `paneflow.app` to `new_dir`.
    let parent = app_dir
        .parent()
        .context("app_dir has no parent directory")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create parent {}", parent.display()))?;

    // Extract into a fresh scratch directory (not `parent` directly) so
    // we can't clobber an unrelated sibling if the tarball's top-level
    // entry name ever changes, then promote the result with an atomic
    // rename (below). The extraction itself is hardened in
    // [`extract_hardened`] (US-003): every entry is filtered *during*
    // iteration, not after.
    //
    // `.paneflow-extract-<pid>` makes two PaneFlow instances updating at
    // once work: same-parent but distinct scratch dirs.
    let scratch = parent.join(format!(".paneflow-extract-{}", std::process::id()));
    if scratch.exists() {
        let _ = std::fs::remove_dir_all(&scratch);
    }
    std::fs::create_dir_all(&scratch)
        .with_context(|| format!("create scratch {}", scratch.display()))?;

    let extract_result = (|| -> Result<()> {
        // US-003: hardened extraction. Filters every entry as it is read
        // (reject `..`, absolute paths, drive prefixes, and any symlink /
        // hardlink), with permissions and xattrs disabled, plus a per-entry
        // containment check against the canonical scratch root. This replaces
        // the old `archive.unpack()` + post-extract symlink sweep with a
        // single pass that never writes an unsafe entry to disk at all
        // (TARmageddon-class, CVE-2025-59825).
        extract_hardened(tarball, &scratch)?;

        // Move the top-level extracted directory into place as `new_dir`.
        // Strict "exactly one top-level directory" - the bundler writes
        // exactly `paneflow.app/` at the top (see
        // `scripts/bundle-tarball.sh`) and any deviation is more likely
        // a CI bug than a layout we should silently accept.
        let top = find_single_top_level(&scratch)?;
        if new_dir.exists() {
            let _ = std::fs::remove_dir_all(new_dir);
        }
        std::fs::rename(&top, new_dir)
            .with_context(|| format!("rename {} → {}", top.display(), new_dir.display()))?;

        // Belt-and-braces: force the restart binary to a known-good mode.
        // The bundler ships it 0o755, but a tampered archive could
        // downgrade to 0o777 (world-writable - local-priv-esc on shared
        // HOMEs) or upgrade to 0o700 (unexecutable by other session-id
        // processes). Force 0o755, which is the tarball's original mode.
        //
        // US-005 - Unix-only belt. Windows has no POSIX mode bits, so the
        // threat model above (mode-based priv-esc) does not apply; the
        // analogous concern would be ACL tampering, handled separately
        // if/when Windows ever ships a tar.gz update path (today it
        // uses MSI from EP-W4).
        #[cfg(unix)]
        {
            let bin_path = new_dir.join("bin").join("paneflow");
            if bin_path.exists() {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&bin_path)
                    .with_context(|| format!("stat {}", bin_path.display()))?
                    .permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&bin_path, perms)
                    .with_context(|| format!("chmod 0o755 {}", bin_path.display()))?;
            }
        }
        Ok(())
    })();

    // Scratch is always cleaned up, even on success (the useful content
    // is already at `new_dir`).
    let _ = std::fs::remove_dir_all(&scratch);

    extract_result.inspect_err(|_| {
        let _ = std::fs::remove_dir_all(new_dir);
    })?;

    // Atomic swap. Two renames; brief window but same-filesystem so each
    // is atomic individually. `app_dir.old` must not exist (checked by
    // caller); any failure is aggressively rolled back.
    if app_dir.exists() {
        std::fs::rename(app_dir, old_dir)
            .with_context(|| format!("rename {} → {}", app_dir.display(), old_dir.display()))?;
    }
    if let Err(e) = std::fs::rename(new_dir, app_dir) {
        // Roll back: restore `app_dir` from `.old`. If the rollback
        // rename ALSO fails, the user has no live install at all -
        // surface both errors and the actual on-disk location of their
        // previous version so they can recover manually instead of
        // silently discarding the rollback error.
        let rollback_msg = if old_dir.exists() {
            match std::fs::rename(old_dir, app_dir) {
                Ok(()) => None,
                Err(rb) => Some(format!(
                    "rollback also failed ({rb}); your previous install is at {}",
                    old_dir.display()
                )),
            }
        } else {
            None
        };
        let _ = std::fs::remove_dir_all(new_dir);
        return match rollback_msg {
            Some(msg) => Err(e).context(format!("rename .new → app_dir ({msg})")),
            None => Err(e).context("rename .new → app_dir"),
        };
    }

    // Housekeeping: delete the previous version. Best-effort; a transient
    // failure here leaves a `.old` dir for the user to remove manually,
    // but the new install is already live.
    if old_dir.exists() {
        let _ = std::fs::remove_dir_all(old_dir);
    }

    Ok(())
}

/// Extract `tarball` into `scratch`, filtering every entry as it is read
/// (US-003). Unlike `tar::Archive::unpack`, this:
///
/// - **Disables permission and xattr preservation** - a tampered archive
///   must not be able to set setuid / world-writable bits or attach
///   quarantine-bypassing xattrs. The restart binary's mode is forced back
///   to `0o755` by the caller after extraction.
/// - **Rejects link entries** (symlink *and* hardlink) outright. Our
///   bundler produces a link-free layout; a link is either a CI regression
///   or a tampered archive pointing `bin/paneflow` at `/etc/passwd` or
///   hardlinking a file outside the root (TARmageddon-class,
///   CVE-2025-59825). Rejecting during iteration means the link is never
///   materialised, closing the post-extract-walk race entirely.
/// - **Validates path components** before extraction (no absolute path, no
///   root/drive prefix, no `..` traversal) and relies on `unpack_in`'s own
///   containment check against the canonicalised root as a second layer.
///
/// `sync` `tar` only (never `tokio-tar`/`async-tar`, whose streaming
/// extractors are the ones the TARmageddon advisory implicates).
fn extract_hardened(tarball: &Path, scratch: &Path) -> Result<()> {
    let f = std::fs::File::open(tarball).with_context(|| format!("open {}", tarball.display()))?;
    let gz = flate2::read::GzDecoder::new(f);
    let mut archive = tar::Archive::new(gz);
    archive.set_preserve_permissions(false);
    archive.set_unpack_xattrs(false);
    archive.set_preserve_mtime(false);

    // Canonical root for the per-entry containment check. `scratch` was just
    // created by the caller, so canonicalisation resolves any symlinked
    // ancestor (e.g. `/tmp` → `/private/tmp` on macOS) to a real prefix.
    let canonical_root = std::fs::canonicalize(scratch)
        .with_context(|| format!("canonicalize scratch root {}", scratch.display()))?;

    for entry in archive.entries().context("read tar entries")? {
        let mut entry = entry.context("read tar entry")?;
        let entry_type = entry.header().entry_type();

        if entry_type.is_symlink() || entry_type.is_hard_link() {
            let where_at = entry
                .path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "<unreadable path>".to_string());
            bail!(
                "Update archive contains a link entry at {where_at}, which PaneFlow refuses to install. Download the release manually from the releases page."
            );
        }

        let path = entry.path().context("read tar entry path")?.into_owned();
        validate_extract_path(&path)?;

        // `unpack_in` performs its own containment check against
        // `canonical_root` and returns `false` if it refused the entry -
        // belt-and-braces against anything `validate_extract_path` missed.
        // It only ever writes under `canonical_root`.
        let unpacked = entry
            .unpack_in(&canonical_root)
            .with_context(|| format!("unpack entry {}", path.display()))?;
        if !unpacked {
            bail!(
                "Update archive entry {} escapes the extraction root - refusing to install. Download the release manually from the releases page.",
                path.display()
            );
        }
    }
    Ok(())
}

/// Reject a tar entry path that is absolute, carries a Windows drive/root
/// prefix, or contains a `..` traversal component. Pure (no I/O) so it is
/// trivially unit-testable; the filesystem-level containment check lives in
/// `unpack_in`.
fn validate_extract_path(path: &Path) -> Result<()> {
    use std::path::Component;
    for comp in path.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => bail!(
                "Update archive contains an absolute path ({}) - refusing to install.",
                path.display()
            ),
            Component::ParentDir => bail!(
                "Update archive contains a `..` traversal ({}) - refusing to install.",
                path.display()
            ),
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    Ok(())
}

/// Find the single top-level directory inside `dir`, returning its path.
/// Errors if the archive's shape is ambiguous (zero or multiple top
/// entries, or a file instead of a directory at the top).
fn find_single_top_level(dir: &Path) -> Result<PathBuf> {
    let mut entries = std::fs::read_dir(dir)
        .with_context(|| format!("read_dir {}", dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()
        .context("collect dir entries")?;
    entries.sort_by_key(|e| e.file_name());
    match entries.as_slice() {
        [only] => {
            let ty = only.file_type().context("inspect top-level entry")?;
            if !ty.is_dir() {
                bail!("tarball top-level entry is not a directory");
            }
            Ok(only.path())
        }
        [] => bail!("tarball is empty - no top-level entry"),
        multi => bail!("tarball has {} top-level entries, expected 1", multi.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Pure helpers ───────────────────────────────────────────────────

    #[test]
    fn staging_dirs_derives_sibling_paths() {
        let (old, new) = staging_dirs(Path::new("/home/u/.local/paneflow.app")).unwrap();
        assert_eq!(old, PathBuf::from("/home/u/.local/paneflow.app.old"));
        assert_eq!(new, PathBuf::from("/home/u/.local/paneflow.app.new"));
    }

    // ── extract_and_swap fixtures ──────────────────────────────────────

    /// Build a minimal tar.gz containing `paneflow.app/bin/paneflow` with
    /// a marker string. Returns its path.
    fn make_fixture_tarball(root: &Path, marker: &[u8]) -> PathBuf {
        let out = root.join("fixture.tar.gz");
        let f = std::fs::File::create(&out).unwrap();
        let gz = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        let mut builder = tar::Builder::new(gz);

        let bin_path = root.join("fixture-src/paneflow.app/bin/paneflow");
        std::fs::create_dir_all(bin_path.parent().unwrap()).unwrap();
        std::fs::write(&bin_path, marker).unwrap();

        builder
            .append_dir_all("paneflow.app", root.join("fixture-src/paneflow.app"))
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();
        out
    }

    #[test]
    fn extract_and_swap_replaces_existing_app_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let tarball = make_fixture_tarball(root, b"new-version");

        let app_dir = root.join("home/.local/paneflow.app");
        std::fs::create_dir_all(app_dir.join("bin")).unwrap();
        std::fs::write(app_dir.join("bin/paneflow"), b"old-version").unwrap();
        let (old_dir, new_dir) = staging_dirs(&app_dir).unwrap();

        extract_and_swap(&tarball, &app_dir, &new_dir, &old_dir).unwrap();

        let content = std::fs::read(app_dir.join("bin/paneflow")).unwrap();
        assert_eq!(content, b"new-version");
        assert!(!old_dir.exists(), ".old should be cleaned up");
        assert!(!new_dir.exists(), ".new should be gone post-swap");
    }

    #[test]
    fn extract_and_swap_works_when_app_dir_absent() {
        // Fresh install (no prior app_dir) must still succeed.
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let tarball = make_fixture_tarball(root, b"fresh-install");

        let app_dir = root.join("home/.local/paneflow.app");
        std::fs::create_dir_all(app_dir.parent().unwrap()).unwrap();
        let (old_dir, new_dir) = staging_dirs(&app_dir).unwrap();

        extract_and_swap(&tarball, &app_dir, &new_dir, &old_dir).unwrap();
        assert_eq!(
            std::fs::read(app_dir.join("bin/paneflow")).unwrap(),
            b"fresh-install"
        );
    }

    #[test]
    fn extract_and_swap_rolls_back_on_corrupt_tarball() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let corrupt = root.join("corrupt.tar.gz");
        std::fs::write(&corrupt, b"not actually a gzip").unwrap();

        let app_dir = root.join("home/.local/paneflow.app");
        std::fs::create_dir_all(app_dir.join("bin")).unwrap();
        std::fs::write(app_dir.join("bin/paneflow"), b"keep-me").unwrap();
        let (old_dir, new_dir) = staging_dirs(&app_dir).unwrap();

        let r = extract_and_swap(&corrupt, &app_dir, &new_dir, &old_dir);
        assert!(r.is_err(), "corrupt tarball must fail");

        // The live install is preserved.
        assert_eq!(
            std::fs::read(app_dir.join("bin/paneflow")).unwrap(),
            b"keep-me"
        );
        assert!(!new_dir.exists(), ".new must be cleaned up on failure");
        assert!(!old_dir.exists(), ".old must not exist on failure");
    }

    // ── US-008: crash recovery + concurrency lock ──────────────────────

    #[test]
    fn recover_restores_live_install_when_app_dir_missing() {
        // Crash mid-swap: `.old` holds the only copy, live `app_dir` is gone.
        let tmp = tempfile::TempDir::new().unwrap();
        let app_dir = tmp.path().join(".local/paneflow.app");
        let (old_dir, _new) = staging_dirs(&app_dir).unwrap();
        std::fs::create_dir_all(old_dir.join("bin")).unwrap();
        std::fs::write(old_dir.join("bin/paneflow"), b"prev-version").unwrap();

        recover_and_clean_staging(&app_dir, &old_dir).unwrap();

        assert!(app_dir.exists(), "live install must be restored from .old");
        assert_eq!(
            std::fs::read(app_dir.join("bin/paneflow")).unwrap(),
            b"prev-version"
        );
        assert!(!old_dir.exists(), ".old must be consumed by the recovery");
    }

    #[test]
    fn recover_removes_stale_old_when_app_dir_intact() {
        // Leftover `.old` (failed housekeeping) coexisting with a live
        // install: clean it up and proceed, never bail forever.
        let tmp = tempfile::TempDir::new().unwrap();
        let app_dir = tmp.path().join(".local/paneflow.app");
        std::fs::create_dir_all(app_dir.join("bin")).unwrap();
        std::fs::write(app_dir.join("bin/paneflow"), b"live").unwrap();
        let (old_dir, _new) = staging_dirs(&app_dir).unwrap();
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join("junk"), b"x").unwrap();

        recover_and_clean_staging(&app_dir, &old_dir).unwrap();

        assert!(!old_dir.exists(), "stale .old must be removed");
        assert_eq!(
            std::fs::read(app_dir.join("bin/paneflow")).unwrap(),
            b"live",
            "live install must be untouched"
        );
    }

    #[test]
    fn recover_is_noop_when_no_old_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let app_dir = tmp.path().join(".local/paneflow.app");
        std::fs::create_dir_all(&app_dir).unwrap();
        let (old_dir, _new) = staging_dirs(&app_dir).unwrap();
        // No `.old` - nothing to do, must succeed.
        recover_and_clean_staging(&app_dir, &old_dir).unwrap();
        assert!(app_dir.exists());
    }

    #[cfg(unix)]
    #[test]
    fn update_lock_is_exclusive_then_released_on_drop() {
        let tmp = tempfile::TempDir::new().unwrap();
        let parent = tmp.path();
        // First holder succeeds.
        let guard = acquire_update_lock(parent).unwrap();
        assert!(guard.is_some());
        // A second concurrent acquisition must fail (lock held).
        let second = acquire_update_lock(parent);
        assert!(second.is_err(), "second concurrent lock must be refused");
        // Dropping the first releases the flock; a fresh acquisition succeeds.
        drop(guard);
        let third = acquire_update_lock(parent).unwrap();
        assert!(third.is_some(), "lock must be re-acquirable after release");
    }

    #[test]
    fn never_writes_outside_its_roots() {
        // End-to-end swap confined to a tempdir. We snapshot every entry
        // under a curated "outside" subtree before and after, and verify
        // no file was added, removed, or modified there. Everything the
        // updater is allowed to touch lives under `home/.local/paneflow.*`
        // (for the swap) or under the scratch dir (which must not
        // persist post-call); anything else is a bug.
        //
        // The real guarantee comes from code inspection: every path in
        // this module is derived from an input `app_dir`/`cache_dir`,
        // never from a hardcoded absolute path. This test catches
        // regressions where that invariant is violated.
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        // Outside subtree: pre-populate with files whose names/contents
        // we can assert on afterwards.
        let outside = root.join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("a"), b"a-body").unwrap();
        std::fs::write(outside.join("b"), b"b-body").unwrap();
        std::fs::create_dir(outside.join("sub")).unwrap();
        std::fs::write(outside.join("sub/c"), b"c-body").unwrap();
        let before = snapshot_tree(&outside);

        let tarball = make_fixture_tarball(root, b"body");
        let app_dir = root.join("home/.local/paneflow.app");
        std::fs::create_dir_all(app_dir.parent().unwrap()).unwrap();
        let (old_dir, new_dir) = staging_dirs(&app_dir).unwrap();

        extract_and_swap(&tarball, &app_dir, &new_dir, &old_dir).unwrap();

        let after = snapshot_tree(&outside);
        assert_eq!(before, after, "updater touched the outside subtree");
    }

    /// Collect `(relative_path, contents)` pairs for every regular file
    /// under `root`. Deterministic ordering (BTreeMap-like via sort).
    fn snapshot_tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out: Vec<(PathBuf, Vec<u8>)> = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(p) = stack.pop() {
            for entry in std::fs::read_dir(&p).unwrap().flatten() {
                let ft = entry.file_type().unwrap();
                if ft.is_dir() {
                    stack.push(entry.path());
                } else if ft.is_file() {
                    let rel = entry.path().strip_prefix(root).unwrap().to_path_buf();
                    out.push((rel, std::fs::read(entry.path()).unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    // ── US-003: hardened extraction ────────────────────────────────────

    #[test]
    fn validate_extract_path_accepts_normal_relative_paths() {
        assert!(validate_extract_path(Path::new("paneflow.app/bin/paneflow")).is_ok());
        assert!(validate_extract_path(Path::new("./paneflow.app/lib/x.so")).is_ok());
    }

    #[test]
    fn validate_extract_path_rejects_traversal_and_absolute() {
        assert!(validate_extract_path(Path::new("../evil")).is_err());
        assert!(validate_extract_path(Path::new("paneflow.app/../../etc/passwd")).is_err());
        assert!(validate_extract_path(Path::new("/etc/passwd")).is_err());
        // A Windows drive-prefixed absolute path (rejected via Component::Prefix
        // on Windows; on Linux `C:\…` is one Normal component and is harmless
        // because it can't escape the root, so only assert the cross-platform
        // cases above).
    }

    /// US-003 AC4: an entry whose path traverses out of the root via `..`
    /// is rejected *during* extraction - it never lands on disk and the
    /// live install is untouched.
    #[test]
    fn extract_rejects_path_traversal_entry() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        // Craft a tarball with a single `../evil` entry. `append_data` /
        // `set_path` sanitise `..` away, so write the unsanitised name
        // straight into the GNU header's name field (offset 0..100) - this
        // is exactly what a hostile, hand-rolled archive looks like.
        let out = root.join("evil.tar.gz");
        let f = std::fs::File::create(&out).unwrap();
        let gz = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        let mut builder = tar::Builder::new(gz);
        let payload = b"pwned";
        let mut header = tar::Header::new_gnu();
        header.set_size(payload.len() as u64);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(0o644);
        {
            let name = b"../evil";
            let bytes = header.as_mut_bytes();
            bytes[..name.len()].copy_from_slice(name);
        }
        header.set_cksum();
        builder.append(&header, &payload[..]).unwrap();
        builder.into_inner().unwrap().finish().unwrap();

        let scratch = root.join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();
        let err = extract_hardened(&out, &scratch).unwrap_err().to_string();
        assert!(
            err.contains("traversal") || err.contains("escapes"),
            "expected traversal rejection, got: {err}"
        );
        // The escaping target must never have been written.
        assert!(
            !root.join("evil").exists(),
            "traversal entry must not write outside the scratch root"
        );
    }

    // US-003 - Unix-only fixture. The test injects a real symlink entry into
    // a tarball via `std::os::unix::fs::symlink`; building that entry on
    // Windows needs `SeCreateSymbolicLinkPrivilege`, which non-admin GH
    // Actions runs lack. The runtime property - `extract_hardened` refuses
    // any link entry - is platform-neutral (it checks the tar header's entry
    // type, not the host filesystem), so Windows gets the same guard; only
    // the malicious-fixture construction is Unix-only.
    #[cfg(unix)]
    #[test]
    fn extract_rejects_tarball_with_symlink() {
        // Build a tarball that includes a symlink and verify that
        // `extract_and_swap` refuses to install it - even though the
        // symlink itself never leaves the scratch dir, letting it reach
        // the live `app_dir` would let a tampered release point
        // `bin/paneflow` at `/home/u/.ssh/id_rsa` and exfiltrate on
        // restart.
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        // Assemble the tarball manually so we can inject a symlink entry.
        let fixture_src = root.join("src/paneflow.app/bin");
        std::fs::create_dir_all(&fixture_src).unwrap();
        std::fs::write(fixture_src.join("paneflow"), b"real-bin").unwrap();
        let evil_link = fixture_src.join("evil-link");
        std::os::unix::fs::symlink("/etc/passwd", &evil_link).unwrap();

        let out = root.join("fixture.tar.gz");
        let f = std::fs::File::create(&out).unwrap();
        let gz = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        let mut builder = tar::Builder::new(gz);
        // `follow_symlinks(false)` preserves the symlink as a symlink
        // entry in the archive - the whole point of this test.
        builder.follow_symlinks(false);
        builder
            .append_dir_all("paneflow.app", root.join("src/paneflow.app"))
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();

        let app_dir = root.join("home/.local/paneflow.app");
        std::fs::create_dir_all(app_dir.parent().unwrap()).unwrap();
        let (old_dir, new_dir) = staging_dirs(&app_dir).unwrap();

        let r = extract_and_swap(&out, &app_dir, &new_dir, &old_dir);
        let err = r.unwrap_err().to_string();
        assert!(
            err.contains("link entry"),
            "expected link rejection, got: {err}"
        );
        assert!(!app_dir.exists(), "a failed update must not leave app_dir");
        assert!(!new_dir.exists(), ".new must be cleaned up");
    }
}
