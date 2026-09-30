use std::path::{Path, PathBuf};

use paneflow_home::SOCKET_PATH_ENV;

#[cfg(unix)]
pub(crate) const MAX_SOCKET_PATH_BYTES: usize = 104;

pub const APP_SUBDIR: &str = if cfg!(debug_assertions) {
    "paneflow-dev"
} else {
    "paneflow"
};

pub(crate) fn socket_path_spec() -> Option<paneflow_home::IpcEndpoint> {
    socket_path_spec_in(&paneflow_home::EndpointEnv::from_process())
}

fn socket_path_spec_in(env: &paneflow_home::EndpointEnv) -> Option<paneflow_home::IpcEndpoint> {
    let spec = paneflow_home::ipc_endpoint_in(env)?;
    #[cfg(unix)]
    if !check_sun_path_fits(&spec.path) {
        return None;
    }
    Some(spec)
}

pub(crate) fn socket_path() -> Option<PathBuf> {
    socket_path_spec().map(|spec| spec.path)
}

pub(crate) unsafe fn shed_inherited_instance_env() {
    let foreign_home = std::env::var_os(paneflow_home::HOME_ENV)
        .is_some_and(|raw| paneflow_home::paneflow_home().as_deref() != Some(Path::new(&raw)));
    let foreign_socket = std::env::var_os(SOCKET_PATH_ENV)
        .is_some_and(|raw| socket_path().as_deref() != Some(Path::new(&raw)));
    let shed_path = std::env::var_os(BIN_DIR_ENV).and_then(|bin_dir| {
        std::env::var_os("PATH").and_then(|path| path_without_dir(&path, Path::new(&bin_dir)))
    });
    unsafe {
        if let Some(path) = shed_path {
            std::env::set_var("PATH", path);
        }
        for key in paneflow_host::env::PANE_CONTEXT_ENV {
            std::env::remove_var(key);
        }
        if foreign_home {
            std::env::remove_var(paneflow_home::HOME_ENV);
        }
        if foreign_socket {
            std::env::remove_var(SOCKET_PATH_ENV);
        }
    }
}

const BIN_DIR_ENV: &str = "PANEFLOW_BIN_DIR";

fn path_without_dir(path: &std::ffi::OsStr, dir: &Path) -> Option<std::ffi::OsString> {
    let entries: Vec<PathBuf> = std::env::split_paths(path).collect();
    let kept: Vec<&PathBuf> = entries
        .iter()
        .filter(|entry| entry.as_path() != dir)
        .collect();
    (kept.len() != entries.len())
        .then(|| std::env::join_paths(kept).ok())
        .flatten()
}

pub(crate) fn shell_integration_dir() -> Option<PathBuf> {
    data_dir().map(|dir| shell_integration_dir_in(&dir))
}

pub(crate) fn shell_integration_dir_in(home: &std::path::Path) -> PathBuf {
    home.join("shell")
}

pub fn cache_dir() -> Option<PathBuf> {
    let dir = paneflow_home::cache_dir()?;
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::debug!(
            "paneflow: cache_dir {} is unwritable ({e}); callers will skip caching",
            dir.display()
        );
        return None;
    }
    Some(dir)
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
    let dir = paneflow_home::paneflow_home()?;
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
    Some(bridge_binary_path_in(&data_dir()?))
}

pub fn bridge_binary_path_in(home: &std::path::Path) -> PathBuf {
    helper_binary_path_in(home, "paneflow-mcp")
}

pub fn ai_hook_binary_path_in(home: &std::path::Path) -> PathBuf {
    helper_binary_path_in(home, "paneflow-ai-hook")
}

fn helper_binary_path_in(home: &std::path::Path, name: &str) -> PathBuf {
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    home.join("bin").join(format!("{name}{suffix}"))
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

#[cfg(unix)]
pub fn path_to_raw(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(windows)]
pub fn path_to_raw(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(unix)]
pub fn path_from_raw(raw: &[u8]) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    (!raw.is_empty()).then(|| PathBuf::from(std::ffi::OsStr::from_bytes(raw)))
}

#[cfg(windows)]
pub fn path_from_raw(raw: &[u8]) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    let (pairs, rest) = raw.as_chunks::<2>();
    if pairs.is_empty() || !rest.is_empty() {
        return None;
    }
    let wide: Vec<u16> = pairs.iter().copied().map(u16::from_le_bytes).collect();
    Some(PathBuf::from(std::ffi::OsString::from_wide(&wide)))
}

#[cfg(test)]
mod raw_path_tests {
    use super::{path_from_raw, path_to_raw};

    #[cfg(unix)]
    fn non_utf8_path() -> std::path::PathBuf {
        use std::os::unix::ffi::OsStrExt;
        std::path::PathBuf::from(std::ffi::OsStr::from_bytes(b"/wt/caf\xe9"))
    }

    #[cfg(windows)]
    fn non_utf8_path() -> std::path::PathBuf {
        use std::os::windows::ffi::OsStringExt;
        let mut wide: Vec<u16> = r"C:\wt\caf".encode_utf16().collect();
        wide.push(0xD800);
        std::path::PathBuf::from(std::ffi::OsString::from_wide(&wide))
    }

