//! Runtime detection of how PaneFlow was installed.
//!
//! The in-app updater needs to pick different strategies depending on whether
//! the running binary came from a distro package, an AppImage, a user-local
//! tar.gz install, or an unknown location. We determine this from the binary
//! path alone - no config, no env var (except `$APPIMAGE`, which the AppImage
//! runtime already sets for us).
//!
//! Detection runs at startup. The caller canonicalises `current_exe()` before
//! classifying, so a symlink like `~/.local/bin/paneflow ->
//! ~/.local/paneflow.app/bin/paneflow` resolves to the real path and is
//! correctly identified as `TarGz`.
//!
//! Every public API in this module is consumed by the updater work in
//! US-009/010/011/012. Until those stories land, much of it is only
//! reachable through the unit tests - hence the crate-level dead-code
//! suppression.

#![allow(dead_code)]

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// Package manager used for system-wide installs. Advisory only - the updater
/// uses this to pick the correct in-app update strategy (pkexec dnf/apt) or
/// UI hint (generic clipboard-copy / rpm-ostree informational toast).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageManager {
    Apt,
    Dnf,
    Zypper,
    /// Immutable Fedora variants (Silverblue, Kinoite, Bazzite). Detected
    /// via `/run/ostree-booted` - these systems have `/etc/fedora-release`
    /// too, so the ostree probe MUST run before the Dnf probe. `dnf`
    /// cannot mutate the read-only `/usr`; updates must go through
    /// `rpm-ostree upgrade` which stages a new deployment for next boot.
    /// US-004 only surfaces an informational toast + clipboard copy; a
    /// full in-place `pkexec rpm-ostree install …` flow is deferred.
    RpmOstree,
    /// `/usr/bin/paneflow` exists but no supported package-manager marker is
    /// present (e.g., `eopkg` on Solus, `xbps` on Void). The UI falls back to
    /// a generic "via your package manager" hint.
    Other,
}

/// How the running binary was installed on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallMethod {
    /// `/usr/bin/paneflow` or `/usr/local/bin/paneflow` - the apt/dnf managed
    /// binary. In-app updates are disabled; user is pointed at the system
    /// package manager.
    SystemPackage { manager: PackageManager },

    /// Launched from a mounted AppImage (`/tmp/.mount_*/...`). Update flow
    /// delegates to `appimageupdatetool` on the source `.AppImage` file.
    AppImage {
        mount_point: PathBuf,
        source_path: PathBuf,
    },

    /// Installed by the tar.gz installer under `$HOME/.local/paneflow.app/`.
    /// Update flow downloads a new tarball and atomically swaps the app dir.
    TarGz { app_dir: PathBuf },

    /// macOS `.app` bundle layout (US-007) - the running binary lives at
    /// `<bundle_path>/Contents/MacOS/paneflow`, whether under
    /// `/Applications`, `$HOME/Applications`, or anywhere the user dragged
    /// the bundle. The updater pairs this with `AssetFormat::Dmg`
    /// (US-008) to download a matching `.dmg`.
    AppBundle { bundle_path: PathBuf },

    /// Windows MSI install (US-010 - prd-windows-port.md). The shipping WiX
    /// installer is machine-wide, so the running `paneflow.exe` must live under
    /// `%ProgramFiles%\PaneFlow\paneflow.exe`.
    ///
    /// `install_path` is the containing PaneFlow directory (not the exe).
    /// The updater pairs this with `AssetFormat::Msi` (US-011) to match
    /// the correct `.msi` release asset for x86_64 Windows.
    WindowsMsi { install_path: PathBuf },

    /// In-app updates are disabled by the host environment. Set when the
    /// process is sandboxed (Flatpak / Snap) or when the build / runtime
    /// environment carries `PANEFLOW_UPDATE_EXPLANATION`. The pill renders
    /// a system-managed hint and clicking copies the explanation copy
    /// rather than attempting any download.
    ///
    /// This mirrors Zed's `ZED_UPDATE_EXPLANATION` convention: distro and
    /// store packagers (Flatpak, Snap, Solus, NixOS, Fedora COPR, …) bake
    /// the env var into their wrapper / manifest at build time, and the
    /// in-app updater stays out of their way at runtime - the package
    /// manager is the only path to a new version.
    ExternallyManaged { explanation: String },

    /// Binary location doesn't match any known layout (legacy `.run` install,
    /// manual copy, dev build). Updater disables in-app updates.
    Unknown,
}

