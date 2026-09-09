use std::path::{Path, PathBuf};

pub const HOME_DIR_NAME: &str = if cfg!(debug_assertions) {
    ".paneflow-dev"
} else {
    ".paneflow"
};

pub const HOME_ENV: &str = "PANEFLOW_HOME";

const LEGACY_SUBDIR: &str = if cfg!(debug_assertions) {
    "paneflow-dev"
} else {
    "paneflow"
};

pub fn paneflow_home() -> Option<PathBuf> {
    if let Some(raw) = std::env::var_os(HOME_ENV) {
        let path = PathBuf::from(raw);
        if path.is_absolute() {
            return Some(path);
        }
    }
    dirs::home_dir().map(|home| home.join(HOME_DIR_NAME))
}

pub fn config_path() -> Option<PathBuf> {
    paneflow_home().map(|home| home.join("paneflow.json"))
}

pub fn session_path() -> Option<PathBuf> {
    paneflow_home().map(|home| home.join("session.json"))
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
    }
}
