use std::path::{Path, PathBuf};

const ENDPOINT_PREFIX: &str = "paneflow-host-";

#[cfg(windows)]
pub fn host_endpoint_path(home: &Path) -> PathBuf {
    PathBuf::from(format!(
        r"\\.\pipe\{ENDPOINT_PREFIX}{}",
        paneflow_home::home_fingerprint(home)
    ))
}

#[cfg(unix)]
pub fn host_endpoint_path(home: &Path) -> PathBuf {
    runtime_dir().join(format!(
        "{ENDPOINT_PREFIX}{}.sock",
        paneflow_home::home_fingerprint(home)
    ))
}

#[cfg(unix)]
fn runtime_dir() -> PathBuf {
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
    paneflow_home::paneflow_home().map(|home| host_endpoint_path(&home))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolated_and_normal_homes_get_distinct_endpoints() {
        let normal = host_endpoint_path(Path::new("/home/arthur/.paneflow"));
        let dev = host_endpoint_path(Path::new("/home/arthur/.paneflow-dev"));
        let exercise = host_endpoint_path(Path::new("/tmp/paneflow-exercise-home"));
        assert_ne!(normal, dev);
        assert_ne!(normal, exercise);
        assert_ne!(dev, exercise);
        assert!(normal.to_string_lossy().contains(ENDPOINT_PREFIX));
        assert_eq!(
            normal,
            host_endpoint_path(Path::new("/home/arthur/.paneflow"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_endpoints_fit_the_sun_path_limit() {
        let path = host_endpoint_path(Path::new("/home/arthur/.paneflow"));
        assert!(path.as_os_str().len() < 104, "{}", path.display());
    }
}
