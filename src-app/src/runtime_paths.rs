use std::path::{Path, PathBuf};

#[cfg(unix)]
pub(crate) const MAX_SOCKET_PATH_BYTES: usize = 104;

pub const APP_SUBDIR: &str = if cfg!(debug_assertions) {
    "paneflow-dev"
} else {
    "paneflow"
};

#[cfg(unix)]
const PANEFLOW_SUBDIR: &str = APP_SUBDIR;
#[cfg(unix)]
const SOCKET_FILE: &str = if cfg!(debug_assertions) {
    "paneflow-dev.sock"
} else {
    "paneflow.sock"
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IpcSocketPath {
    path: PathBuf,
    owned_parent: bool,
}

impl IpcSocketPath {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(unix)]
    pub(crate) fn owned_parent(&self) -> bool {
        self.owned_parent
    }
}

#[cfg(unix)]
fn runtime_dir() -> Option<PathBuf> {
    std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(dirs::runtime_dir)
        .or_else(|| {
            std::env::var("TMPDIR")
                .ok()
                .map(PathBuf::from)
                .filter(|p| !p.as_os_str().is_empty())
        })
        .or_else(|| dirs::cache_dir().map(|d| d.join("run")))
}

#[cfg(unix)]
pub(crate) fn socket_path_spec() -> Option<IpcSocketPath> {
    if let Some(path) = socket_path_from_env(std::env::var_os("PANEFLOW_SOCKET_PATH")) {
        return check_sun_path_fits(&path).then_some(IpcSocketPath {
            path,
            owned_parent: false,
        });
    }
    let path = runtime_dir()?.join(PANEFLOW_SUBDIR).join(SOCKET_FILE);
    check_sun_path_fits(&path).then_some(IpcSocketPath {
        path,
        owned_parent: true,
    })
}

#[cfg(windows)]
pub(crate) fn socket_path_spec() -> Option<IpcSocketPath> {
    if let Some(path) = socket_path_from_env(std::env::var_os("PANEFLOW_SOCKET_PATH")) {
        return Some(IpcSocketPath {
            path,
            owned_parent: false,
        });
    }
    Some(IpcSocketPath {
        path: PathBuf::from(if cfg!(debug_assertions) {
            r"\\.\pipe\paneflow-dev"
        } else {
            r"\\.\pipe\paneflow"
        }),
        owned_parent: false,
    })
}

pub(crate) fn socket_path() -> Option<PathBuf> {
    socket_path_spec().map(|spec| spec.path)
}

pub(crate) fn shell_integration_dir() -> Option<PathBuf> {
    data_dir().map(|dir| dir.join("shell"))
}

fn socket_path_from_env(raw: Option<std::ffi::OsString>) -> Option<PathBuf> {
    let path = PathBuf::from(raw?);
    path.is_absolute().then_some(path)
}

pub fn augment_path_for_gui_launch() {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".bun").join("bin"));
        candidates.push(home.join(".cargo").join("bin"));
        candidates.push(home.join(".local").join("bin"));
    }

    #[cfg(target_os = "macos")]
    {
        candidates.push(PathBuf::from("/opt/homebrew/bin"));
        candidates.push(PathBuf::from("/usr/local/bin"));
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(home) = dirs::home_dir() {
            candidates.push(home.join(".bun").join("bin"));
        }
        if let Some(program_files) = std::env::var_os("ProgramFiles") {
            candidates.push(PathBuf::from(&program_files).join("Git").join("cmd"));
        }
        if let Some(program_files_x86) = std::env::var_os("ProgramFiles(x86)") {
            candidates.push(PathBuf::from(&program_files_x86).join("Git").join("cmd"));
        }
        if let Some(local) = dirs::data_local_dir() {
            candidates.push(local.join("Programs").join("Git").join("cmd"));
        }
    }

    let current = std::env::var_os("PATH").unwrap_or_default();
    let existing: Vec<PathBuf> = std::env::split_paths(&current).collect();

    let mut to_prepend: Vec<PathBuf> = Vec::new();
    for cand in candidates {
        if !cand.is_dir() {
            continue;
        }
        if existing.iter().any(|p| p == &cand) {
            continue;
        }
        if to_prepend.contains(&cand) {
            continue;
        }
        to_prepend.push(cand);
    }

    if to_prepend.is_empty() {
        return;
    }

    let mut merged: Vec<PathBuf> = to_prepend.clone();
    merged.extend(existing);

    match std::env::join_paths(&merged) {
        Ok(joined) => {
            log::info!(
                "paneflow: augmented PATH with user bin dirs: {}",
                to_prepend
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            unsafe { std::env::set_var("PATH", joined) };
        }
        Err(e) => {
            log::warn!("paneflow: failed to join augmented PATH ({e}); leaving PATH unchanged");
        }
    }
}

