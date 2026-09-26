use std::path::{Path, PathBuf};

pub const EXE_SUFFIX: &str = if cfg!(windows) { ".exe" } else { "" };

pub const AI_HOOK_STEM: &str = "paneflow-ai-hook";

pub const HELPER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("no host-local {wanted} was found; searched {}", .searched.join(", "))]
pub struct HelperMissing {
    pub wanted: String,
    pub searched: Vec<String>,
}

pub fn file_name(stem: &str) -> String {
    format!("{stem}{EXE_SUFFIX}")
}

pub fn helper_candidates(host_exe: Option<&Path>, home: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let exe_dir = host_exe.and_then(Path::parent);
    if let Some(dir) = exe_dir {
        candidates.push(dir.join("bin"));
    }
    candidates.push(versioned_helper_root(home).join(HELPER_VERSION));
    if let Some(dir) = exe_dir {
        candidates.push(dir.to_path_buf());
    }
    candidates.push(home.join("bin"));
    candidates.dedup();
    candidates
}

fn first_dir_with(candidates: &[PathBuf], wanted: &str) -> Result<PathBuf, HelperMissing> {
    for candidate in candidates {
        if candidate.join(wanted).is_file() {
            return Ok(candidate.clone());
        }
    }
    Err(HelperMissing {
        wanted: wanted.to_string(),
        searched: candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
    })
}

pub fn resolve_hook_dir(host_exe: Option<&Path>, home: &Path) -> Result<PathBuf, HelperMissing> {
    first_dir_with(&helper_candidates(host_exe, home), &file_name(AI_HOOK_STEM))
}

pub fn current_hook_dir(home: &Path) -> Result<PathBuf, HelperMissing> {
    resolve_hook_dir(std::env::current_exe().ok().as_deref(), home)
}

fn versioned_helper_root(home: &Path) -> PathBuf {
    home.join("cache").join("bin")
}

pub fn without_helper_dirs(path: &str, home: &Path, retired: &[&Path]) -> Option<String> {
    let versioned_root = versioned_helper_root(home);
    let kept: Vec<PathBuf> = std::env::split_paths(path)
        .filter(|component| {
            component.parent() != Some(versioned_root.as_path())
                && !retired.iter().any(|dir| component == dir)
        })
        .collect();
    std::env::join_paths(kept)
        .ok()
        .map(|joined| joined.to_string_lossy().into_owned())
}

pub fn prepend_to_path(existing: Option<&str>, dir: &Path) -> Option<String> {
    let mut components: Vec<PathBuf> = vec![dir.to_path_buf()];
    if let Some(existing) = existing.filter(|value| !value.is_empty()) {
        components.extend(std::env::split_paths(existing));
    }
    std::env::join_paths(components)
        .ok()
        .map(|joined| joined.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, b"helper").unwrap();
    }

    #[test]
    fn the_packaged_directory_next_to_the_host_wins_over_the_state_home() {
        let root = tempfile::tempdir().unwrap();
        let exe = root.path().join("app").join(file_name("paneflow-host"));
        touch(&exe);
        let packaged = root.path().join("app").join("bin");
        touch(&packaged.join(file_name(AI_HOOK_STEM)));
        let home = root.path().join("home");
        touch(&home.join("bin").join(file_name(AI_HOOK_STEM)));

        assert_eq!(resolve_hook_dir(Some(&exe), &home).unwrap(), packaged);
    }

    #[test]
    fn the_state_home_answers_when_nothing_ships_beside_the_host() {
        let root = tempfile::tempdir().unwrap();
        let exe = root.path().join("app").join(file_name("paneflow-host"));
        touch(&exe);
        let home = root.path().join("home");
        let cached = home.join("cache").join("bin").join(HELPER_VERSION);
        touch(&cached.join(file_name(AI_HOOK_STEM)));

        assert_eq!(resolve_hook_dir(Some(&exe), &home).unwrap(), cached);
    }

    #[test]
    fn a_missing_helper_names_every_place_that_was_searched() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let error = resolve_hook_dir(None, &home).unwrap_err();
        assert_eq!(error.wanted, file_name(AI_HOOK_STEM));
        assert_eq!(error.searched.len(), 2);
        assert!(error.to_string().contains("searched"));
    }

    #[test]
    fn the_versioned_shim_directory_wins_over_the_hook_only_state_directory() {
        let root = tempfile::tempdir().unwrap();
        let exe = root.path().join("usr").join(file_name("paneflow-host"));
        touch(&exe);
        let home = root.path().join("home");
        touch(&home.join("bin").join(file_name(AI_HOOK_STEM)));
        let versioned = home.join("cache").join("bin").join(HELPER_VERSION);
        touch(&versioned.join(file_name(AI_HOOK_STEM)));

        assert_eq!(resolve_hook_dir(Some(&exe), &home).unwrap(), versioned);
    }

    #[test]
    fn every_versioned_helper_directory_and_each_retired_one_leave_the_path() {
        let home = Path::new("/home/u/.paneflow");
        let old = home.join("cache").join("bin").join("0.16.0");
        let retired = PathBuf::from("/opt/old-paneflow/bin");
        let path = std::env::join_paths([
            old.as_path(),
            Path::new("/usr/bin"),
            retired.as_path(),
            home.join("cache").as_path(),
        ])
        .unwrap();
        let pruned = without_helper_dirs(&path.to_string_lossy(), home, &[&retired]).unwrap();
        assert_eq!(
            std::env::split_paths(&pruned).collect::<Vec<_>>(),
            vec![PathBuf::from("/usr/bin"), home.join("cache")]
        );
    }

    #[test]
    fn the_helper_directory_leads_the_child_path() {
        let dir = Path::new("/opt/paneflow/bin");
        let joined = prepend_to_path(Some("/usr/bin"), dir).unwrap();
        let mut parts = std::env::split_paths(&joined);
        assert_eq!(parts.next().unwrap(), dir);
        assert_eq!(parts.next().unwrap(), PathBuf::from("/usr/bin"));
        assert_eq!(
            std::env::split_paths(&prepend_to_path(None, dir).unwrap()).count(),
            1
        );
    }
}
