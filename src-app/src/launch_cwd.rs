use std::path::Path;

pub(crate) use paneflow_launch::cwd::{implicit_launch_cwd, is_filesystem_root};

pub(crate) fn title_for_cwd_or(cwd: &Path, fallback: impl Into<String>) -> String {
    cwd.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| fallback.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn platform_root() -> PathBuf {
        std::env::current_dir()
            .ok()
            .and_then(|path| path.ancestors().last().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from(std::path::MAIN_SEPARATOR.to_string()))
    }

    fn child_of_root(name: &str) -> PathBuf {
        let mut path = platform_root();
        path.push(name);
        path
    }

    #[test]
    fn title_for_cwd_or_uses_last_path_component() {
        let cwd = child_of_root("paneflow");

        assert_eq!(title_for_cwd_or(&cwd, "Terminal 1"), "paneflow");
    }

    #[test]
    fn title_for_cwd_or_falls_back_for_root() {
        let root = platform_root();

        assert_eq!(title_for_cwd_or(&root, "Terminal 1"), "Terminal 1");
    }
}