pub fn data_dir() -> Option<PathBuf> {
    let dir = dirs::data_local_dir()?.join(APP_SUBDIR);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::debug!(
            "paneflow: data_dir {} is unwritable ({e}); callers will use ephemeral state",
            dir.display()
        );
        return None;
    }
    Some(dir)
}

pub fn bridge_binary_path() -> Option<PathBuf> {
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    Some(
        data_dir()?
            .join("bin")
            .join(format!("paneflow-mcp{suffix}")),
    )
}

pub fn ai_hook_binary_path() -> Option<PathBuf> {
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    Some(
        data_dir()?
            .join("bin")
            .join(format!("paneflow-ai-hook{suffix}")),
    )
}

#[cfg(unix)]
fn check_sun_path_fits(path: &std::path::Path) -> bool {
    let bytes = path.as_os_str().len();
    if bytes >= MAX_SOCKET_PATH_BYTES {
        log::warn!(
            "paneflow: computed IPC socket path does not fit sun_path ({} >= {} bytes, no room for the NUL terminator): {} - IPC will be disabled. Set $XDG_RUNTIME_DIR (Linux) or shorten $TMPDIR (macOS) to enable it.",
            bytes,
            MAX_SOCKET_PATH_BYTES,
            path.display()
        );
        false
    } else {
        true
    }
}

pub fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    let stripped = path.to_str().and_then(|s| {
        s.strip_prefix(r"\\?\UNC\")
            .map(|rest| PathBuf::from(format!(r"\\{rest}")))
            .or_else(|| s.strip_prefix(r"\\?\").map(PathBuf::from))
    });
    stripped.unwrap_or(path)
}

#[cfg(test)]
mod verbatim_prefix_tests {
    use super::strip_verbatim_prefix;
    use std::path::PathBuf;

    #[test]
    fn disk_unc_and_passthrough() {
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"\\?\C:\dev\paneflow")),
            PathBuf::from(r"C:\dev\paneflow")
        );
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"\\?\UNC\server\share\paneflow")),
            PathBuf::from(r"\\server\share\paneflow")
        );
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"C:\dev\paneflow")),
            PathBuf::from(r"C:\dev\paneflow")
        );
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from("/home/arthur/paneflow")),
            PathBuf::from("/home/arthur/paneflow")
        );
    }

    #[test]
    fn a_forward_slash_tail_is_left_alone() {
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"\\?\C:/Program Files/PaneFlow/paneflow.exe")),
            PathBuf::from("C:/Program Files/PaneFlow/paneflow.exe")
        );
    }

    #[cfg(windows)]
    #[test]
    fn stripped_form_matches_what_git_prints() {
        let from_git = PathBuf::from("C:/dev/paneflow");
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"\\?\C:\dev\paneflow")),
            from_git
        );
        assert_ne!(PathBuf::from(r"\\?\C:\dev\paneflow"), from_git);
    }
}

#[cfg(test)]
mod socket_env_tests {
    use super::*;

