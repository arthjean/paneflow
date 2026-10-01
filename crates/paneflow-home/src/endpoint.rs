use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

pub const SOCKET_PATH_ENV: &str = "PANEFLOW_SOCKET_PATH";

pub const ALLOW_SOCKET_OVERRIDE_ENV: &str = "PANEFLOW_ALLOW_SOCKET_OVERRIDE";

#[cfg(unix)]
const FALLBACK_RUNTIME_DIR: &str = "/tmp";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpcEndpoint {
    pub path: PathBuf,
    pub owned_parent: bool,
    pub home_is_isolated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EndpointEnv {
    pub paneflow_home: Option<OsString>,
    pub user_home: Option<PathBuf>,
    pub socket_path: Option<OsString>,
    pub allow_socket_override: Option<OsString>,
    pub xdg_runtime_dir: Option<OsString>,
    pub tmpdir: Option<OsString>,
    pub user_cache_dir: Option<PathBuf>,
}

impl EndpointEnv {
    pub fn from_process() -> Self {
        Self {
            paneflow_home: std::env::var_os(crate::HOME_ENV),
            user_home: dirs::home_dir(),
            socket_path: std::env::var_os(SOCKET_PATH_ENV),
            allow_socket_override: std::env::var_os(ALLOW_SOCKET_OVERRIDE_ENV),
            xdg_runtime_dir: std::env::var_os("XDG_RUNTIME_DIR"),
            tmpdir: std::env::var_os("TMPDIR"),
            user_cache_dir: dirs::cache_dir(),
        }
    }

    #[cfg(unix)]
    fn runtime_dir(&self) -> PathBuf {
        runtime_dir_from(
            self.xdg_runtime_dir.as_deref(),
            self.tmpdir.as_deref(),
            runtime_dir_is_usable,
        )
    }

    fn isolated_ipc_endpoint(&self) -> Option<PathBuf> {
        let home = crate::resolve_home(
            self.paneflow_home.clone(),
            self.user_home.clone(),
            crate::RESERVED_HOME_DIR_NAME,
        )?;
        let default_home = self
            .user_home
            .as_ref()
            .is_some_and(|user| crate::same_home(&home, &user.join(crate::HOME_DIR_NAME)));
        (!default_home).then(|| self.ipc_endpoint_path(&home))
    }

    #[cfg(unix)]
    fn ipc_endpoint_path(&self, home: &Path) -> PathBuf {
        crate::ipc_endpoint_path_in(&self.runtime_dir(), home)
    }

    #[cfg(windows)]
    fn ipc_endpoint_path(&self, home: &Path) -> PathBuf {
        crate::ipc_endpoint_path(home)
    }

    #[cfg(unix)]
    fn default_ipc_endpoint(&self, dev: bool) -> PathBuf {
        let (subdir, socket_file) = if dev {
            ("paneflow-dev", "paneflow-dev.sock")
        } else {
            ("paneflow", "paneflow.sock")
        };
        self.default_ipc_runtime_dir()
            .join(subdir)
            .join(socket_file)
    }

    #[cfg(unix)]
    fn default_ipc_runtime_dir(&self) -> PathBuf {
        usable_runtime_dir_from(
            self.xdg_runtime_dir.as_deref(),
            self.tmpdir.as_deref(),
            runtime_dir_is_usable,
        )
        .or_else(|| {
            self.user_cache_dir
                .as_ref()
                .filter(|dir| dir.is_absolute())
                .map(|dir| dir.join("run"))
        })
        .unwrap_or_else(|| PathBuf::from(FALLBACK_RUNTIME_DIR))
    }

    #[cfg(windows)]
    fn default_ipc_endpoint(&self, dev: bool) -> PathBuf {
        PathBuf::from(if dev {
            r"\\.\pipe\paneflow-dev"
        } else {
            r"\\.\pipe\paneflow"
        })
    }

    fn reserved_release_ipc_endpoint(&self) -> Option<PathBuf> {
        cfg!(debug_assertions).then(|| self.default_ipc_endpoint(false))
    }
}

pub fn ipc_endpoint() -> Option<IpcEndpoint> {
    ipc_endpoint_in(&EndpointEnv::from_process())
}

pub fn ipc_endpoint_in(env: &EndpointEnv) -> Option<IpcEndpoint> {
    let isolated = env.isolated_ipc_endpoint();
    let reserved = env.reserved_release_ipc_endpoint();
    let chosen = honored_endpoint_override(
        endpoint_from_env(env.socket_path.as_deref()),
        isolated.as_deref(),
        reserved.as_deref(),
        env.allow_socket_override.as_deref() == Some(OsStr::new("1")),
    )
    .or_else(|| isolated.clone());
    let home_is_isolated = isolated.is_some();
    Some(match chosen {
        Some(path) => IpcEndpoint {
            path,
            owned_parent: false,
            home_is_isolated,
        },
        None => IpcEndpoint {
            path: env.default_ipc_endpoint(cfg!(debug_assertions)),
            owned_parent: cfg!(unix),
            home_is_isolated,
        },
    })
}

pub fn same_endpoint(left: &Path, right: &Path) -> bool {
    if cfg!(windows) {
        left.as_os_str().eq_ignore_ascii_case(right.as_os_str())
    } else {
        left == right
    }
}

pub fn honored_endpoint_override(
    requested: Option<PathBuf>,
    isolated_endpoint: Option<&Path>,
    reserved: Option<&Path>,
    allow_override: bool,
) -> Option<PathBuf> {
    let requested = requested?;
    if reserved.is_some_and(|reserved| same_endpoint(reserved, &requested)) {
        return None;
    }
    match isolated_endpoint {
        Some(owned) if !allow_override && !same_endpoint(owned, &requested) => None,
        _ => Some(requested),
    }
}

pub fn endpoint_override_allowed() -> bool {
    std::env::var_os(ALLOW_SOCKET_OVERRIDE_ENV).is_some_and(|value| value == "1")
}

pub fn endpoint_from_env(raw: Option<&OsStr>) -> Option<PathBuf> {
    let path = PathBuf::from(raw?);
    path.is_absolute().then_some(path)
}

#[cfg(unix)]
pub fn runtime_dir() -> PathBuf {
    EndpointEnv::from_process().runtime_dir()
}

#[cfg(unix)]
fn runtime_dir_from(
    xdg_runtime_dir: Option<&OsStr>,
    tmpdir: Option<&OsStr>,
    usable: impl Fn(&Path) -> bool,
) -> PathBuf {
    usable_runtime_dir_from(xdg_runtime_dir, tmpdir, usable)
        .unwrap_or_else(|| PathBuf::from(FALLBACK_RUNTIME_DIR))
}

#[cfg(unix)]
fn usable_runtime_dir_from(
    xdg_runtime_dir: Option<&OsStr>,
    tmpdir: Option<&OsStr>,
    usable: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let xdg_runtime_dir = xdg_runtime_dir.filter(|_| !cfg!(target_os = "macos"));
    [xdg_runtime_dir, tmpdir]
        .into_iter()
        .flatten()
        .map(PathBuf::from)
        .find(|dir| dir.is_absolute() && usable(dir))
}

#[cfg(unix)]
fn runtime_dir_is_usable(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt as _;
    if !dir.is_dir() {
        return false;
    }
    let Ok(raw) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
        return false;
    };
    unsafe { libc::access(raw.as_ptr(), libc::W_OK | libc::X_OK) == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(name: &str) -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(format!(r"\\.\pipe\{name}"))
        } else {
            PathBuf::from(format!("/run/user/1000/{name}.sock"))
        }
    }

    fn env_with_user_home(user_home: &Path) -> EndpointEnv {
        EndpointEnv {
            user_home: Some(user_home.to_path_buf()),
            xdg_runtime_dir: Some("/run/user/1000".into()),
            tmpdir: Some("/tmp".into()),
            ..EndpointEnv::default()
        }
    }

    #[test]
    fn a_debug_build_never_addresses_the_release_socket() {
        let env = env_with_user_home(Path::new("/home/u"));
        let release = env.default_ipc_endpoint(false);
        let reserved = |path: &Path| {
            env.reserved_release_ipc_endpoint()
                .is_some_and(|reserved| same_endpoint(&reserved, path))
        };
        assert_eq!(
            reserved(&release),
            cfg!(debug_assertions),
            "a dev CLI or test inside an installed pane must not drive the installed app"
        );
        assert!(!reserved(&env.default_ipc_endpoint(true)));
        let inherited_release = EndpointEnv {
            socket_path: Some(release.clone().into_os_string()),
            ..env.clone()
        };
        let resolved = ipc_endpoint_in(&inherited_release).expect("endpoint").path;
        assert_eq!(resolved == release, !cfg!(debug_assertions));
    }

    #[test]
    fn an_isolated_home_resolves_its_own_socket_over_an_inherited_one() {
        let user_home = if cfg!(windows) {
            Path::new(r"C:\Users\u")
        } else {
            Path::new("/home/u")
        };
        let isolated_home = user_home.join(".paneflow-dev-alpha");
        let isolated_home = isolated_home.as_path();
        let inherited = endpoint("paneflow-ipc-fedcba9876543210");
        let env = |allow: Option<&str>| EndpointEnv {
            paneflow_home: Some(isolated_home.as_os_str().to_owned()),
            socket_path: Some(inherited.clone().into_os_string()),
            allow_socket_override: allow.map(OsString::from),
            ..env_with_user_home(user_home)
        };
        let owned = env(None).ipc_endpoint_path(isolated_home);
        let resolved = |allow| ipc_endpoint_in(&env(allow)).expect("endpoint");
        assert_eq!(resolved(None).path, owned);
        assert!(resolved(None).home_is_isolated);
        assert!(!resolved(None).owned_parent);
        assert_eq!(resolved(Some("0")).path, owned);
        assert_eq!(resolved(Some("1")).path, inherited);
    }

    #[test]
    fn the_default_home_honors_an_absolute_socket_and_otherwise_owns_its_runtime_socket() {
        let user_home = Path::new("/home/u");
        let explicit = endpoint("paneflow-isolated");
        let env = env_with_user_home(user_home);
        let default = ipc_endpoint_in(&env).expect("default endpoint");
        assert!(!default.home_is_isolated);
        assert_eq!(default.owned_parent, cfg!(unix));
        assert_eq!(
            default.path,
            env.default_ipc_endpoint(cfg!(debug_assertions))
        );
        let honored = ipc_endpoint_in(&EndpointEnv {
            socket_path: Some(explicit.clone().into_os_string()),
            ..env.clone()
        })
        .expect("explicit endpoint");
        assert_eq!(honored.path, explicit);
        assert!(!honored.owned_parent);
        let relative = ipc_endpoint_in(&EndpointEnv {
            socket_path: Some("relative.sock".into()),
            ..env
        })
        .expect("relative override ignored");
        assert_eq!(relative, default);
    }

    #[test]
    fn an_endpoint_from_env_must_be_absolute() {
        #[cfg(not(windows))]
        let absolute = "/run/user/1000/paneflow/paneflow.sock";
        #[cfg(windows)]
        let absolute = r"\\.\pipe\paneflow";
        let raw = |value: &'static str| Some(OsStr::new(value));
        assert_eq!(
            endpoint_from_env(raw(absolute)),
            Some(PathBuf::from(absolute))
        );
        assert_eq!(endpoint_from_env(raw("relative/path.sock")), None);
        assert_eq!(endpoint_from_env(raw("")), None);
        assert_eq!(endpoint_from_env(None), None);
    }

    #[test]
    fn a_build_that_is_not_the_release_never_binds_the_release_socket() {
        let release = endpoint("paneflow");
        assert_eq!(
            honored_endpoint_override(Some(release.clone()), None, Some(&release), false),
            None,
            "a pane of the installed app exports its socket; a dev build must not claim it"
        );
        assert_eq!(
            honored_endpoint_override(Some(release.clone()), None, Some(&release), true),
            None,
            "the reservation holds even when overrides are allowed"
        );
        assert_eq!(
            honored_endpoint_override(Some(release.clone()), None, None, false),
            Some(release),
            "the release build keeps honoring its own socket"
        );
    }

    #[test]
    fn an_isolated_home_ignores_a_socket_it_does_not_own() {
        let owned = endpoint("paneflow-ipc-0123456789abcdef");
        let parent = endpoint("paneflow-ipc-fedcba9876543210");
        assert_eq!(
            honored_endpoint_override(Some(parent.clone()), Some(&owned), None, false),
            None
        );
        assert_eq!(
            honored_endpoint_override(Some(owned.clone()), Some(&owned), None, false),
            Some(owned.clone())
        );
        assert_eq!(
            honored_endpoint_override(Some(parent.clone()), Some(&owned), None, true),
            Some(parent.clone()),
            "PANEFLOW_ALLOW_SOCKET_OVERRIDE=1 is the explicit escape hatch"
        );
        assert_eq!(
            honored_endpoint_override(Some(parent.clone()), None, None, false),
            Some(parent),
            "the default home keeps honoring an explicit socket"
        );
        assert_eq!(
            honored_endpoint_override(None, Some(&owned), None, false),
            None
        );
    }

    #[test]
    fn named_pipe_endpoints_compare_without_case() {
        let upper = PathBuf::from(r"\\.\pipe\Paneflow-IPC-ABC");
        let lower = PathBuf::from(r"\\.\pipe\paneflow-ipc-abc");
        assert_eq!(same_endpoint(&upper, &lower), cfg!(windows));
        assert!(same_endpoint(&lower, &lower));
    }

    #[cfg(windows)]
    #[test]
    fn windows_default_endpoint_matches_build_profile() {
        let expected = if cfg!(debug_assertions) {
            r"\\.\pipe\paneflow-dev"
        } else {
            r"\\.\pipe\paneflow"
        };
        assert_eq!(
            ipc_endpoint_in(&env_with_user_home(Path::new(r"C:\Users\u"))).map(|e| e.path),
            Some(PathBuf::from(expected))
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_runtime_dir_prefers_a_usable_xdg_dir_then_tmpdir_then_tmp() {
        let xdg = OsStr::new("/run/user/1000");
        let tmp = OsStr::new("/var/folders/xy/T");
        let all = |_: &Path| true;
        let expected_with_xdg = if cfg!(target_os = "macos") {
            PathBuf::from("/var/folders/xy/T")
        } else {
            PathBuf::from("/run/user/1000")
        };
        assert_eq!(
            runtime_dir_from(Some(xdg), Some(tmp), all),
            expected_with_xdg
        );
        assert_eq!(
            runtime_dir_from(None, Some(tmp), all),
            PathBuf::from("/var/folders/xy/T")
        );
        assert_eq!(
            runtime_dir_from(Some(OsStr::new("relative")), None, all),
            PathBuf::from(FALLBACK_RUNTIME_DIR)
        );
        assert_eq!(
            runtime_dir_from(Some(OsStr::new("")), Some(OsStr::new("")), all),
            PathBuf::from(FALLBACK_RUNTIME_DIR)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_ignores_xdg_runtime_dir() {
        let xdg = OsStr::new("/run/user/501");
        assert_eq!(
            runtime_dir_from(Some(xdg), None, |_| true),
            PathBuf::from(FALLBACK_RUNTIME_DIR)
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_unusable_xdg_runtime_dir_falls_back_like_a_missing_one() {
        let usable_tmp = tempfile::tempdir().expect("tempdir");
        let missing = usable_tmp.path().join("missing");
        let file = usable_tmp.path().join("file");
        std::fs::write(&file, b"").expect("file");
        let tmp = usable_tmp.path().as_os_str();
        for unusable in [missing.as_os_str(), file.as_os_str(), OsStr::new("/")] {
            if unusable == OsStr::new("/") && runtime_dir_is_usable(Path::new("/")) {
                continue;
            }
            assert_eq!(
                runtime_dir_from(Some(unusable), Some(tmp), runtime_dir_is_usable),
                usable_tmp.path(),
                "{} is unusable",
                Path::new(unusable).display()
            );
            assert_eq!(
                runtime_dir_from(Some(unusable), None, runtime_dir_is_usable),
                PathBuf::from(FALLBACK_RUNTIME_DIR)
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn without_a_usable_runtime_dir_the_default_socket_stays_in_the_user_cache() {
        let cache = tempfile::tempdir().expect("cache");
        let env = EndpointEnv {
            xdg_runtime_dir: Some("relative".into()),
            tmpdir: None,
            user_cache_dir: Some(cache.path().to_path_buf()),
            ..env_with_user_home(Path::new("/home/u"))
        };
        let socket = if cfg!(debug_assertions) {
            "paneflow-dev/paneflow-dev.sock"
        } else {
            "paneflow/paneflow.sock"
        };
        let endpoint = ipc_endpoint_in(&env).expect("endpoint");
        assert_eq!(endpoint.path, cache.path().join("run").join(socket));
        assert!(endpoint.owned_parent);
    }

    #[cfg(unix)]
    #[test]
    fn a_root_owned_xdg_runtime_dir_puts_client_and_server_on_the_same_fallback_socket() {
        if runtime_dir_is_usable(Path::new("/")) {
            return;
        }
        let tmpdir = tempfile::tempdir().expect("tmpdir");
        let env = EndpointEnv {
            xdg_runtime_dir: Some("/".into()),
            tmpdir: Some(tmpdir.path().as_os_str().to_owned()),
            ..env_with_user_home(Path::new("/home/u"))
        };
        let subdir = if cfg!(debug_assertions) {
            "paneflow-dev/paneflow-dev.sock"
        } else {
            "paneflow/paneflow.sock"
        };
        let endpoint = ipc_endpoint_in(&env).expect("endpoint");
        assert_eq!(endpoint.path, tmpdir.path().join(subdir));
        assert!(endpoint.owned_parent);
        assert_eq!(ipc_endpoint_in(&env), Some(endpoint));
    }
}
