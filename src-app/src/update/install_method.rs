#![allow(dead_code)]

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageManager {
    Apt,
    Dnf,
    Zypper,
    RpmOstree,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallMethod {
    SystemPackage {
        manager: PackageManager,
    },

    AppImage {
        mount_point: PathBuf,
        source_path: PathBuf,
    },

    TarGz {
        app_dir: PathBuf,
    },

    AppBundle {
        bundle_path: PathBuf,
    },

    WindowsMsi {
        install_path: PathBuf,
    },

    ExternallyManaged {
        explanation: String,
    },

    Unknown,
}

pub fn detect() -> InstallMethod {
    #[cfg(debug_assertions)]
    if let Ok(force) = std::env::var("PANEFLOW_DEV_INSTALL_METHOD") {
        match force.trim().to_ascii_lowercase().as_str() {
            "dnf" => {
                return InstallMethod::SystemPackage {
                    manager: PackageManager::Dnf,
                };
            }
            "apt" => {
                return InstallMethod::SystemPackage {
                    manager: PackageManager::Apt,
                };
            }
            "zypper" | "opensuse" => {
                return InstallMethod::SystemPackage {
                    manager: PackageManager::Zypper,
                };
            }
            "rpm-ostree" | "ostree" => {
                return InstallMethod::SystemPackage {
                    manager: PackageManager::RpmOstree,
                };
            }
            _ => {}
        }
    }

    if let Some(externally_managed) = detect_externally_managed(
        std::env::var_os("PANEFLOW_UPDATE_EXPLANATION"),
        option_env!("PANEFLOW_UPDATE_EXPLANATION"),
        std::env::var_os("FLATPAK_ID"),
        std::env::var_os("SNAP"),
    ) {
        return externally_managed;
    }

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return InstallMethod::Unknown,
    };
    let canonical =
        crate::runtime_paths::strip_verbatim_prefix(std::fs::canonicalize(&exe).unwrap_or(exe));

    #[cfg(target_os = "windows")]
    let (program_files, local_app_data): (Option<OsString>, Option<OsString>) =
        (std::env::var_os("ProgramFiles"), None);
    #[cfg(not(target_os = "windows"))]
    let (program_files, local_app_data): (Option<OsString>, Option<OsString>) = (None, None);

    let result = classify(
        &canonical,
        std::env::var_os("HOME"),
        std::env::var_os("APPIMAGE"),
        program_files,
        local_app_data,
    );

    #[cfg(target_os = "macos")]
    if matches!(result, InstallMethod::Unknown) {
        let msg = format!(
            "paneflow: running binary at {} is not inside a .app bundle - in-app updates disabled",
            canonical.display()
        );
        if cfg!(debug_assertions) {
            log::debug!("{msg}");
        } else {
            log::warn!("{msg}");
        }
    }

    result
}

fn detect_externally_managed(
    runtime_explanation: Option<OsString>,
    build_explanation: Option<&str>,
    flatpak_id: Option<OsString>,
    snap: Option<OsString>,
) -> Option<InstallMethod> {
    if let Some(value) = runtime_explanation
        && let Some(text) = value.to_str()
        && !text.trim().is_empty()
    {
        return Some(InstallMethod::ExternallyManaged {
            explanation: text.trim().to_string(),
        });
    }
    if let Some(text) = build_explanation
        && !text.trim().is_empty()
    {
        return Some(InstallMethod::ExternallyManaged {
            explanation: text.trim().to_string(),
        });
    }
    if let Some(value) = flatpak_id
        && let Some(id) = value.to_str()
        && !id.trim().is_empty()
    {
        return Some(InstallMethod::ExternallyManaged {
            explanation: format!(
                "PaneFlow is installed as a Flatpak. Run `flatpak update {}` to upgrade.",
                id.trim()
            ),
        });
    }
    if snap.is_some() {
        return Some(InstallMethod::ExternallyManaged {
            explanation:
                "PaneFlow is installed as a Snap. Run `sudo snap refresh paneflow` to upgrade."
                    .to_string(),
        });
    }
    None
}

fn classify(
    canonical: &Path,
    home: Option<OsString>,
    appimage: Option<OsString>,
    program_files: Option<OsString>,
    local_app_data: Option<OsString>,
) -> InstallMethod {
    if let Some(bundle_path) = app_bundle_path(canonical) {
        return InstallMethod::AppBundle { bundle_path };
    }

    if let Some(install_path) = windows_msi_install_path(
        canonical,
        program_files.as_deref(),
        local_app_data.as_deref(),
    ) {
        return InstallMethod::WindowsMsi { install_path };
    }

    if canonical == Path::new("/usr/bin/paneflow")
        || canonical == Path::new("/usr/local/bin/paneflow")
    {
        return InstallMethod::SystemPackage {
            manager: detect_package_manager(),
        };
    }

    let appimage_source = appimage
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty());
    let mount_point = appimage_mount_point(canonical);
    if appimage_source.is_some() || mount_point.is_some() {
        return InstallMethod::AppImage {
            mount_point: mount_point
                .or_else(|| canonical.parent().map(Path::to_path_buf))
                .unwrap_or_default(),
            source_path: appimage_source.unwrap_or_default(),
        };
    }

    if let Some(home_path) = home.map(PathBuf::from) {
        let app_dir = home_path.join(".local").join("paneflow.app");
        if canonical.starts_with(&app_dir) {
            return InstallMethod::TarGz { app_dir };
        }
    }

    InstallMethod::Unknown
}

