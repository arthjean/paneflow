use std::path::{Path, PathBuf};

pub(crate) fn implicit_launch_cwd() -> PathBuf {
    resolve_implicit_launch_cwd(std::env::current_dir().ok(), dirs::home_dir())
}

pub(crate) fn title_for_cwd_or(cwd: &Path, fallback: impl Into<String>) -> String {
    cwd.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| fallback.into())
}

fn resolve_implicit_launch_cwd(current: Option<PathBuf>, home: Option<PathBuf>) -> PathBuf {
    match current {
        Some(current) if is_filesystem_root(&current) => home.unwrap_or(current),
        Some(current) => current,
        None => home.unwrap_or_else(|| PathBuf::from(std::path::MAIN_SEPARATOR.to_string())),
    }
}

pub(crate) fn is_filesystem_root(path: &Path) -> bool {
    path.has_root() && path.parent().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn implicit_launch_cwd_uses_home_when_current_is_root() {
        let root = platform_root();
        let home = child_of_root("home");

        assert_eq!(
            resolve_implicit_launch_cwd(Some(root), Some(home.clone())),
            home
        );
    }

    #[test]
    fn implicit_launch_cwd_keeps_non_root_current() {
        let current = child_of_root("project");
        let home = child_of_root("home");

        assert_eq!(
            resolve_implicit_launch_cwd(Some(current.clone()), Some(home)),
            current
        );
    }

    #[test]
    fn implicit_launch_cwd_uses_home_when_current_is_missing() {
        let home = child_of_root("home");

        assert_eq!(resolve_implicit_launch_cwd(None, Some(home.clone())), home);
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
