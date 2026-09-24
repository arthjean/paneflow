use std::path::{Path, PathBuf};

pub const HOME_DIR_NAME: &str = if cfg!(debug_assertions) {
    ".paneflow-dev"
} else {
    ".paneflow"
};

pub const HOME_ENV: &str = "PANEFLOW_HOME";

const RELEASE_HOME_DIR_NAME: &str = ".paneflow";

const RESERVED_HOME_DIR_NAME: Option<&str> = if cfg!(debug_assertions) {
    Some(RELEASE_HOME_DIR_NAME)
} else {
    None
};

const LEGACY_SUBDIR: &str = if cfg!(debug_assertions) {
    "paneflow-dev"
} else {
    "paneflow"
};

pub fn paneflow_home() -> Option<PathBuf> {
    resolve_home(
        std::env::var_os(HOME_ENV),
        dirs::home_dir(),
        RESERVED_HOME_DIR_NAME,
    )
}

fn resolve_home(
    requested: Option<std::ffi::OsString>,
    user_home: Option<PathBuf>,
    reserved_dir_name: Option<&str>,
) -> Option<PathBuf> {
    let reserved = reserved_dir_name.and_then(|name| Some(user_home.as_ref()?.join(name)));
    let requested = requested
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .filter(|path| {
            !reserved
                .as_deref()
                .is_some_and(|reserved| same_home(path, reserved))
        });
    requested.or_else(|| user_home.map(|home| home.join(HOME_DIR_NAME)))
}

pub fn is_default_home(home: &Path) -> bool {
    dirs::home_dir().is_some_and(|user| same_home(home, &user.join(HOME_DIR_NAME)))
}

fn same_home(left: &Path, right: &Path) -> bool {
    normalized_home(left) == normalized_home(right)
}

pub fn config_path() -> Option<PathBuf> {
    paneflow_home().map(|home| home.join("paneflow.json"))
}

pub fn session_path() -> Option<PathBuf> {
    paneflow_home().map(|home| home.join("session.json"))
}

pub fn recents_path() -> Option<PathBuf> {
    paneflow_home().map(|home| home.join("recents.json"))
}

pub fn window_state_path() -> Option<PathBuf> {
    paneflow_home().map(|home| home.join("window-state.json"))
}

pub fn cache_dir() -> Option<PathBuf> {
    paneflow_home().map(|home| home.join("cache"))
}

pub fn worktrees_dir() -> Option<PathBuf> {
    paneflow_home().map(|home| home.join("worktrees"))
}

pub const HOST_DIR_NAME: &str = "host";

pub const HOST_SESSIONS_DIR_NAME: &str = "sessions";

pub const HOST_SESSION_DATA_DIR_NAME: &str = "session-data";

pub const HOST_INSTANCE_FILE_NAME: &str = "instance.json";

pub fn host_dir_in(home: &Path) -> PathBuf {
    home.join(HOST_DIR_NAME)
}

pub fn host_sessions_dir_in(home: &Path) -> PathBuf {
    host_dir_in(home).join(HOST_SESSIONS_DIR_NAME)
}

pub fn host_session_manifest_path_in(home: &Path, session_id: &str) -> PathBuf {
    host_sessions_dir_in(home).join(format!("{session_id}.json"))
}

pub fn host_session_data_root_in(home: &Path) -> PathBuf {
    host_dir_in(home).join(HOST_SESSION_DATA_DIR_NAME)
}

pub fn host_session_data_dir_in(home: &Path, session_id: &str) -> PathBuf {
    host_session_data_root_in(home).join(session_id)
}

pub fn host_instance_record_path_in(home: &Path) -> PathBuf {
    host_dir_in(home).join(HOST_INSTANCE_FILE_NAME)
}

pub fn host_dir() -> Option<PathBuf> {
    paneflow_home().map(|home| host_dir_in(&home))
}

pub fn host_sessions_dir() -> Option<PathBuf> {
    paneflow_home().map(|home| host_sessions_dir_in(&home))
}

fn normalized_home(home: &Path) -> String {
    let normalized: String = home
        .to_string_lossy()
        .chars()
        .map(|c| if c == '\\' { '/' } else { c })
        .collect();
    let normalized = if cfg!(windows) {
        normalized.to_lowercase()
    } else {
        normalized
    };
    normalized.trim_end_matches('/').to_string()
}

