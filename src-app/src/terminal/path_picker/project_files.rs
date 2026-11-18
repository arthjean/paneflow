use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use super::fuzzy::{self, Pattern};
use super::index::{self, LOCAL_CAP, Located};

pub(crate) const PROJECT_FILES_CAP: usize = LOCAL_CAP;

pub(crate) struct ProjectFiles {
    pub(crate) root: PathBuf,
    pub(crate) paths: Vec<String>,
    pub(crate) complete: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectMatch {
    pub(crate) index: usize,
    pub(crate) score: i32,
    pub(crate) highlights: Vec<Range<usize>>,
}

pub(crate) fn walk_project_files(root: &Path, cancelled: &AtomicBool) -> Option<ProjectFiles> {
    let index = index::walk(root, PROJECT_FILES_CAP, &[], cancelled)?;
    let mut paths = Vec::new();
    let mut text = String::new();
    let mut chain = Vec::new();
    for node in 0..index.len() as u32 {
        if index.is_dir(node) {
            continue;
        }
        index.write_path(node, Located::Root, &mut text, &mut chain);
        paths.push(text.clone());
    }
    Some(ProjectFiles {
        root: index.root().to_path_buf(),
        paths,
        complete: index.complete(),
    })
}

pub(crate) fn rank_project_files(paths: &[String], query: &str, limit: usize) -> Vec<ProjectMatch> {
    let pattern = Pattern::new(query);
    let mut matches: Vec<ProjectMatch> = paths
        .iter()
        .enumerate()
        .filter(|(_, path)| !pattern.excluded_by(fuzzy::mask_of(path)))
        .filter_map(|(index, path)| {
            let found = fuzzy::match_path(path, &pattern)?;
            Some(ProjectMatch {
                index,
                score: found.score,
                highlights: found.highlights,
            })
        })
        .collect();
    matches.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| paths[left.index].len().cmp(&paths[right.index].len()))
            .then_with(|| paths[left.index].cmp(&paths[right.index]))
    });
    matches.truncate(limit);
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_walk_lists_the_files_of_unopened_folders_relative_to_the_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/deep/er")).expect("dirs");
        std::fs::write(root.join("src/deep/er/needle.rs"), "").expect("file");
        std::fs::write(root.join("README.md"), "").expect("file");
        let cancelled = AtomicBool::new(false);
        let files = walk_project_files(root, &cancelled).expect("walked");
        assert!(files.complete);
        let needle = Path::new("src")
            .join("deep")
            .join("er")
            .join("needle.rs")
            .to_string_lossy()
            .into_owned();
        assert!(files.paths.contains(&needle), "{:?}", files.paths);
        assert!(files.paths.contains(&"README.md".to_string()));
        assert!(
            !files.paths.iter().any(|path| path == "src"),
            "folders are not offered"
        );
    }

    #[test]
    fn ranking_keeps_the_best_matches_up_to_the_limit_with_their_highlights() {
        let paths: Vec<String> = (0..500)
            .map(|index| format!("src/module_{index}/view.rs"))
            .chain(["src/main.rs".to_string(), "docs/maintenance.md".to_string()])
            .collect();
        let ranked = rank_project_files(&paths, "main", 200);
        assert_eq!(paths[ranked[0].index], "src/main.rs");
        assert_eq!(ranked[0].highlights, vec![4..8]);
        assert!(ranked.len() <= 200);
        let ranked = rank_project_files(&paths, "view", 200);
        assert_eq!(ranked.len(), 200, "the list stops at the limit");
        assert!(rank_project_files(&paths, "zzzz", 200).is_empty());
    }
}
