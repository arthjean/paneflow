//! Startup migrations for users crossing install-method boundaries.
//!
//! US-008 (EP-002 - Install Method Hygiene): when a user upgrades from the
//! tar.gz flavor (installed under `$HOME/.local/paneflow.app/`) to the signed
//! rpm/deb flavor (installed under `/usr/bin/paneflow`), the tar.gz
//! post-install script leaves stale icon files under
//! `~/.local/share/icons/hicolor/*/apps/paneflow.png`. Those user-local files
//! shadow the rpm/deb-installed icons at `/usr/share/icons/hicolor/...` and
//! keep the desktop shell displaying the previous app icon indefinitely -
//! the exact bug debugged on the v0.2.2 upgrade pass.
//!
//! This module runs once at startup for rpm/deb users only, compares each
//! stale PNG against the corresponding system file by SHA-256, and deletes
//! only the ones that differ (identical copies are treated as intentional
//! overrides and preserved). A marker file under `~/.cache/paneflow/` makes
//! the migration idempotent across subsequent launches.
//!
//! Design choices that the acceptance criteria nail down:
//!
//! - `#[cfg(target_os = "linux")]` - the tar.gz → rpm/deb crossover is
//!   Linux-only by construction (no equivalent hicolor layout on macOS /
//!   Windows).
//! - No `unwrap()` / `expect()` in any production path - the workspace-level
//!   clippy lints warn on both; a failed migration logs and returns instead
//!   of crashing startup.
//! - Argv-free: pure filesystem work, no subprocess, no shell.
//! - Marker name is versioned (`migration-v0.2.3-*`) so a future cleanup
//!   migration can add its own marker without colliding (R10).

#![cfg(target_os = "linux")]

use std::io::{self, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::update::install_method::{InstallMethod, PackageManager};

/// Freedesktop hicolor icon sizes shipped by PaneFlow's rpm/deb payloads
/// (see `package.metadata.deb.assets` and
/// `package.metadata.generate-rpm.assets` in `src-app/Cargo.toml`). Must
/// stay in lockstep with the packaging manifests.
const HICOLOR_SIZES: &[u32] = &[16, 32, 48, 128, 256, 512];

/// Marker filename written under `~/.cache/paneflow/` to short-circuit the
/// migration on subsequent launches. Versioned so additive migrations don't
/// collide (R10).
const MARKER_FILENAME: &str = "migration-v0.2.3-icons-cleaned";

/// Run the stale-icon cleanup when and only when the resolved install method
/// is a native rpm/deb system package. Must be invoked exactly once per
/// process start from `bootstrap.rs`, after `install_method::detect()`.
///
/// The helper deliberately consumes any error it produces: a failed
/// migration degrades to a `log::warn!` but never panics and never blocks
/// startup. See US-008 AC: "a failing migration is better than a crashed
/// startup".
pub fn run_startup_migrations(method: &InstallMethod) {
    let should_run = matches!(
        method,
        InstallMethod::SystemPackage {
            manager: PackageManager::Dnf | PackageManager::Apt | PackageManager::Zypper,
        }
    );
    if !should_run {
        return;
    }

    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        // Sandboxed / container runtimes may not expose $HOME. Nothing to
        // migrate - stay silent rather than warn on every restart.
        return;
    };

    let user_icon_dir = home
        .join(".local")
        .join("share")
        .join("icons")
        .join("hicolor");
    let system_icon_dir = PathBuf::from("/usr/share/icons/hicolor");
    let cache_dir = home.join(".cache").join("paneflow");

    if let Err(err) =
        migrate_user_hicolor_icons(&user_icon_dir, &system_icon_dir, &cache_dir, HICOLOR_SIZES)
    {
        log::warn!(
            "paneflow: hicolor icon migration failed ({err}); leaving user-local icons untouched"
        );
    }
}

