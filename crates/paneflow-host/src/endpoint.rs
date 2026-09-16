pub use paneflow_home::{host_endpoint_path, host_endpoint_path_for_current_home};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn isolated_and_normal_homes_get_distinct_endpoints() {
        let normal = host_endpoint_path(Path::new("/home/arthur/.paneflow"));
        let dev = host_endpoint_path(Path::new("/home/arthur/.paneflow-dev"));
        let exercise = host_endpoint_path(Path::new("/tmp/paneflow-exercise-home"));
        assert_ne!(normal, dev);
        assert_ne!(normal, exercise);
        assert_ne!(dev, exercise);
        assert!(normal.to_string_lossy().contains("paneflow-host-"));
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