fn windows_msi_install_path(
    canonical: &Path,
    program_files: Option<&std::ffi::OsStr>,
    _local_app_data: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    program_files
        .map(|p| PathBuf::from(p).join("PaneFlow"))
        .filter(|candidate| canonical.starts_with(candidate))
}

fn detect_package_manager() -> PackageManager {
    detect_package_manager_with_probes(
        Path::new("/etc/debian_version").exists(),
        Path::new("/etc/fedora-release").exists(),
        Path::new("/etc/SuSE-release").exists()
            || Path::new("/etc/zypp").exists()
            || os_release_id_like_suse(Path::new("/etc/os-release")),
        Path::new("/run/ostree-booted").exists(),
    )
}

fn os_release_id_like_suse(path: &Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return false;
    };
    contents.lines().any(|line| {
        let Some((key, value)) = line.split_once('=') else {
            return false;
        };
        if key != "ID" && key != "ID_LIKE" {
            return false;
        }
        value
            .trim_matches('"')
            .split_whitespace()
            .any(|token| matches!(token, "opensuse" | "suse" | "sles"))
    })
}

fn detect_package_manager_with_probes(
    debian_marker: bool,
    fedora_marker: bool,
    suse_marker: bool,
    ostree_booted: bool,
) -> PackageManager {
    if debian_marker && ostree_booted {
        return PackageManager::Other;
    }
    if debian_marker {
        return PackageManager::Apt;
    }
    if ostree_booted {
        return PackageManager::RpmOstree;
    }
    if fedora_marker {
        return PackageManager::Dnf;
    }
    if suse_marker {
        return PackageManager::Zypper;
    }
    PackageManager::Other
}

fn app_bundle_path(path: &Path) -> Option<PathBuf> {
    let macos_dir = path.parent()?;
    if macos_dir.file_name()?.to_str()? != "MacOS" {
        return None;
    }
    let contents_dir = macos_dir.parent()?;
    if contents_dir.file_name()?.to_str()? != "Contents" {
        return None;
    }
    let bundle = contents_dir.parent()?;
    let bundle_name = bundle.file_name()?.to_str()?;
    if !bundle_name.ends_with(".app") || bundle_name == ".app" {
        return None;
    }
    Some(bundle.to_path_buf())
}