    #[test]
    fn a_non_utf8_path_survives_the_raw_round_trip() {
        let path = non_utf8_path();
        assert!(path.to_str().is_none());
        assert_eq!(path_from_raw(&path_to_raw(&path)), Some(path));
        assert_eq!(path_from_raw(&[]), None);
    }
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
    fn shedding_the_instance_context_drops_the_inherited_helper_dir_from_path() {
        let inherited = PathBuf::from("/home/u/.paneflow/cache/bin/0.17.4");
        let path = std::env::join_paths([
            inherited.clone(),
            PathBuf::from("/usr/bin"),
            inherited.clone(),
            PathBuf::from("/bin"),
        ])
        .expect("join");
        let shed = path_without_dir(&path, &inherited).expect("changed");
        assert_eq!(
            std::env::split_paths(&shed).collect::<Vec<_>>(),
            [PathBuf::from("/usr/bin"), PathBuf::from("/bin")]
        );
        assert_eq!(
            path_without_dir(&shed, &inherited),
            None,
            "no inherited helper dir, nothing to rewrite"
        );
        assert!(paneflow_host::env::PANE_CONTEXT_ENV.contains(&BIN_DIR_ENV));
    }

    #[test]
    fn shedding_pane_context_never_drops_an_honored_home_or_socket() {
        for key in paneflow_host::env::PANE_CONTEXT_ENV {
            assert_ne!(*key, SOCKET_PATH_ENV);
            assert_ne!(*key, paneflow_home::HOME_ENV);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use paneflow_home::EndpointEnv;

    const SOCKET_FILE: &str = if cfg!(debug_assertions) {
        "paneflow-dev.sock"
    } else {
        "paneflow.sock"
    };

    fn env(xdg_runtime_dir: Option<&str>) -> EndpointEnv {
        EndpointEnv {
            user_home: Some(PathBuf::from("/home/u")),
            xdg_runtime_dir: xdg_runtime_dir.map(Into::into),
            ..EndpointEnv::default()
        }
    }

    #[test]
    fn a_debug_build_launched_from_an_installed_pane_keeps_its_own_socket() {
        let tmpdir = tempfile::tempdir().expect("tmpdir");
        let release = tmpdir.path().join("paneflow").join("paneflow.sock");
        let spec = socket_path_spec_in(&EndpointEnv {
            socket_path: Some(release.clone().into_os_string()),
            tmpdir: Some(tmpdir.path().as_os_str().to_owned()),
            ..env(None)
        })
        .expect("socket spec");
        let expected = if cfg!(debug_assertions) {
            tmpdir.path().join("paneflow-dev").join("paneflow-dev.sock")
        } else {
            release
        };
        assert_eq!(spec.path, expected);
    }

    #[test]
    fn paneflow_socket_path_env_wins_when_absolute() {
        let spec = socket_path_spec_in(&EndpointEnv {
            socket_path: Some("/tmp/paneflow-isolated.sock".into()),
            ..env(Some("/run/user/1000"))
        })
        .expect("env socket path resolves");
        assert_eq!(spec.path, Path::new("/tmp/paneflow-isolated.sock"));
        assert!(
            !spec.owned_parent,
            "env override parent must not be treated as Paneflow-owned"
        );
    }

    #[test]
    fn a_usable_xdg_runtime_dir_wins_on_linux() {
        let runtime = tempfile::tempdir().expect("runtime dir");
        let runtime_str = runtime.path().to_str().expect("utf-8 tempdir");
        let spec = socket_path_spec_in(&env(Some(runtime_str))).expect("socket spec");
        if cfg!(target_os = "macos") {
            assert!(!spec.path.starts_with(runtime.path()));
        } else {
            assert_eq!(spec.path, runtime.path().join(APP_SUBDIR).join(SOCKET_FILE));
        }
        assert!(
            spec.owned_parent,
            "default runtime-dir socket is Paneflow-owned"
        );
    }

    #[test]
    fn tmpdir_is_the_fallback_when_xdg_runtime_dir_is_missing() {
        let tmpdir = tempfile::tempdir().expect("tmpdir");
        let spec = socket_path_spec_in(&EndpointEnv {
            tmpdir: Some(tmpdir.path().as_os_str().to_owned()),
            ..env(None)
        })
        .expect("socket spec");
        assert_eq!(spec.path, tmpdir.path().join(APP_SUBDIR).join(SOCKET_FILE));
    }

    #[test]
    fn overlong_path_returns_none() {
        let long = std::env::temp_dir().join("x".repeat(119));
        std::fs::create_dir_all(&long).expect("long runtime dir");
        let spec = socket_path_spec_in(&EndpointEnv {
            tmpdir: Some(long.as_os_str().to_owned()),
            ..env(None)
        });
        let _ = std::fs::remove_dir(&long);
        assert!(
            spec.is_none(),
            "AC6: over-long sun_path must return None rather than a bind-time error"
        );
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn paneflow_socket_path_env_wins_for_named_pipe() {
        let spec = socket_path_spec_in(&paneflow_home::EndpointEnv {
            user_home: Some(PathBuf::from(r"C:\Users\u")),
            socket_path: Some(r"\\.\pipe\paneflow-isolated-test".into()),
            ..paneflow_home::EndpointEnv::default()
        })
        .expect("named pipe");
        assert_eq!(spec.path, PathBuf::from(r"\\.\pipe\paneflow-isolated-test"));
    }
}