pub fn home_fingerprint(home: &Path) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let normalized = normalized_home(home);
    let mut hash = FNV_OFFSET;
    for byte in normalized.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{hash:016x}")
}

const HOST_ENDPOINT_PREFIX: &str = "paneflow-host-";

#[cfg(windows)]
pub fn host_endpoint_path(home: &Path) -> PathBuf {
    PathBuf::from(format!(
        r"\\.\pipe\{HOST_ENDPOINT_PREFIX}{}",
        home_fingerprint(home)
    ))
}

#[cfg(unix)]
pub fn host_endpoint_path(home: &Path) -> PathBuf {
    host_runtime_dir().join(format!(
        "{HOST_ENDPOINT_PREFIX}{}.sock",
        home_fingerprint(home)
    ))
}

#[cfg(unix)]
fn host_runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty() && p.is_absolute())
        .or_else(|| {
            std::env::var_os("TMPDIR")
                .map(PathBuf::from)
                .filter(|p| !p.as_os_str().is_empty() && p.is_absolute())
        })
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

pub fn host_endpoint_path_for_current_home() -> Option<PathBuf> {
    paneflow_home().map(|home| host_endpoint_path(&home))
}

pub fn reserved_host_endpoint() -> Option<PathBuf> {
    let reserved = RESERVED_HOME_DIR_NAME?;
    Some(host_endpoint_path(&dirs::home_dir()?.join(reserved)))
}

pub const SERVE_DIR_NAME: &str = "serve";

pub const SERVE_OWNER_LOCK_FILE_NAME: &str = "owner.lock";

pub const SERVE_INSTANCE_FILE_NAME: &str = "instance.json";

pub const SERVE_LOG_FILE_NAME: &str = "serve.log";

pub const SERVE_RUNTIME_DIR_NAME: &str = "runtime";

pub fn serve_dir_in(home: &Path) -> PathBuf {
    home.join(SERVE_DIR_NAME)
}

pub fn serve_owner_lock_path_in(home: &Path) -> PathBuf {
    serve_dir_in(home).join(SERVE_OWNER_LOCK_FILE_NAME)
}

pub fn serve_instance_record_path_in(home: &Path) -> PathBuf {
    serve_dir_in(home).join(SERVE_INSTANCE_FILE_NAME)
}

pub fn serve_log_path_in(home: &Path) -> PathBuf {
    serve_dir_in(home).join(SERVE_LOG_FILE_NAME)
}

pub fn serve_runtime_dir_in(home: &Path) -> PathBuf {
    serve_dir_in(home).join(SERVE_RUNTIME_DIR_NAME)
}

const SERVE_ENDPOINT_PREFIX: &str = "paneflow-serve-";

#[cfg(windows)]
pub fn serve_endpoint_path(home: &Path) -> PathBuf {
    PathBuf::from(format!(
        r"\\.\pipe\{SERVE_ENDPOINT_PREFIX}{}",
        home_fingerprint(home)
    ))
}

#[cfg(unix)]
pub fn serve_endpoint_path(home: &Path) -> PathBuf {
    host_runtime_dir().join(format!(
        "{SERVE_ENDPOINT_PREFIX}{}.sock",
        home_fingerprint(home)
    ))
}

pub fn serve_endpoint_path_for_current_home() -> Option<PathBuf> {
    paneflow_home().map(|home| serve_endpoint_path(&home))
}

const IPC_ENDPOINT_PREFIX: &str = "paneflow-ipc-";

#[cfg(windows)]
pub fn ipc_endpoint_path(home: &Path) -> PathBuf {
    PathBuf::from(format!(
        r"\\.\pipe\{IPC_ENDPOINT_PREFIX}{}",
        home_fingerprint(home)
    ))
}

#[cfg(unix)]
pub fn ipc_endpoint_path(home: &Path) -> PathBuf {
    host_runtime_dir().join(format!(
        "{IPC_ENDPOINT_PREFIX}{}.sock",
        home_fingerprint(home)
    ))
}

pub fn isolated_ipc_endpoint_for_current_home() -> Option<PathBuf> {
    paneflow_home()
        .filter(|home| !is_default_home(home))
        .map(|home| ipc_endpoint_path(&home))
}

pub fn legacy_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join(LEGACY_SUBDIR).join("paneflow.json"))
}