    #[test]
    fn socket_path_env_helper_requires_absolute_path() {
        let absolute = if cfg!(windows) {
            r"\\.\pipe\paneflow-test"
        } else {
            "/tmp/paneflow-test.sock"
        };
        assert_eq!(
            socket_path_from_env(Some(std::ffi::OsString::from(absolute))),
            Some(PathBuf::from(absolute))
        );
        assert_eq!(
            socket_path_from_env(Some(std::ffi::OsString::from("relative-paneflow.sock"))),
            None
        );
        assert_eq!(socket_path_from_env(None), None);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        socket: Option<String>,
        xdg: Option<String>,
        tmp: Option<String>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn take() -> Self {
            let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            Self {
                socket: std::env::var("PANEFLOW_SOCKET_PATH").ok(),
                xdg: std::env::var("XDG_RUNTIME_DIR").ok(),
                tmp: std::env::var("TMPDIR").ok(),
                _guard: guard,
            }
        }

        fn clear(&self) {
            unsafe {
                std::env::remove_var("PANEFLOW_SOCKET_PATH");
                std::env::remove_var("XDG_RUNTIME_DIR");
                std::env::remove_var("TMPDIR");
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.socket {
                    Some(v) => std::env::set_var("PANEFLOW_SOCKET_PATH", v),
                    None => std::env::remove_var("PANEFLOW_SOCKET_PATH"),
                }
                match &self.xdg {
                    Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
                    None => std::env::remove_var("XDG_RUNTIME_DIR"),
                }
                match &self.tmp {
                    Some(v) => std::env::set_var("TMPDIR", v),
                    None => std::env::remove_var("TMPDIR"),
                }
            }
        }
    }

    #[test]
    fn paneflow_socket_path_env_wins_when_absolute() {
        let g = EnvGuard::take();
        g.clear();
        unsafe {
            std::env::set_var("PANEFLOW_SOCKET_PATH", "/tmp/paneflow-isolated.sock");
            std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        }
        assert_eq!(
            socket_path(),
            Some(PathBuf::from("/tmp/paneflow-isolated.sock"))
        );
        let spec = socket_path_spec().expect("env socket path resolves");
        assert_eq!(spec.path(), Path::new("/tmp/paneflow-isolated.sock"));
        assert!(
            !spec.owned_parent(),
            "env override parent must not be treated as Paneflow-owned"
        );
    }

    #[test]
    fn xdg_runtime_dir_wins_when_set() {
        let g = EnvGuard::take();
        g.clear();
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000") };
        let p = socket_path().expect("runtime dir must resolve");
        assert_eq!(
            p,
            PathBuf::from(format!("/run/user/1000/{APP_SUBDIR}/{SOCKET_FILE}")),
            "AC5: Linux with XDG_RUNTIME_DIR must resolve to the XDG path \
             (subdir + filename vary by build profile via APP_SUBDIR / SOCKET_FILE)"
        );
        assert!(
            socket_path_spec().expect("socket spec").owned_parent(),
            "default runtime-dir socket is Paneflow-owned"
        );
    }

    #[test]
    fn tmpdir_fallback_when_xdg_and_runtime_dir_missing() {
        let g = EnvGuard::take();
        g.clear();
        unsafe { std::env::set_var("TMPDIR", "/tmp/macos-stub") };
        let p = socket_path();
        if let Some(p) = p {
            assert!(p.ends_with(format!("{APP_SUBDIR}/{SOCKET_FILE}")));
        }
    }

    #[test]
    fn overlong_path_returns_none() {
        let g = EnvGuard::take();
        g.clear();
        let long = "/".to_string() + &"x".repeat(119);
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", &long) };
        assert!(
            socket_path().is_none(),
            "AC6: over-long sun_path must return None rather than a bind-time error"
        );
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        socket: Option<String>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn take() -> Self {
            let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            Self {
                socket: std::env::var("PANEFLOW_SOCKET_PATH").ok(),
                _guard: guard,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.socket {
                    Some(v) => std::env::set_var("PANEFLOW_SOCKET_PATH", v),
                    None => std::env::remove_var("PANEFLOW_SOCKET_PATH"),
                }
            }
        }
    }

    #[test]
    fn paneflow_socket_path_env_wins_for_named_pipe() {
        let _guard = EnvGuard::take();
        unsafe {
            std::env::set_var("PANEFLOW_SOCKET_PATH", r"\\.\pipe\paneflow-isolated-test");
        }
        assert_eq!(
            socket_path(),
            Some(PathBuf::from(r"\\.\pipe\paneflow-isolated-test"))
        );
    }
}
