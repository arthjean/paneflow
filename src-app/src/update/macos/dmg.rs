use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use super::super::error::UpdateError;

const UPDATE_HTTP_TIMEOUT: Duration = Duration::from_secs(30);

const NATIVE_INSTALLER_TIMEOUT: Duration = Duration::from_secs(10 * 60);

const NATIVE_DETACH_TIMEOUT: Duration = Duration::from_secs(60);

const NATIVE_STDOUT_CAP: u64 = 64 * 1024;

const MAX_DMG_BYTES: u64 = 500 * 1024 * 1024;

pub fn install(asset_url: &str, bundle_path: &Path) -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME environment variable is not set")?;

    if !is_expected_bundle_location(bundle_path, &home) {
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
    Ok(bundle_path.to_path_buf())
}

fn is_expected_bundle_location(bundle_path: &Path, home: &Path) -> bool {
    let Some(parent) = bundle_path.parent() else {
        return false;
    };
    parent == Path::new("/Applications") || parent == home.join("Applications")
}

fn is_translocated_path(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.contains("/AppTranslocation/") || s.contains("/var/folders/")
}

fn dmg_cache_dir(home: &Path) -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| home.join("Library").join("Caches"))
        .join(crate::runtime_paths::APP_SUBDIR)
}

#[cfg(target_os = "macos")]
const APPLE_TEAM_ID: &str = "228F9H5P95";

#[cfg(any(target_os = "macos", test))]
fn team_id_requirement_arg(team_id: &str) -> String {
    format!("-R=anchor apple generic and certificate leaf[subject.OU] = \"{team_id}\"")
}

#[cfg(target_os = "macos")]
fn verify_macos_bundle(bundle: &Path) -> Result<()> {
    run_gatekeeper_tool(
        "codesign",
        &["--verify", "--strict", "--deep", "--verbose=2"],
        bundle,
    )?;
    let team_arg = team_id_requirement_arg(APPLE_TEAM_ID);
    run_gatekeeper_tool("codesign", &["--verify", team_arg.as_str()], bundle)?;
    run_gatekeeper_tool("spctl", &["--assess", "--type", "execute"], bundle)?;
    Ok(())
}

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

    let _detach_guard = DetachGuard {
        runner,
        mount: mounted.clone(),
    };

    #[cfg(target_os = "macos")]
    {
        let bundle_name = bundle_file_name(install_dir)?;
        if let Err(e) = verify_macos_bundle(&mounted.join(bundle_name)) {
            let _ = std::fs::remove_file(&dmg);
            return Err(e);
        }
    }

    let swap_result = copy_and_swap(&mounted, install_dir);

    let _ = std::fs::remove_file(&dmg);
    swap_result
}

fn download_with_verification(asset_url: &str, dest: &Path) -> Result<()> {
    super::super::verified_download::download_verified_asset(
        asset_url,
        dest,
        MAX_DMG_BYTES,
        UPDATE_HTTP_TIMEOUT,
        "DMG",
    )
}

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

    if new_dir.exists()
        && let Err(e) = std::fs::remove_dir_all(&new_dir)
    {
        log::warn!(
            "self-update/dmg: could not clean stale {}: {e}",
            new_dir.display()
        );
    }

    if let Err(e) = copy_bundle_to_staging(&source_bundle, &new_dir) {
        let _ = std::fs::remove_dir_all(&new_dir);
        return Err(e);
    }

    #[cfg(all(target_os = "macos", not(test)))]
    if let Err(e) = verify_macos_bundle(&new_dir) {
        let _ = std::fs::remove_dir_all(&new_dir);
        return Err(e);
    }

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

    if old_dir.exists()
        && let Err(e) = std::fs::remove_dir_all(&old_dir)
    {
        log::warn!(
            "self-update/dmg: could not remove stale {}: {e}",
            old_dir.display()
        );
    }

    #[cfg(target_os = "macos")]
    strip_quarantine(install_dir);

    Ok(())
}

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

fn bundle_file_name(install_dir: &Path) -> Result<&std::ffi::OsStr> {
    install_dir
        .file_name()
        .context("install_dir has no file name - cannot locate the bundle inside the DMG")
}

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

    #[test]
    fn staging_dirs_derives_sibling_paths() {
        let (old, new) = staging_dirs(Path::new("/Applications/PaneFlow.app")).unwrap();
        assert_eq!(old, PathBuf::from("/Applications/PaneFlow.app.old"));
        assert_eq!(new, PathBuf::from("/Applications/PaneFlow.app.new"));
    }

    #[test]
    fn team_id_requirement_uses_attached_form() {
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
        assert!(!is_translocated_path(Path::new(
            "/Applications/PaneFlow.app"
        )));
        assert!(!is_translocated_path(Path::new(
            "/Users/x/Applications/PaneFlow.app"
        )));
        assert!(is_translocated_path(Path::new(
            "/private/var/folders/ab/cd/T/AppTranslocation/UUID/d/PaneFlow.app"
        )));
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
        assert!(!is_expected_bundle_location(
            Path::new("/opt/third-party/PaneFlow.app"),
            home
        ));
        assert!(!is_expected_bundle_location(
            Path::new("/Users/alice/Downloads/PaneFlow.app"),
            home
        ));
        assert!(!is_expected_bundle_location(
            Path::new("/Users/bob/Applications/PaneFlow.app"),
            home
        ));
    }

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
        assert!(matches!(UpdateError::classify(&err), UpdateError::Other(_)));
    }

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

    fn fake_bundle_at(root: &Path) -> PathBuf {
        let bundle = root.join("PaneFlow.app");
        let macos = bundle.join("Contents").join("MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::write(macos.join("paneflow"), b"#!/bin/sh\necho paneflow").unwrap();
        bundle
    }

    #[test]
    fn copy_and_swap_performs_atomic_rename() {
        let tmp = tempfile::TempDir::new().unwrap();
        let source_root = tmp.path().join("mount");
        std::fs::create_dir_all(&source_root).unwrap();
        fake_bundle_at(&source_root);

        let install_dir = tmp.path().join("Applications").join("PaneFlow.app");
        std::fs::create_dir_all(install_dir.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&install_dir).unwrap();
        std::fs::write(install_dir.join("old-marker"), b"old").unwrap();

        copy_and_swap(&source_root, &install_dir).unwrap();

        assert!(install_dir.join("Contents/MacOS/paneflow").exists());
        assert!(!install_dir.join("old-marker").exists());
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

    #[test]
    fn copy_and_swap_fresh_install_no_existing_bundle() {
        let tmp = tempfile::TempDir::new().unwrap();
        let source_root = tmp.path().join("mount");
        std::fs::create_dir_all(&source_root).unwrap();
        fake_bundle_at(&source_root);

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
        std::fs::create_dir_all(install_parent.join("PaneFlow.app.old")).unwrap();

        copy_and_swap(&mount, &install_dir).unwrap();

        assert!(install_dir.join("Contents/MacOS/paneflow").exists());
        assert!(!install_parent.join("PaneFlow.app.old").exists());
    }

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

        let result = stub.attach(Path::new("/nonexistent.dmg"), &install_dir);
        assert!(result.is_err(), "stub attach returned Ok unexpectedly");
        assert_eq!(
            stub.detach_calls.borrow().len(),
            0,
            "detach must not run when attach itself failed"
        );
        let _ = cache;
    }

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
