use std::path::Path;

use semver::Version;

const MARKER_FILENAME: &str = "last-launched-version";
const CHANGELOG_BASE_URL: &str = "https://paneflow.dev/docs/changelog";

pub fn changelog_url(version: &str) -> String {
    format!("{CHANGELOG_BASE_URL}/v{version}")
}

pub fn upgraded_version() -> Option<String> {
    let marker_path = crate::runtime_paths::cache_dir()?.join(MARKER_FILENAME);
    record_launch(&marker_path, env!("CARGO_PKG_VERSION"))
}

fn record_launch(marker_path: &Path, current: &str) -> Option<String> {
    let previous = std::fs::read_to_string(marker_path).ok();
    write_marker(marker_path, current);
    is_upgrade(previous.as_deref(), current).then(|| current.to_string())
}

fn write_marker(marker_path: &Path, current: &str) {
    if let Some(parent) = marker_path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        log::warn!(
            "paneflow: cannot create cache dir {} ({err}); the release toast may repeat next launch",
            parent.display()
        );
        return;
    }
    if let Err(err) = std::fs::write(marker_path, current.as_bytes()) {
        log::warn!(
            "paneflow: cannot write version marker {} ({err}); the release toast may repeat next launch",
            marker_path.display()
        );
    }
}

fn is_upgrade(previous: Option<&str>, current: &str) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    let (Ok(previous), Ok(current)) = (Version::parse(previous.trim()), Version::parse(current))
    else {
        return false;
    };
    current > previous
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changelog_url_points_at_the_tag_page() {
        assert_eq!(
            changelog_url("0.14.2"),
            "https://paneflow.dev/docs/changelog/v0.14.2"
        );
    }

    #[test]
    fn a_first_launch_records_the_version_without_announcing_it() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let marker = tmp.path().join("cache").join(MARKER_FILENAME);

        assert_eq!(record_launch(&marker, "0.14.2"), None);
        assert_eq!(
            std::fs::read_to_string(&marker).expect("marker written"),
            "0.14.2"
        );
    }

    #[test]
    fn a_newer_version_announces_once() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let marker = tmp.path().join(MARKER_FILENAME);
        std::fs::write(&marker, b"0.14.1").expect("seed marker");

        assert_eq!(record_launch(&marker, "0.14.2"), Some("0.14.2".to_string()));
        assert_eq!(record_launch(&marker, "0.14.2"), None);
    }

    #[test]
    fn a_trailing_newline_in_the_marker_still_compares() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let marker = tmp.path().join(MARKER_FILENAME);
        std::fs::write(&marker, b"0.14.1\n").expect("seed marker");

        assert_eq!(record_launch(&marker, "0.14.2"), Some("0.14.2".to_string()));
    }

    #[test]
    fn a_downgrade_does_not_announce() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let marker = tmp.path().join(MARKER_FILENAME);
        std::fs::write(&marker, b"0.15.0").expect("seed marker");

        assert_eq!(record_launch(&marker, "0.14.2"), None);
        assert_eq!(
            std::fs::read_to_string(&marker).expect("marker rewritten"),
            "0.14.2"
        );
    }

    #[test]
    fn an_unparsable_marker_does_not_announce() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let marker = tmp.path().join(MARKER_FILENAME);
        std::fs::write(&marker, b"nightly").expect("seed marker");

        assert_eq!(record_launch(&marker, "0.14.2"), None);
    }
}
