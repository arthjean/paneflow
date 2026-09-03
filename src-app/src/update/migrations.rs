#![cfg(target_os = "linux")]

use std::io::{self, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::update::install_method::{InstallMethod, PackageManager};

const HICOLOR_SIZES: &[u32] = &[16, 32, 48, 128, 256, 512];

const MARKER_FILENAME: &str = "migration-v0.2.3-icons-cleaned";

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

fn migrate_user_hicolor_icons(
    user_icon_dir: &Path,
    system_icon_dir: &Path,
    cache_dir: &Path,
    sizes: &[u32],
) -> io::Result<()> {
    let marker_path = cache_dir.join(MARKER_FILENAME);
    if marker_path.exists() {
        return Ok(());
    }

    match user_icon_dir.try_exists() {
        Ok(false) => {
            write_marker(cache_dir, &marker_path);
            return Ok(());
        }
        Ok(true) => {}
        Err(e) => return Err(e),
    }

    let mut deleted_any = false;
    let mut remove_failed: Option<io::Error> = None;

    for &size in sizes {
        let rel = format!("{size}x{size}/apps/paneflow.png");
        let user_file = user_icon_dir.join(&rel);
        let system_file = system_icon_dir.join(&rel);

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
                log::warn!(
                    "paneflow: system icon {} unreadable ({err}); preserving {}",
                    system_file.display(),
                    user_file.display()
                );
                continue;
            }
        };

        if user_hash == system_hash {
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

    if let Some(err) = remove_failed {
        return Err(err);
    }
    write_marker(cache_dir, &marker_path);
    Ok(())
}

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

pub const COEXISTENCE_MARKER_FILENAME: &str = "migration-v0.2.3-coexistence-warned";

const SYSTEM_BIN_PATH: &str = "/usr/bin/paneflow";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoexistenceReport {
    pub running_path: PathBuf,
    pub other_path: PathBuf,
    pub other_method_label: &'static str,
}

pub fn detect_coexistent_install(current: &InstallMethod) -> Option<CoexistenceReport> {
    detect_coexistent_install_with_probes(
        current,
        std::env::var_os("HOME").map(PathBuf::from),
        Path::new(SYSTEM_BIN_PATH).exists(),
        |p: &Path| p.exists(),
    )
}

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

pub fn coexistence_marker_path(home: &Path) -> PathBuf {
    home.join(".cache")
        .join("paneflow")
        .join(COEXISTENCE_MARKER_FILENAME)
}

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
        assert!(layout.cache_dir.join(MARKER_FILENAME).exists());
    }

    #[test]
    fn migrate_refuses_to_follow_user_icon_symlinks() {
        use std::os::unix::fs::symlink;
        let layout = Layout::new();
        layout.write_png(&layout.system_icon_dir, 48, b"canonical");

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
        let layout = Layout::new();
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
        run_startup_migrations(&InstallMethod::Unknown);
        run_startup_migrations(&InstallMethod::SystemPackage {
            manager: PackageManager::RpmOstree,
        });
        run_startup_migrations(&InstallMethod::SystemPackage {
            manager: PackageManager::Other,
        });
    }

    #[test]
    fn detect_coexistence_reports_tar_gz_when_system_package_is_running_and_home_app_dir_exists() {
        let report = detect_coexistent_install_with_probes(
            &InstallMethod::SystemPackage {
                manager: PackageManager::Dnf,
            },
            Some(PathBuf::from("/home/alice")),
            true,
            |_p| true,
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
        let app_dir = PathBuf::from("/home/bob/.local/paneflow.app");
        let report = detect_coexistent_install_with_probes(
            &InstallMethod::TarGz {
                app_dir: app_dir.clone(),
            },
            Some(PathBuf::from("/home/bob")),
            true,
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

        std::fs::write(&marker, b"prior run").expect("write marker");

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
        let home = Path::new("/home/carol");
        assert_eq!(
            coexistence_marker_path(home),
            PathBuf::from("/home/carol/.cache/paneflow/migration-v0.2.3-coexistence-warned"),
        );
    }

    #[test]
    fn write_coexistence_marker_creates_parent_directory() {
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