/// Pure, parameter-driven implementation so tests can stand up a full layout
/// under a `TempDir` without patching env vars or touching `/usr/share`.
///
/// Semantics:
///
/// - If the marker file exists under `cache_dir`, return `Ok(())` after
///   performing zero additional I/O - proving idempotency.
/// - Otherwise, for each size in `sizes`, compare
///   `user_icon_dir/{N}x{N}/apps/paneflow.png` against
///   `system_icon_dir/{N}x{N}/apps/paneflow.png` by SHA-256. If they differ,
///   remove the user-local file. Identical copies are preserved
///   (user-authored override - `rm` would be user-surprising).
/// - If the system file is missing OR unreadable at any size, skip that
///   size with a `log::warn!` and leave the user-local copy alone (defensive
///   do not nuke state we cannot prove is stale).
/// - If at least one file was deleted, attempt to remove the orphaned
///   `user_icon_dir/icon-theme.cache` but only when
///   `user_icon_dir/index.theme` does NOT exist (presence of `index.theme`
///   marks a legitimate user-owned icon theme, whose `icon-theme.cache`
///   must survive - see US-008 AC).
/// - On any successful run (with or without deletions), write the marker
///   file. A failed marker write logs at `warn!` and returns `Ok(())` -
///   re-running the migration next boot is acceptable; returning `Err`
///   here would make the outer guard log a spurious failure.
fn migrate_user_hicolor_icons(
    user_icon_dir: &Path,
    system_icon_dir: &Path,
    cache_dir: &Path,
    sizes: &[u32],
) -> io::Result<()> {
    let marker_path = cache_dir.join(MARKER_FILENAME);
    if marker_path.exists() {
        // Fast path: marker short-circuit. AC: "without any further
        // filesystem access".
        return Ok(());
    }

    // US-030: distinguish "missing" (Ok(false) → nothing to migrate, mark
    // done) from "cannot determine" (Err, e.g. permission denied on an
    // ancestor). The old `unwrap_or(false)` collapsed Err into "absent" and
    // wrote the marker - marking the migration done even though it never ran.
    // On Err we return without writing the marker so it retries next boot.
    match user_icon_dir.try_exists() {
        Ok(false) => {
            write_marker(cache_dir, &marker_path);
            return Ok(());
        }
        Ok(true) => {}
        Err(e) => return Err(e),
    }

    let mut deleted_any = false;
    // US-030: remember the first deletion failure. A stale icon we couldn't
    // remove means the migration did NOT reach its goal (MigrationOutcome
    // `Failed`), so the marker is left unwritten and we retry next boot -
    // rather than the old code that wrote the marker unconditionally even
    // after a failed `remove_file`.
    let mut remove_failed: Option<io::Error> = None;

    for &size in sizes {
        let rel = format!("{size}x{size}/apps/paneflow.png");
        let user_file = user_icon_dir.join(&rel);
        let system_file = system_icon_dir.join(&rel);

        // `symlink_metadata` does NOT follow symlinks (unlike `metadata` /
        // `File::open`). Reject symlinks outright: hashing via `File::open`
        // would resolve to whatever the symlink targets (potentially a file
        // outside `~/.local/share/icons/hicolor/`), and subsequently
        // `remove_file` would remove the symlink itself (not the target) -
        // surprising in both directions. A legitimate hicolor layout never
        // uses symlinks for per-size PNGs, so skipping them is safe.
        let user_meta = match std::fs::symlink_metadata(&user_file) {
            Ok(m) => m,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => {
                log::warn!(
                    "paneflow: cannot stat user icon {} ({err}); skipping this size",
                    user_file.display()
                );
                continue;
            }
        };
        if user_meta.file_type().is_symlink() {
            log::warn!(
                "paneflow: user icon {} is a symlink; skipping (refusing to follow for hash + remove)",
                user_file.display()
            );
            continue;
        }
        if !user_meta.is_file() {
            // Regular files only - dirs, sockets, fifos should never appear
            // here but if they do, stay out of the way.
            continue;
        }

        let user_hash = match sha256_of(&user_file) {
            Ok(h) => h,
            Err(err) => {
                log::warn!(
                    "paneflow: cannot hash {} ({err}); leaving user-local icon in place",
                    user_file.display()
                );
                continue;
            }
        };

        let system_hash = match sha256_of(&system_file) {
            Ok(h) => h,
            Err(err) => {
                // Defensive: never delete user-local state when we can't
                // prove the system file is the canonical source.
                log::warn!(
                    "paneflow: system icon {} unreadable ({err}); preserving {}",
                    system_file.display(),
                    user_file.display()
                );
                continue;
            }
        };

        if user_hash == system_hash {
            // Identical → treat as intentional override, preserve.
            continue;
        }

        match std::fs::remove_file(&user_file) {
            Ok(()) => {
                log::info!(
                    "paneflow: removed stale user-local icon {} (sha256 differs from system copy)",
                    user_file.display()
                );
                deleted_any = true;
            }
            Err(err) => {
                log::warn!(
                    "paneflow: cannot remove stale user-local icon {} ({err})",
                    user_file.display()
                );
                if remove_failed.is_none() {
                    remove_failed = Some(err);
                }
            }
        }
    }

    if deleted_any {
        maybe_remove_orphaned_cache(user_icon_dir);
    }

    // US-030: write the marker only on a genuinely complete run (Completed /
    // Skipped). A deletion failure propagates as `Err`, leaving the marker
    // unset so the migration is retried.
    if let Some(err) = remove_failed {
        return Err(err);
    }
    write_marker(cache_dir, &marker_path);
    Ok(())
}