fn appimage_mount_point(path: &Path) -> Option<PathBuf> {
    let mut comps = path.components();
    if !matches!(comps.next()?, Component::RootDir) {
        return None;
    }
    if comps.next()?.as_os_str() != "tmp" {
        return None;
    }
    let mount = comps.next()?;
    let mount_str = mount.as_os_str().to_str()?;
    if !mount_str.starts_with(".mount_") {
        return None;
    }
    comps.next()?;

    Some(Path::new("/tmp").join(mount_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_package_usr_bin() {
        let r = classify(Path::new("/usr/bin/paneflow"), None, None, None, None);
        assert!(matches!(r, InstallMethod::SystemPackage { .. }));
    }

    #[test]
    fn system_package_usr_local_bin() {
        let r = classify(Path::new("/usr/local/bin/paneflow"), None, None, None, None);
        assert!(matches!(r, InstallMethod::SystemPackage { .. }));
    }

    #[test]
    fn appimage_with_env() {
        let r = classify(
            Path::new("/tmp/.mount_abc123/usr/bin/paneflow"),
            None,
            Some(OsString::from("/home/u/Downloads/paneflow.AppImage")),
            None,
            None,
        );
        match r {
            InstallMethod::AppImage {
                mount_point,
                source_path,
            } => {
                assert_eq!(mount_point, Path::new("/tmp/.mount_abc123"));
                assert_eq!(
                    source_path,
                    Path::new("/home/u/Downloads/paneflow.AppImage")
                );
            }
            other => panic!("expected AppImage, got {other:?}"),
        }
    }

    #[test]
    fn appimage_without_env_still_detected() {
        let r = classify(
            Path::new("/tmp/.mount_abc123/usr/bin/paneflow"),
            None,
            None,
            None,
            None,
        );
        match r {
            InstallMethod::AppImage {
                mount_point,
                source_path,
            } => {
                assert_eq!(mount_point, Path::new("/tmp/.mount_abc123"));
                assert_eq!(source_path, PathBuf::new());
            }
            other => panic!("expected AppImage, got {other:?}"),
        }
    }

    #[test]
    fn tar_gz_under_home_app_dir() {
        let r = classify(
            Path::new("/home/u/.local/paneflow.app/bin/paneflow"),
            Some(OsString::from("/home/u")),
            None,
            None,
            None,
        );
        match r {
            InstallMethod::TarGz { app_dir } => {
                assert_eq!(app_dir, Path::new("/home/u/.local/paneflow.app"));
            }
            other => panic!("expected TarGz, got {other:?}"),
        }
    }

    #[test]
    fn unknown_for_legacy_run_install() {
        let r = classify(
            Path::new("/home/u/.local/bin/paneflow"),
            Some(OsString::from("/home/u")),
            None,
            None,
            None,
        );
        assert_eq!(r, InstallMethod::Unknown);
    }

    #[test]
    fn unknown_for_random_path() {
        let r = classify(
            Path::new("/opt/random/paneflow"),
            Some(OsString::from("/home/u")),
            None,
            None,
            None,
        );
        assert_eq!(r, InstallMethod::Unknown);
    }

    #[test]
    fn app_bundle_in_slash_applications() {
        let r = classify(
            Path::new("/Applications/PaneFlow.app/Contents/MacOS/paneflow"),
            Some(OsString::from("/Users/alice")),
            None,
            None,
            None,
        );
        match r {
            InstallMethod::AppBundle { bundle_path } => {
                assert_eq!(bundle_path, Path::new("/Applications/PaneFlow.app"));
            }
            other => panic!("expected AppBundle, got {other:?}"),
        }
    }

    #[test]
    fn app_bundle_in_home_applications() {
        let r = classify(
            Path::new("/Users/alice/Applications/PaneFlow.app/Contents/MacOS/paneflow"),
            Some(OsString::from("/Users/alice")),
            None,
            None,
            None,
        );
        match r {
            InstallMethod::AppBundle { bundle_path } => {
                assert_eq!(
                    bundle_path,
                    Path::new("/Users/alice/Applications/PaneFlow.app")
                );
            }
            other => panic!("expected AppBundle, got {other:?}"),
        }
    }

    #[test]
    fn app_bundle_at_arbitrary_drag_install_location() {
        let r = classify(
            Path::new("/opt/third-party/PaneFlow.app/Contents/MacOS/paneflow"),
            None,
            None,
            None,
            None,
        );
        assert!(matches!(r, InstallMethod::AppBundle { .. }));
    }

    #[test]
    fn macos_binary_outside_bundle_is_unknown() {
        let r = classify(
            Path::new("/Users/alice/bin/paneflow"),
            Some(OsString::from("/Users/alice")),
            None,
            None,
            None,
        );
        assert_eq!(r, InstallMethod::Unknown);
    }

    #[test]
    fn app_bundle_parser_rejects_wrong_layout() {
        assert!(
            app_bundle_path(Path::new(
                "/Applications/PaneFlow.app/Contents/bin/paneflow"
            ))
            .is_none()
        );
        assert!(
            app_bundle_path(Path::new(
                "/Applications/PaneFlow.app/Payload/MacOS/paneflow"
            ))
            .is_none()
        );
        assert!(
            app_bundle_path(Path::new("/Applications/PaneFlow/Contents/MacOS/paneflow")).is_none()
        );
        assert!(app_bundle_path(Path::new("/Applications/.app/Contents/MacOS/paneflow")).is_none());
        assert!(app_bundle_path(Path::new("/paneflow")).is_none());
    }

    #[test]
    fn appimage_mount_parsing() {
        assert_eq!(
            appimage_mount_point(Path::new("/tmp/.mount_abc/usr/bin/paneflow")),
            Some(PathBuf::from("/tmp/.mount_abc"))
        );
        assert!(appimage_mount_point(Path::new("/var/.mount_abc/paneflow")).is_none());
        assert!(appimage_mount_point(Path::new("/tmp/foo/paneflow")).is_none());
        assert!(appimage_mount_point(Path::new("/tmp/.mount_x")).is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn canonicalize_resolves_tar_gz_symlink() {
        let tmp = tempfile::TempDir::new().unwrap();
        let app_dir = tmp.path().join(".local/paneflow.app/bin");
        std::fs::create_dir_all(&app_dir).unwrap();
        let real_bin = app_dir.join("paneflow");
        std::fs::write(&real_bin, b"").unwrap();

        let bin_dir = tmp.path().join(".local/bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let sym = bin_dir.join("paneflow");
        std::os::unix::fs::symlink(&real_bin, &sym).unwrap();

        let canonical = std::fs::canonicalize(&sym).unwrap();
        let r = classify(
            &canonical,
            Some(OsString::from(tmp.path())),
            None,
            None,
            None,
        );
        match r {
            InstallMethod::TarGz { app_dir } => {
                assert_eq!(app_dir, tmp.path().join(".local/paneflow.app"));
            }
            other => panic!("expected TarGz, got {other:?}"),
        }
    }

    #[test]
    fn windows_msi_machine_wide_program_files() {
        let r = classify(
            Path::new("C:/Program Files/PaneFlow/paneflow.exe"),
            None,
            None,
            Some(OsString::from("C:/Program Files")),
            Some(OsString::from("C:/Users/alice/AppData/Local")),
        );
        match r {
            InstallMethod::WindowsMsi { install_path } => {
                assert_eq!(install_path, PathBuf::from("C:/Program Files/PaneFlow"));
            }
            other => panic!("expected WindowsMsi, got {other:?}"),
        }
    }

    #[test]
    fn windows_local_app_data_is_unknown_until_per_user_msi_ships() {
        let r = classify(
            Path::new("C:/Users/alice/AppData/Local/Programs/PaneFlow/paneflow.exe"),
            None,
            None,
            Some(OsString::from("C:/Program Files")),
            Some(OsString::from("C:/Users/alice/AppData/Local")),
        );
        assert_eq!(r, InstallMethod::Unknown);
    }

    #[test]
    fn windows_binary_outside_standard_paths_is_unknown() {
        let r = classify(
            Path::new("C:/dev/paneflow/target/release/paneflow.exe"),
            None,
            None,
            Some(OsString::from("C:/Program Files")),
            Some(OsString::from("C:/Users/alice/AppData/Local")),
        );
        assert_eq!(r, InstallMethod::Unknown);
    }

    #[test]
    fn windows_msi_detected_after_stripping_verbatim_canonical_prefix() {
        let canonical = crate::runtime_paths::strip_verbatim_prefix(PathBuf::from(
            r"\\?\C:/Program Files/PaneFlow/paneflow.exe",
        ));
        let r = classify(
            &canonical,
            None,
            None,
            Some(OsString::from("C:/Program Files")),
            Some(OsString::from("C:/Users/alice/AppData/Local")),
        );
        assert!(matches!(r, InstallMethod::WindowsMsi { .. }), "got {r:?}");
    }

    #[test]
    fn windows_msi_detection_ignored_when_env_vars_missing() {
        let r = classify(
            Path::new("C:/Program Files/PaneFlow/paneflow.exe"),
            None,
            None,
            None,
            None,
        );
        assert_eq!(r, InstallMethod::Unknown);
    }

    #[test]
    fn detect_package_manager_debian_marker_wins() {
        assert_eq!(
            detect_package_manager_with_probes(true, false, false, false),
            PackageManager::Apt
        );
    }

    #[test]
    fn detect_package_manager_fedora_marker_returns_dnf() {
        assert_eq!(
            detect_package_manager_with_probes(false, true, false, false),
            PackageManager::Dnf
        );
    }

    #[test]
    fn detect_package_manager_suse_marker_returns_zypper() {
        assert_eq!(
            detect_package_manager_with_probes(false, false, true, false),
            PackageManager::Zypper
        );
    }

    #[test]
    fn os_release_id_like_suse_matches_id_and_id_like() {
        let tmp = tempfile::TempDir::new().unwrap();
        let file = tmp.path().join("os-release");
        std::fs::write(&file, "ID=opensuse-tumbleweed\nID_LIKE=\"suse rhel\"\n").unwrap();
        assert!(os_release_id_like_suse(&file));
    }

    #[test]
    fn classify_system_package_detects_rpm_ostree_via_ostree_booted_marker() {
        assert_eq!(
            detect_package_manager_with_probes(false, true, false, true),
            PackageManager::RpmOstree
        );
    }

    #[test]
    fn detect_package_manager_ostree_without_fedora_marker_still_rpm_ostree() {
        assert_eq!(
            detect_package_manager_with_probes(false, false, false, true),
            PackageManager::RpmOstree
        );
    }

    #[test]
    fn detect_package_manager_no_markers_returns_other() {
        assert_eq!(
            detect_package_manager_with_probes(false, false, false, false),
            PackageManager::Other
        );
    }

    #[test]
    fn detect_package_manager_debian_plus_ostree_is_externally_managed() {
        assert_eq!(
            detect_package_manager_with_probes(true, false, false, true),
            PackageManager::Other
        );
        assert_eq!(
            detect_package_manager_with_probes(true, false, false, false),
            PackageManager::Apt
        );
        assert_eq!(
            detect_package_manager_with_probes(false, true, false, true),
            PackageManager::RpmOstree
        );
    }
}