pub fn legacy_session_path() -> Option<PathBuf> {
    let filename = if cfg!(debug_assertions) {
        "session-dev.json"
    } else {
        "session.json"
    };
    dirs::cache_dir().map(|dir| dir.join(LEGACY_SUBDIR).join(filename))
}

pub fn legacy_window_state_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("paneflow").join("window-state.json"))
}

pub fn legacy_data_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|dir| dir.join(LEGACY_SUBDIR))
}

pub fn migrate_legacy_home() -> Vec<PathBuf> {
    let Some(home) = paneflow_home() else {
        return Vec::new();
    };
    let mut pairs: Vec<(Option<PathBuf>, PathBuf)> = vec![
        (legacy_config_path(), home.join("paneflow.json")),
        (legacy_session_path(), home.join("session.json")),
        (legacy_window_state_path(), home.join("window-state.json")),
        (
            legacy_data_dir().map(|dir| dir.join("telemetry_id")),
            home.join("telemetry_id"),
        ),
    ];
    let pairs: Vec<(PathBuf, PathBuf)> = pairs
        .drain(..)
        .filter_map(|(legacy, target)| legacy.map(|legacy| (legacy, target)))
        .collect();
    migrate_files(&pairs)
}

pub fn migrate_files(pairs: &[(PathBuf, PathBuf)]) -> Vec<PathBuf> {
    let mut migrated = Vec::new();
    for (legacy, target) in pairs {
        if target.exists() || !legacy.is_file() {
            continue;
        }
        if let Some(parent) = target.parent()
            && std::fs::create_dir_all(parent).is_err()
        {
            continue;
        }
        if copy_private(legacy, target).is_ok() {
            migrated.push(target.clone());
        }
    }
    migrated
}