/// Remove `user_icon_dir/icon-theme.cache` iff `index.theme` does NOT exist.
/// If `index.theme` is present, the cache belongs to a legitimate
/// user-owned theme (e.g. a Flatpak-managed partial hicolor override) and
/// must survive.
fn maybe_remove_orphaned_cache(user_icon_dir: &Path) {
    let cache_file = user_icon_dir.join("icon-theme.cache");
    let index_file = user_icon_dir.join("index.theme");

    let cache_exists = cache_file.try_exists().unwrap_or(false);
    if !cache_exists {
        return;
    }
    let index_exists = index_file.try_exists().unwrap_or(false);
    if index_exists {
        log::info!(
            "paneflow: user-local hicolor theme has an index.theme at {}; preserving icon-theme.cache",
            index_file.display()
        );
        return;
    }

    match std::fs::remove_file(&cache_file) {
        Ok(()) => log::info!(
            "paneflow: removed orphaned user-local icon-theme.cache at {}",
            cache_file.display()
        ),
        Err(err) => log::warn!(
            "paneflow: cannot remove orphaned icon-theme.cache at {} ({err})",
            cache_file.display()
        ),
    }
}

/// Write the migration marker file under `cache_dir`, creating the directory
/// if missing. A failure here is non-fatal by design - see module doc.
fn write_marker(cache_dir: &Path, marker_path: &Path) {
    if let Err(err) = std::fs::create_dir_all(cache_dir) {
        log::warn!(
            "paneflow: cannot create cache dir {} ({err}); migration marker will retry next boot",
            cache_dir.display()
        );
        return;
    }
    if let Err(err) = std::fs::write(marker_path, b"v0.2.3 hicolor cleanup\n") {
        log::warn!(
            "paneflow: cannot write migration marker {} ({err}); migration will retry next boot",
            marker_path.display()
        );
    }
}

/// Stream-hash a file with SHA-256. Uses a 64 KiB buffer (matches the
/// existing pattern in `update::linux::targz::verify_sha256_of_file`).
fn sha256_of(path: &Path) -> io::Result<[u8; 32]> {
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().into())
}

// ────────────────────────────────────────────────────────────────────────────
// US-009 - Coexistence detection + one-time advisory toast.
// ────────────────────────────────────────────────────────────────────────────

/// Marker filename written once the coexistence-advisory toast has been
/// shown. Users who genuinely want dual installs (dev + production
/// side-by-side) can keep the marker in place to silence the toast forever,
/// or delete it to re-surface it after they've cleaned up.
pub const COEXISTENCE_MARKER_FILENAME: &str = "migration-v0.2.3-coexistence-warned";

/// Canonical absolute path of the rpm/deb-installed binary. Shared by US-008
/// and US-009 so a rename is a single-site change.
const SYSTEM_BIN_PATH: &str = "/usr/bin/paneflow";