/// Probe the filesystem and environment to classify the running binary.
pub fn detect() -> InstallMethod {
    // Dev-only override: lets `cargo run` simulate any install-method
    // branch without having to reinstall over /usr/bin/paneflow. Only
    // compiled in debug builds - release binaries ignore it.
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

    // Sandboxed / packager-managed environments take priority over any
    // path-based heuristic. A Flatpak install of PaneFlow has its real
    // binary at `/app/bin/paneflow` (which would otherwise look like an
    // ad-hoc system install), and a Snap install lives in
    // `/snap/paneflow/current/bin/paneflow` - both are immutable and the
    // in-app updater would silently fail. We disable it up front so the
    // pill copies the right `flatpak update` / `snap refresh` command
    // instead of attempting a download. Mirrors Zed's
    // `ZED_UPDATE_EXPLANATION` convention (see `crates/auto_update`
    // and `crates/cli/src/main.rs::try_restart_to_host` in
    // /home/arthur/dev/zed).
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
    // Canonicalise resolves symlinks and `..` segments. If it fails (unlikely),
    // fall back to the raw exe path.
    //
    // On Windows, `canonicalize` returns the extended-length `\\?\C:\…` form
    // (rust-lang/rust#42869). `windows_msi_install_path` compares this against
    // the non-verbatim `%ProgramFiles%` env value, and
    // `Path::starts_with`'s leading component (`Prefix(VerbatimDisk)` vs
    // `Prefix(Disk)`) never matches - so every MSI install would fall through
    // to `Unknown` and the updater would wrongly take the Linux `$HOME` tar.gz
    // path. Strip the verbatim prefix so the comparison lines up.
    let canonical = strip_verbatim_prefix(std::fs::canonicalize(&exe).unwrap_or(exe));

    // US-039 - Windows MSI install detection. `ProgramFiles` is the only
    // supported root because the shipping WiX package is machine-wide.
    //
    // B.2 - type-gate the WindowsMsi arm to the Windows target: on non-Windows
    // we feed `None` regardless of any leaked `ProgramFiles`
    // (Wine, cross-build, CI), so a Linux binary can never be misclassified as
    // WindowsMsi. The pure `classify` keeps its logic so its tests still run on
    // Linux CI.
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

    // US-007 AC3 - on macOS, a binary that is NOT inside a .app bundle means
    // someone extracted paneflow ad-hoc (e.g. copied to ~/bin/). In-app
    // updates can't target such installs, so surface the reason once at
    // startup instead of silently showing a never-firing update prompt.
    #[cfg(target_os = "macos")]
    if matches!(result, InstallMethod::Unknown) {
        // In a debug build the binary lives under `target/debug/` and is never
        // inside a .app - this is the expected dev path, so log it at debug
        // level to avoid spamming a warning on every `cargo run`. A release
        // binary running outside a bundle is a genuine ad-hoc extraction worth
        // surfacing at warn level.
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

/// Pure detector for sandboxed / packager-managed environments. Returns
/// `Some(InstallMethod::ExternallyManaged)` when any of the inputs signals
/// that a third party owns this install:
///
/// - `runtime_explanation` - `PANEFLOW_UPDATE_EXPLANATION` env var read at
///   startup. Highest priority because it's the explicit opt-out a packager
///   set in the host wrapper / launcher.
/// - `build_explanation` - same env var captured at build time via
///   `option_env!("PANEFLOW_UPDATE_EXPLANATION")`. Distro packagers (Fedora
///   COPR, AUR, Solus, NixOS) bake this into their RPM/PKGBUILD/derivation.
/// - `flatpak_id` - `FLATPAK_ID` is set by `flatpak-spawn` when the binary
///   runs inside a Flatpak sandbox. Fixed copy: `flatpak update <id>`.
/// - `snap` - `SNAP` env var is set by snapd for Snap packages. Fixed
///   copy: `sudo snap refresh paneflow`.
///
/// Pure (no I/O, no FS reads) so the unit tests can mock all four signals.
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

/// Pure classifier - no I/O beyond the `/etc/*-release` probe for package
/// manager inference, which is only reached on the SystemPackage arm. All
/// other inputs are parameters so callers (and tests) control them.
fn classify(
    canonical: &Path,
    home: Option<OsString>,
    appimage: Option<OsString>,
    program_files: Option<OsString>,
    local_app_data: Option<OsString>,
) -> InstallMethod {
    // 0. macOS `.app` bundle (US-007). Structural check on the path
    //    components: `<bundle>/Contents/MacOS/<binary>`. Placed first
    //    because it's the cheapest and cannot false-positive on a Linux
    //    path (no Linux layout has `Contents/MacOS/` in the tail).
    if let Some(bundle_path) = app_bundle_path(canonical) {
        return InstallMethod::AppBundle { bundle_path };
    }

    // 0.5. Windows MSI install (US-010). Same no-false-positive reasoning
    //      as AppBundle: Linux/macOS never set `ProgramFiles`, so the helper
    //      returns None and this branch
    //      short-circuits on non-Windows. Cheap to keep ungated.
    if let Some(install_path) = windows_msi_install_path(
        canonical,
        program_files.as_deref(),
        local_app_data.as_deref(),
    ) {
        return InstallMethod::WindowsMsi { install_path };
    }

    // 1. System package (apt/dnf).
    if canonical == Path::new("/usr/bin/paneflow")
        || canonical == Path::new("/usr/local/bin/paneflow")
    {
        return InstallMethod::SystemPackage {
            manager: detect_package_manager(),
        };
    }

    // 2. AppImage. The type-2 runtime ALWAYS exports $APPIMAGE (the absolute
    //    path to the .AppImage), which is exactly what the updater needs, so a
    //    non-empty $APPIMAGE is the PRIMARY signal. The `/tmp/.mount_` path
    //    heuristic alone breaks when the runtime mounts under $TMPDIR instead of
    //    /tmp (NixOS, systemd `PrivateTmp=yes`, sandboxes), which silently
    //    dropped self-update to `Unknown`. The heuristic stays as a fallback for
    //    a non-standard launch that didn't set $APPIMAGE.
    let appimage_source = appimage
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty());
    let mount_point = appimage_mount_point(canonical);
    if appimage_source.is_some() || mount_point.is_some() {
        return InstallMethod::AppImage {
            // The updater only consumes `source_path`; `mount_point` is
            // diagnostic, so best-effort: the real `/tmp/.mount_` dir when the
            // heuristic matched, else the running binary's parent.
            mount_point: mount_point
                .or_else(|| canonical.parent().map(Path::to_path_buf))
                .unwrap_or_default(),
            source_path: appimage_source.unwrap_or_default(),
        };
    }

    // 3. Tar.gz install under $HOME/.local/paneflow.app/.
    if let Some(home_path) = home.map(PathBuf::from) {
        let app_dir = home_path.join(".local").join("paneflow.app");
        if canonical.starts_with(&app_dir) {
            return InstallMethod::TarGz { app_dir };
        }
    }

    InstallMethod::Unknown
}

/// Return the PaneFlow MSI install directory if `canonical` points at a binary
/// under the standard machine-wide Windows location:
/// `%ProgramFiles%\PaneFlow\` (US-010 - prd-windows-port.md).
///
/// Pure path manipulation - no FS access, no env-var reads. `ProgramFiles`
/// comes in as a parameter so tests can mock it on any host.
fn windows_msi_install_path(
    canonical: &Path,
    program_files: Option<&std::ffi::OsStr>,
    _local_app_data: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    program_files
        .map(|p| PathBuf::from(p).join("PaneFlow"))
        .filter(|candidate| canonical.starts_with(candidate))
}

/// Drop the Windows extended-length (`\\?\` or `\\?\UNC\`) prefix that
/// `std::fs::canonicalize` prepends (rust-lang/rust#42869). Without this, a
/// canonicalized `current_exe()` carries a `Prefix(VerbatimDisk)` component
/// that never compares equal to the `Prefix(Disk)` of the non-verbatim
/// `%ProgramFiles%` value in [`windows_msi_install_path`], so every MSI
/// install is misdetected as `Unknown` (and the updater wrongly tries the
/// Linux `$HOME` tar.gz path).
///
/// No-op for any path not in verbatim form - every Unix path, and any Windows
/// path that was never canonicalized - so it is safe to call on all targets.
/// Pure string logic, so the regression test runs on Linux CI.
fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    // Decide on a borrowed view, then move `path` only in the fall-through:
    // returning `path` from inside a `match path.to_str() { … }` arm would
    // conflict with the `to_str()` borrow.
    let stripped = path.to_str().and_then(|s| {
        s.strip_prefix(r"\\?\UNC\")
            // `\\?\UNC\server\share\…` → `\\server\share\…`
            .map(|rest| PathBuf::from(format!(r"\\{rest}")))
            // `\\?\C:\…` → `C:\…`
            .or_else(|| s.strip_prefix(r"\\?\").map(PathBuf::from))
    });
    stripped.unwrap_or(path)
}

/// Infer the system package manager from distro-identifier files.
///
/// Only reached when the binary is at `/usr/bin` or `/usr/local/bin` - i.e.
/// we already know a system package put it there. Returns `Other` when no
/// recognised marker is found so the UI can degrade to a generic hint
/// instead of pretending `apt` is available.
///
/// Precedence matters: Silverblue / Kinoite carry BOTH `/etc/fedora-release`
/// AND `/run/ostree-booted`. The ostree probe must run first, otherwise we
/// would route those users to a broken `dnf install` (US-004).
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

/// Pure, parameter-driven version of [`detect_package_manager`] so the
/// precedence logic is unit-testable without touching the real filesystem.
/// Each `bool` is the result of a `Path::exists()` probe the caller runs.
fn detect_package_manager_with_probes(
    debian_marker: bool,
    fedora_marker: bool,
    suse_marker: bool,
    ostree_booted: bool,
) -> PackageManager {
    // Endless OS is Debian-based but boots ostree (immutable rootfs), so it
    // carries BOTH `/etc/debian_version` AND `/run/ostree-booted`. An
    // `apt install` would fail on its read-only deployment, and it is NOT
    // rpm-ostree either - route it to the generic externally-managed hint
    // rather than the broken `apt` path.
    if debian_marker && ostree_booted {
        return PackageManager::Other;
    }
    // A plain Debian/Ubuntu (no ostree) uses apt.
    if debian_marker {
        return PackageManager::Apt;
    }
    // Ostree marker beats the Fedora marker - Silverblue has both.
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

/// Return the enclosing `.app` bundle path if `path` points at a binary
/// inside a macOS app bundle, else `None`. We check structurally - parent
/// must be `MacOS`, grandparent `Contents`, great-grandparent ends with
/// `.app` - so drag-installs to arbitrary locations (e.g. `~/Downloads/`)
/// are still detected, not just the canonical `/Applications` path.
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
    // `.app` is an extension; a directory literally named `.app` with no
    // prefix isn't a real bundle.
    if !bundle_name.ends_with(".app") || bundle_name == ".app" {
        return None;
    }
    Some(bundle.to_path_buf())
}

/// Return the `/tmp/.mount_XXXXXX/` directory if `path` lives inside a
/// mounted AppImage, else `None`. Matches the AppImage runtime's naming
/// convention.
fn appimage_mount_point(path: &Path) -> Option<PathBuf> {
    let mut comps = path.components();
    // `/` (root)
    if !matches!(comps.next()?, Component::RootDir) {
        return None;
    }
    // `tmp`
    if comps.next()?.as_os_str() != "tmp" {
        return None;
    }
    // `.mount_XXXXXX`
    let mount = comps.next()?;
    let mount_str = mount.as_os_str().to_str()?;
    if !mount_str.starts_with(".mount_") {
        return None;
    }
    // Require at least one component after the mount dir - `current_exe()`
    // always returns a file path, so a bare mount dir would be impossible,
    // but the guard makes the classifier resistant to malformed inputs.
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

    // ---- US-007 tests ----

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
        // Structural check matches any location, not just /Applications.
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
        // A user who extracted paneflow to ~/bin/ gets Unknown (AC3).
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
        // Wrong MacOS directory name
        assert!(
            app_bundle_path(Path::new(
                "/Applications/PaneFlow.app/Contents/bin/paneflow"
            ))
            .is_none()
        );
        // Wrong Contents directory name
        assert!(
            app_bundle_path(Path::new(
                "/Applications/PaneFlow.app/Payload/MacOS/paneflow"
            ))
            .is_none()
        );
        // Bundle dir not ending in .app
        assert!(
            app_bundle_path(Path::new("/Applications/PaneFlow/Contents/MacOS/paneflow")).is_none()
        );
        // Bundle dir named literally `.app` (edge case)
        assert!(app_bundle_path(Path::new("/Applications/.app/Contents/MacOS/paneflow")).is_none());
        // Missing parent entirely (root-level binary)
        assert!(app_bundle_path(Path::new("/paneflow")).is_none());
    }

    #[test]
    fn appimage_mount_parsing() {
        assert_eq!(
            appimage_mount_point(Path::new("/tmp/.mount_abc/usr/bin/paneflow")),
            Some(PathBuf::from("/tmp/.mount_abc"))
        );
        // Non-`/tmp` root
        assert!(appimage_mount_point(Path::new("/var/.mount_abc/paneflow")).is_none());
        // Non-`.mount_` prefix
        assert!(appimage_mount_point(Path::new("/tmp/foo/paneflow")).is_none());
        // Bare mount dir (no binary beneath)
        assert!(appimage_mount_point(Path::new("/tmp/.mount_x")).is_none());
    }

    /// Proves that `canonicalize` resolves a symlink chain mimicking the
    /// tar.gz install layout. Detection in `detect()` relies on this so the
    /// symlink at `~/.local/bin/paneflow` doesn't get misclassified.
    ///
    /// US-007 (prd-windows-port.md) - Unix-only. `TarGz` is a Linux/macOS
    /// install method; Windows uses `WindowsMsi` from US-010 which installs
    /// `paneflow.exe` directly to `%ProgramFiles%\PaneFlow\` with no
    /// symlink indirection. Creating symlinks on Windows also requires
    /// `SeCreateSymbolicLinkPrivilege`, which non-admin users lack by
    /// default. AC-6 of US-007 explicitly permits "self-update skips
    /// symlink creation on Windows entirely" - this codebase has no
    /// runtime symlink creators anywhere, so there is no `make_symlink`
    /// helper to document; the cfg-gate here IS the design choice.
    ///
    /// Linux-only: on macOS the `.local/paneflow.app` suffix collides with
    /// the `.app` bundle detector and the classifier returns `AppBundle`
    /// instead of `TarGz`. The tar.gz install layout is a Linux convention
    /// (Zed/Ghostty-style `$HOME/.local/paneflow.app/`) that doesn't exist
    /// on macOS, so the test has no meaning there.
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

    // ---- US-010 tests - Windows MSI install detection. ----
    //
    // Pure string/path manipulation; mocked env-var values fed directly to
    // `classify`. No Windows-only types, so
    // these run on Linux CI and prove the detection logic without having
    // to stand up a Windows runner for a unit test.
    //
    // Path literals intentionally use forward slashes. On Windows, `Path`
    // treats both `/` and `\` as separators, so the production code path
    // (which sees backslashes from `current_exe()` and `ProgramFiles`)
    // and the test path both resolve into the same component sequence.
    // On Linux, `Path::starts_with` is component-based and only honors
    // `/` as a separator - using backslashes here would collapse the
    // whole Windows path into a single component and break `starts_with`.

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
        // A dev build running from `target/release/paneflow.exe` - not
        // inside %ProgramFiles%\PaneFlow\.
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
    fn strip_verbatim_prefix_disk_unc_and_passthrough() {
        // `\\?\C:\…` (the form `std::fs::canonicalize` returns on Windows) → `C:\…`
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"\\?\C:\Program Files\PaneFlow\paneflow.exe")),
            PathBuf::from(r"C:\Program Files\PaneFlow\paneflow.exe")
        );
        // `\\?\UNC\server\share\…` → `\\server\share\…`
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"\\?\UNC\server\share\paneflow.exe")),
            PathBuf::from(r"\\server\share\paneflow.exe")
        );
        // Non-verbatim Windows path is left untouched.
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"C:\Program Files\PaneFlow\paneflow.exe")),
            PathBuf::from(r"C:\Program Files\PaneFlow\paneflow.exe")
        );
        // Unix path is left untouched.
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from("/usr/bin/paneflow")),
            PathBuf::from("/usr/bin/paneflow")
        );
    }

    #[test]
    fn windows_msi_detected_after_stripping_verbatim_canonical_prefix() {
        // Regression (US-039 follow-up): `std::fs::canonicalize` on Windows
        // yields the `\\?\` extended-length form, whose leading
        // `Prefix(VerbatimDisk)` component never matched the non-verbatim
        // `%ProgramFiles%` in `windows_msi_install_path` - so a real MSI
        // install fell through to `Unknown` and the updater tried the Linux
        // `$HOME` tar.gz path. `detect()` now strips the prefix first; this
        // proves the strip → classify pipeline lands on `WindowsMsi`.
        //
        // Forward slashes in the tail (after the backslash `\\?\` prefix) so
        // `Path::starts_with` is component-based on Linux CI too - same
        // dual-representation trick the other Windows tests use.
        let canonical =
            strip_verbatim_prefix(PathBuf::from(r"\\?\C:/Program Files/PaneFlow/paneflow.exe"));
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
        // Linux / macOS call site - `ProgramFiles` is None. Even if someone
        // crafts a path that looks like a Windows
        // install, the detection short-circuits (no candidate dirs to
        // test against).
        let r = classify(
            Path::new("C:/Program Files/PaneFlow/paneflow.exe"),
            None,
            None,
            None,
            None,
        );
        assert_eq!(r, InstallMethod::Unknown);
    }

    // ─── US-004: rpm-ostree (Silverblue / Kinoite) detection precedence ───

    #[test]
    fn detect_package_manager_debian_marker_wins() {
        // Debian-family systems never carry `/run/ostree-booted`, but if
        // they did, Apt still wins because apt is the one ground truth
        // for package routing on those hosts.
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
        // US-004 AC: Silverblue / Kinoite carry BOTH /etc/fedora-release AND
        // /run/ostree-booted. The ostree probe must fire first so these
        // users get routed to the informational `rpm-ostree upgrade` toast
        // instead of a broken `dnf install` that would fail on the
        // read-only /usr.
        assert_eq!(
            detect_package_manager_with_probes(false, true, false, true),
            PackageManager::RpmOstree
        );
    }

    #[test]
    fn detect_package_manager_ostree_without_fedora_marker_still_rpm_ostree() {
        // Bazzite / custom ostree spins that don't ship /etc/fedora-release
        // should still be detected correctly.
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
        // Endless OS (Debian-based with an ostree layer) carries BOTH
        // `/etc/debian_version` and `/run/ostree-booted`. An `apt install`
        // would fail against its read-only ostree base, and it is NOT
        // rpm-ostree either - so `(debian && ostree)` routes to the generic
        // externally-managed `Other` hint, checked BEFORE the plain-Debian
        // `Apt` arm.
        assert_eq!(
            detect_package_manager_with_probes(true, false, false, true),
            PackageManager::Other
        );
        // Plain Debian (no ostree) still uses apt …
        assert_eq!(
            detect_package_manager_with_probes(true, false, false, false),
            PackageManager::Apt
        );
        // … and Fedora Silverblue (ostree, no debian) is still rpm-ostree.
        assert_eq!(
            detect_package_manager_with_probes(false, true, false, true),
            PackageManager::RpmOstree
        );
    }
}