fn copy_private(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::copy(from, to)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_copies_only_what_is_missing_at_the_target() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let legacy = tmp.path().join("legacy");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&legacy).expect("legacy dir");
        std::fs::write(legacy.join("paneflow.json"), "{\"theme\":\"x\"}").expect("config");
        std::fs::write(legacy.join("session.json"), "{}").expect("session");
        std::fs::create_dir_all(&home).expect("home dir");
        std::fs::write(home.join("session.json"), "{\"kept\":true}").expect("existing");

        let pairs = vec![
            (legacy.join("paneflow.json"), home.join("paneflow.json")),
            (legacy.join("session.json"), home.join("session.json")),
            (legacy.join("missing.json"), home.join("missing.json")),
        ];
        let migrated = migrate_files(&pairs);

        assert_eq!(migrated, vec![home.join("paneflow.json")]);
        assert_eq!(
            std::fs::read_to_string(home.join("paneflow.json")).expect("copied"),
            "{\"theme\":\"x\"}"
        );
        assert_eq!(
            std::fs::read_to_string(home.join("session.json")).expect("kept"),
            "{\"kept\":true}",
            "an existing file at the new home is never overwritten"
        );
        assert!(
            legacy.join("paneflow.json").exists(),
            "the legacy file stays as a fallback for older builds"
        );
        assert!(!home.join("missing.json").exists());
    }

    #[test]
    fn home_layout_hangs_off_one_directory() {
        let home = paneflow_home().expect("home resolves on the test host");
        assert!(
            home.file_name().and_then(|n| n.to_str()) == Some(HOME_DIR_NAME)
                || std::env::var_os(HOME_ENV).is_some()
        );
        assert_eq!(config_path().expect("config"), home.join("paneflow.json"));
        assert_eq!(session_path().expect("session"), home.join("session.json"));
        assert_eq!(cache_dir().expect("cache"), home.join("cache"));
        assert_eq!(worktrees_dir().expect("worktrees"), home.join("worktrees"));
        assert_eq!(host_dir().expect("host"), home.join("host"));
        assert_eq!(
            host_sessions_dir().expect("host sessions"),
            home.join("host").join("sessions")
        );
    }

    #[test]
    fn host_records_hang_off_the_home_that_owns_them() {
        let home = Path::new("/srv/home/.paneflow-dev");
        assert_eq!(
            host_session_manifest_path_in(home, "550e8400-e29b-41d4-a716-446655440000"),
            home.join("host")
                .join("sessions")
                .join("550e8400-e29b-41d4-a716-446655440000.json")
        );
        assert_eq!(
            host_instance_record_path_in(home),
            home.join("host").join("instance.json")
        );
        assert_eq!(
            host_session_data_dir_in(home, "550e8400-e29b-41d4-a716-446655440000"),
            home.join("host")
                .join("session-data")
                .join("550e8400-e29b-41d4-a716-446655440000")
        );
    }

    #[test]
    fn a_dev_build_never_adopts_the_release_home_it_inherits() {
        let user = PathBuf::from(if cfg!(windows) {
            r"C:\Users\arthur"
        } else {
            "/home/arthur"
        });
        let own = user.join(HOME_DIR_NAME);
        let release = user.join(RELEASE_HOME_DIR_NAME);
        let isolated = user.join(".paneflow-dev-review");

        assert_eq!(
            resolve_home(
                Some(release.clone().into_os_string()),
                Some(user.clone()),
                Some(RELEASE_HOME_DIR_NAME)
            ),
            Some(own.clone()),
            "a pane of the installed app exports its home; a dev build falls back to its own"
        );
        let mut release_with_separator = release.clone().into_os_string();
        release_with_separator.push(std::path::MAIN_SEPARATOR_STR);
        assert_eq!(
            resolve_home(
                Some(release_with_separator),
                Some(user.clone()),
                Some(RELEASE_HOME_DIR_NAME)
            ),
            Some(own.clone())
        );
        assert_eq!(
            resolve_home(
                Some(isolated.clone().into_os_string()),
                Some(user.clone()),
                Some(RELEASE_HOME_DIR_NAME)
            ),
            Some(isolated),
            "an explicit isolated home is honored"
        );
        assert_eq!(
            resolve_home(
                Some(release.clone().into_os_string()),
                Some(user.clone()),
                None
            ),
            Some(release),
            "a release build keeps honoring its own home"
        );
        assert_eq!(
            resolve_home(
                Some("relative/home".into()),
                Some(user.clone()),
                Some(RELEASE_HOME_DIR_NAME)
            ),
            Some(own)
        );
        assert_eq!(resolve_home(None, None, Some(RELEASE_HOME_DIR_NAME)), None);
    }

    #[test]
    fn an_isolated_home_owns_an_ipc_endpoint_the_default_one_does_not_share() {
        let first = Path::new("/tmp/paneflow-dev-alpha");
        let second = Path::new("/tmp/paneflow-dev-beta");
        assert_ne!(ipc_endpoint_path(first), ipc_endpoint_path(second));
        assert_ne!(ipc_endpoint_path(first), host_endpoint_path(first));
        assert_ne!(ipc_endpoint_path(first), serve_endpoint_path(first));
        assert!(
            ipc_endpoint_path(first)
                .to_string_lossy()
                .contains(&home_fingerprint(first))
        );
        assert!(!is_default_home(first));
        if let Some(user) = dirs::home_dir() {
            assert!(is_default_home(&user.join(HOME_DIR_NAME)));
        }
    }

    #[test]
    fn distinct_homes_have_distinct_fingerprints_and_a_home_has_one() {
        let normal = home_fingerprint(Path::new("/home/arthur/.paneflow"));
        let dev = home_fingerprint(Path::new("/home/arthur/.paneflow-dev"));
        let isolated = home_fingerprint(Path::new("/tmp/paneflow-exercise"));
        assert_eq!(normal.len(), 16);
        assert_ne!(normal, dev);
        assert_ne!(normal, isolated);
        assert_ne!(dev, isolated);
        assert_eq!(
            home_fingerprint(Path::new("/home/arthur/.paneflow/")),
            normal,
            "a trailing separator does not change the home"
        );
        assert_eq!(
            home_fingerprint(Path::new("/home/arthur/.paneflow")),
            home_fingerprint(Path::new("/home/arthur/.paneflow")),
        );
        if cfg!(windows) {
            assert_eq!(
                home_fingerprint(Path::new(r"C:\Users\Arthur\.paneflow")),
                home_fingerprint(Path::new("c:/users/arthur/.paneflow")),
            );
        }
    }

    #[test]
    fn only_a_dev_build_reserves_the_release_host_endpoint() {
        let reserved = reserved_host_endpoint();
        if cfg!(debug_assertions) {
            let user = dirs::home_dir().expect("user home");
            assert_eq!(
                reserved,
                Some(host_endpoint_path(&user.join(RELEASE_HOME_DIR_NAME)))
            );
            assert_ne!(
                reserved,
                Some(host_endpoint_path(&user.join(HOME_DIR_NAME)))
            );
        } else {
            assert_eq!(reserved, None);
        }
    }
}