/// Summary of a coexistent-install situation: running binary + the other
/// install's binary + a human label ("tar.gz" / "system package") used in
/// the toast message. Returned by [`detect_coexistent_install`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoexistenceReport {
    /// Absolute path to the binary the current process was launched from.
    /// For SystemPackage this is `/usr/bin/paneflow`; for TarGz it's the
    /// binary under `$HOME/.local/paneflow.app/bin/paneflow`.
    pub running_path: PathBuf,
    /// Absolute path to the *other* install's binary - the one the user
    /// should consider removing to avoid version drift.
    pub other_path: PathBuf,
    /// Short human label for `other_path`'s install flavour. Static so
    /// the struct stays `Clone` + `Eq` + allocation-light.
    pub other_method_label: &'static str,
}

/// US-009 AC: detect when the host carries both flavours of PaneFlow at
/// once. Returns `Some(CoexistenceReport)` when the current process is
/// running from one well-known location AND the other well-known location
/// also contains a PaneFlow binary:
///
/// - `SystemPackage { Dnf | Apt | Zypper }` running + `$HOME/.local/paneflow.app/bin/paneflow` present
/// - `TarGz` running + `/usr/bin/paneflow` present
///
/// All other variants - AppImage, AppBundle, WindowsMsi, Unknown, and the
/// non-apt/dnf system-package managers (RpmOstree, Other) - return `None`,
/// because those install layouts never coexist with the tar.gz flavor.
/// Missing `$HOME` also returns `None` (conservative: sandboxed / container
/// runtimes shouldn't produce false positives).
pub fn detect_coexistent_install(current: &InstallMethod) -> Option<CoexistenceReport> {
    detect_coexistent_install_with_probes(
        current,
        std::env::var_os("HOME").map(PathBuf::from),
        Path::new(SYSTEM_BIN_PATH).exists(),
        |p: &Path| p.exists(),
    )
}

/// Pure, parameter-driven core for [`detect_coexistent_install`]. The three
/// probes are taken as parameters so tests can drive every combination
/// without touching real filesystem state.
fn detect_coexistent_install_with_probes<F: FnOnce(&Path) -> bool>(
    current: &InstallMethod,
    home: Option<PathBuf>,
    system_bin_exists: bool,
    tar_gz_bin_probe: F,
) -> Option<CoexistenceReport> {
    match current {
        InstallMethod::SystemPackage {
            manager: PackageManager::Dnf | PackageManager::Apt | PackageManager::Zypper,
        } => {
            let home = home?;
            let other = home
                .join(".local")
                .join("paneflow.app")
                .join("bin")
                .join("paneflow");
            if tar_gz_bin_probe(&other) {
                Some(CoexistenceReport {
                    running_path: PathBuf::from(SYSTEM_BIN_PATH),
                    other_path: other,
                    other_method_label: "tar.gz",
                })
            } else {
                None
            }
        }
        InstallMethod::TarGz { app_dir } => {
            if system_bin_exists {
                Some(CoexistenceReport {
                    running_path: app_dir.join("bin").join("paneflow"),
                    other_path: PathBuf::from(SYSTEM_BIN_PATH),
                    other_method_label: "system package",
                })
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Resolve the coexistence marker path: `~/.cache/paneflow/<MARKER_FILENAME>`.
pub fn coexistence_marker_path(home: &Path) -> PathBuf {
    home.join(".cache")
        .join("paneflow")
        .join(COEXISTENCE_MARKER_FILENAME)
}

/// Write the coexistence marker, creating the parent dir first. On any I/O
/// failure the helper logs at `warn!` and returns - the toast may then
/// recur next session, which US-009 AC accepts explicitly ("low annoyance").
pub fn write_coexistence_marker(marker_path: &Path) {
    if let Some(parent) = marker_path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        log::warn!(
            "paneflow: cannot create cache dir {} ({err}); coexistence toast may recur next session",
            parent.display()
        );
        return;
    }
    if let Err(err) = std::fs::write(marker_path, b"v0.2.3 coexistence warned\n") {
        log::warn!(
            "paneflow: cannot write coexistence marker {} ({err}); toast may recur next session",
            marker_path.display()
        );
    }
}

/// Orchestration helper kept in the module so the marker short-circuit is
/// testable without standing up a full `PaneFlowApp` + GPUI context. Returns
/// `Some(report)` iff:
///
/// 1. Coexistence is detected (via `detect_coexistent_install_with_probes`), and
/// 2. The marker file at `marker_path` does NOT exist.
///
/// Returns `None` when either condition fails - including the "already
/// warned" case, which US-009 AC requires be a silent no-op on the toast
/// (the caller may still log the detection separately).
///
/// `#[cfg(test)]`-gated: the production path in `bootstrap.rs` splits the
/// detection from the marker check on purpose (it logs every detection,
/// toasts only when the marker is absent), so this combined helper only
/// serves the unit tests.
#[cfg(test)]
pub(crate) fn coexistence_should_warn_with_paths<F: FnOnce(&Path) -> bool>(
    current: &InstallMethod,
    home: Option<PathBuf>,
    system_bin_exists: bool,
    tar_gz_bin_probe: F,
    marker_path: &Path,
) -> Option<CoexistenceReport> {
    let report =
        detect_coexistent_install_with_probes(current, home, system_bin_exists, tar_gz_bin_probe)?;
    if marker_path.exists() {
        return None;
    }
    Some(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Test scaffolding: stand up a fake hicolor layout under a `TempDir`
    /// so every branch of the migration can be exercised without touching
    /// `$HOME` or `/usr/share/icons`.
    struct Layout {
        _tmp: tempfile::TempDir,
        user_icon_dir: PathBuf,
        system_icon_dir: PathBuf,
        cache_dir: PathBuf,
    }

    impl Layout {
        fn new() -> Self {
            let tmp = tempfile::TempDir::new().expect("tempdir");
            let user_icon_dir = tmp.path().join("user/icons/hicolor");
            let system_icon_dir = tmp.path().join("system/icons/hicolor");
            let cache_dir = tmp.path().join("cache/paneflow");
            Self {
                _tmp: tmp,
                user_icon_dir,
                system_icon_dir,
                cache_dir,
            }
        }

        fn write_png(&self, root: &Path, size: u32, bytes: &[u8]) {
            let p = root.join(format!("{size}x{size}/apps/paneflow.png"));
            fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
            fs::write(&p, bytes).expect("write png");
        }

        fn user_png(&self, size: u32) -> PathBuf {
            self.user_icon_dir
                .join(format!("{size}x{size}/apps/paneflow.png"))
        }
    }

    #[test]
    fn migrate_preserves_identical_user_local_files() {
        let layout = Layout::new();
        // Byte-identical copies: user override "happens to" match system.
        layout.write_png(&layout.user_icon_dir, 48, b"same-bytes");
        layout.write_png(&layout.system_icon_dir, 48, b"same-bytes");

        migrate_user_hicolor_icons(
            &layout.user_icon_dir,
            &layout.system_icon_dir,
            &layout.cache_dir,
            &[48],
        )
        .expect("migration should succeed");

        assert!(
            layout.user_png(48).exists(),
            "identical user-local file must be preserved"
        );
        assert!(
            layout.cache_dir.join(MARKER_FILENAME).exists(),
            "marker file must be written after a successful run"
        );
    }

    #[test]
    fn migrate_removes_differing_user_local_files() {
        let layout = Layout::new();
        layout.write_png(&layout.user_icon_dir, 128, b"stale-user-bytes");
        layout.write_png(&layout.system_icon_dir, 128, b"canonical-system-bytes");

        migrate_user_hicolor_icons(
            &layout.user_icon_dir,
            &layout.system_icon_dir,
            &layout.cache_dir,
            &[128],
        )
        .expect("migration should succeed");

        assert!(
            !layout.user_png(128).exists(),
            "differing user-local file must be removed"
        );
        assert!(layout.cache_dir.join(MARKER_FILENAME).exists());
    }

    #[test]
    fn migrate_is_idempotent_via_marker() {
        let layout = Layout::new();
        // Pre-write the marker; leave a "stale" user-local file that a
        // fresh run would normally remove. If the short-circuit works,
        // the file survives untouched.
        fs::create_dir_all(&layout.cache_dir).expect("mkdir cache");
        fs::write(layout.cache_dir.join(MARKER_FILENAME), b"prior run").expect("seed marker");

        layout.write_png(&layout.user_icon_dir, 256, b"would-be-deleted");
        layout.write_png(&layout.system_icon_dir, 256, b"canonical");

        migrate_user_hicolor_icons(
            &layout.user_icon_dir,
            &layout.system_icon_dir,
            &layout.cache_dir,
            &[256],
        )
        .expect("migration should succeed");

        assert!(
            layout.user_png(256).exists(),
            "marker must prevent a second migration run from touching the filesystem"
        );
    }

    #[test]
    fn migrate_preserves_orphaned_user_cache_when_index_theme_present() {
        let layout = Layout::new();
        layout.write_png(&layout.user_icon_dir, 32, b"stale-user");
        layout.write_png(&layout.system_icon_dir, 32, b"canonical");

        // User owns a legitimate partial theme: both index.theme AND
        // icon-theme.cache are present. Even though the PNG is about to be
        // removed, the cache file must stay - it belongs to the theme, not
        // to our stale install.
        fs::write(layout.user_icon_dir.join("index.theme"), b"[Icon Theme]\n")
            .expect("write index.theme");
        fs::write(layout.user_icon_dir.join("icon-theme.cache"), b"cache").expect("write cache");

        migrate_user_hicolor_icons(
            &layout.user_icon_dir,
            &layout.system_icon_dir,
            &layout.cache_dir,
            &[32],
        )
        .expect("migration should succeed");

        assert!(
            !layout.user_png(32).exists(),
            "differing PNG must still be removed"
        );
        assert!(
            layout.user_icon_dir.join("icon-theme.cache").exists(),
            "icon-theme.cache must survive when index.theme is present"
        );
    }

    #[test]
    fn migrate_removes_orphaned_cache_when_index_theme_absent() {
        let layout = Layout::new();
        layout.write_png(&layout.user_icon_dir, 32, b"stale-user");
        layout.write_png(&layout.system_icon_dir, 32, b"canonical");

        // No index.theme → cache is orphaned → remove it once at least one
        // PNG was deleted.
        fs::write(layout.user_icon_dir.join("icon-theme.cache"), b"cache").expect("write cache");

        migrate_user_hicolor_icons(
            &layout.user_icon_dir,
            &layout.system_icon_dir,
            &layout.cache_dir,
            &[32],
        )
        .expect("migration should succeed");

        assert!(
            !layout.user_icon_dir.join("icon-theme.cache").exists(),
            "orphaned icon-theme.cache must be removed"
        );
    }

    #[test]
    fn migrate_skips_silently_when_system_file_missing() {
        let layout = Layout::new();
        // Only the user-local file exists. We cannot verify that the
        // system file is the canonical source → preserve, do not delete.
        layout.write_png(&layout.user_icon_dir, 16, b"user-only");

        migrate_user_hicolor_icons(
            &layout.user_icon_dir,
            &layout.system_icon_dir,
            &layout.cache_dir,
            &[16],
        )
        .expect("migration should succeed even without system file");

        assert!(
            layout.user_png(16).exists(),
            "user-local file must be preserved when system file is missing"
        );
        // Marker still gets written on an overall-successful run (no
        // deletions is still a valid terminal state - we don't want to
        // re-run forever against unrecoverably-missing system icons).
        assert!(layout.cache_dir.join(MARKER_FILENAME).exists());
    }

    #[test]
    fn migrate_refuses_to_follow_user_icon_symlinks() {
        // Defence-in-depth: a crafted hicolor symlink must NOT be followed
        // (neither for sha256 hashing nor for removal). The migration
        // should skip the symlink entirely and leave BOTH the symlink and
        // the target file in place.
        use std::os::unix::fs::symlink;
        let layout = Layout::new();
        layout.write_png(&layout.system_icon_dir, 48, b"canonical");

        // Target lives outside the hicolor tree on purpose - if the
        // migration followed the link we'd hash arbitrary user state.
        let target = layout._tmp.path().join("elsewhere.txt");
        fs::write(&target, b"definitely not an icon").expect("write target");

        let link_path = layout.user_icon_dir.join("48x48/apps/paneflow.png");
        fs::create_dir_all(link_path.parent().expect("parent")).expect("mkdir");
        symlink(&target, &link_path).expect("create symlink");

        migrate_user_hicolor_icons(
            &layout.user_icon_dir,
            &layout.system_icon_dir,
            &layout.cache_dir,
            &[48],
        )
        .expect("migration must not fail when encountering a symlink");

        assert!(
            link_path.symlink_metadata().is_ok(),
            "symlink entry must survive - migration refuses to remove it"
        );
        assert!(
            target.exists(),
            "symlink target outside hicolor tree must never be touched"
        );
    }

    #[test]
    fn migrate_is_a_no_op_when_user_icon_dir_missing() {
        // Fresh rpm install, no prior tar.gz flavor: user never had
        // ~/.local/share/icons/hicolor at all.
        let layout = Layout::new();
        // system/ intentionally populated - helper should not even look at
        // it because the user tree is absent.
        layout.write_png(&layout.system_icon_dir, 48, b"canonical");

        migrate_user_hicolor_icons(
            &layout.user_icon_dir,
            &layout.system_icon_dir,
            &layout.cache_dir,
            &[48],
        )
        .expect("missing user tree is not an error");

        assert!(
            layout.cache_dir.join(MARKER_FILENAME).exists(),
            "marker still written - a fresh install doesn't need the migration to run ever again"
        );
    }

    #[test]
    fn run_startup_migrations_is_a_no_op_for_non_system_package_installs() {
        // Sanity check: the public wrapper must bail before touching env
        // vars or paths when the install method isn't Dnf/Apt. We drive
        // every non-target variant that existed at the time of writing.
        run_startup_migrations(&InstallMethod::Unknown);
        run_startup_migrations(&InstallMethod::SystemPackage {
            manager: PackageManager::RpmOstree,
        });
        run_startup_migrations(&InstallMethod::SystemPackage {
            manager: PackageManager::Other,
        });
        // Succeeds by not panicking; side-effect-free by construction
        // (no `$HOME` lookup, no filesystem access).
    }

    // ── US-009: coexistence detection + marker short-circuit ─────────────

    #[test]
    fn detect_coexistence_reports_tar_gz_when_system_package_is_running_and_home_app_dir_exists() {
        // Running binary: /usr/bin/paneflow (Dnf system package).
        // Tar.gz residue: $HOME/.local/paneflow.app/bin/paneflow still on disk.
        let report = detect_coexistent_install_with_probes(
            &InstallMethod::SystemPackage {
                manager: PackageManager::Dnf,
            },
            Some(PathBuf::from("/home/alice")),
            // system_bin_exists is ignored for the SystemPackage arm, but
            // we still pass true so a refactor regression would fail loud.
            true,
            |_p| true, // tar.gz binary exists
        );
        assert_eq!(
            report,
            Some(CoexistenceReport {
                running_path: PathBuf::from("/usr/bin/paneflow"),
                other_path: PathBuf::from("/home/alice/.local/paneflow.app/bin/paneflow"),
                other_method_label: "tar.gz",
            })
        );
    }

    #[test]
    fn detect_coexistence_reports_system_package_when_tar_gz_is_running_and_usr_bin_exists() {
        // Running binary: $HOME/.local/paneflow.app/bin/paneflow (TarGz).
        // System-package residue: /usr/bin/paneflow also on disk.
        let app_dir = PathBuf::from("/home/bob/.local/paneflow.app");
        let report = detect_coexistent_install_with_probes(
            &InstallMethod::TarGz {
                app_dir: app_dir.clone(),
            },
            Some(PathBuf::from("/home/bob")),
            true, // /usr/bin/paneflow present
            |_p| false,
        );
        assert_eq!(
            report,
            Some(CoexistenceReport {
                running_path: app_dir.join("bin").join("paneflow"),
                other_path: PathBuf::from("/usr/bin/paneflow"),
                other_method_label: "system package",
            })
        );
    }

    #[test]
    fn detect_coexistence_returns_none_for_appimage_and_other_methods() {
        // Every non-target InstallMethod variant must return None - the
        // coexistence failure mode is Dnf/Apt ↔ TarGz, nothing else.
        let cases = [
            InstallMethod::Unknown,
            InstallMethod::AppImage {
                mount_point: PathBuf::from("/tmp/.mount_abc"),
                source_path: PathBuf::from("/home/u/Downloads/paneflow.AppImage"),
            },
            InstallMethod::AppBundle {
                bundle_path: PathBuf::from("/Applications/PaneFlow.app"),
            },
            InstallMethod::WindowsMsi {
                install_path: PathBuf::from("C:/Program Files/PaneFlow"),
            },
            InstallMethod::SystemPackage {
                manager: PackageManager::RpmOstree,
            },
            InstallMethod::SystemPackage {
                manager: PackageManager::Other,
            },
        ];
        for method in cases {
            // Pass `true` for every probe so a regression routing
            // coexistence detection into one of these arms would surface as
            // a Some() we don't expect.
            let report = detect_coexistent_install_with_probes(
                &method,
                Some(PathBuf::from("/home/u")),
                true,
                |_p| true,
            );
            assert_eq!(report, None, "expected None for {method:?}");
        }
    }

    #[test]
    fn detect_coexistence_returns_none_when_home_missing() {
        // Sandboxed / container runtime with $HOME unset - be conservative,
        // never report coexistence without a resolvable home dir.
        let report = detect_coexistent_install_with_probes(
            &InstallMethod::SystemPackage {
                manager: PackageManager::Apt,
            },
            None,
            true,
            |_p| true,
        );
        assert_eq!(report, None);
    }

    #[test]
    fn coexistence_toast_marker_short_circuits_push_on_second_call() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let marker = tmp.path().join(COEXISTENCE_MARKER_FILENAME);

        // First call: marker absent, coexistence detected → Some.
        let first = coexistence_should_warn_with_paths(
            &InstallMethod::SystemPackage {
                manager: PackageManager::Dnf,
            },
            Some(PathBuf::from("/home/alice")),
            true,
            |_p| true,
            &marker,
        );
        assert!(first.is_some(), "first call must surface the toast");

        // Simulate the bootstrap writing the marker after showing the toast.
        std::fs::write(&marker, b"prior run").expect("write marker");

        // Second call: same inputs, marker now present → muted.
        let second = coexistence_should_warn_with_paths(
            &InstallMethod::SystemPackage {
                manager: PackageManager::Dnf,
            },
            Some(PathBuf::from("/home/alice")),
            true,
            |_p| true,
            &marker,
        );
        assert_eq!(
            second, None,
            "marker must short-circuit the toast on second call"
        );
    }

    #[test]
    fn coexistence_marker_path_uses_versioned_filename_under_cache_paneflow() {
        // Pin the marker-path layout so US-010 / future migrations can
        // rely on the `~/.cache/paneflow/migration-v<version>-*` convention.
        let home = Path::new("/home/carol");
        assert_eq!(
            coexistence_marker_path(home),
            PathBuf::from("/home/carol/.cache/paneflow/migration-v0.2.3-coexistence-warned"),
        );
    }

    #[test]
    fn write_coexistence_marker_creates_parent_directory() {
        // `~/.cache/paneflow/` may not exist on a very fresh install - the
        // helper must create it rather than silently fail.
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let marker = tmp
            .path()
            .join(".cache")
            .join("paneflow")
            .join(COEXISTENCE_MARKER_FILENAME);
        assert!(!marker.exists());

        write_coexistence_marker(&marker);

        assert!(marker.exists(), "marker must be written");
        assert!(
            marker.parent().map(Path::exists).unwrap_or(false),
            "parent cache dir must be created"
        );
    }
}
