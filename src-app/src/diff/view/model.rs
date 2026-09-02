use std::path::PathBuf;
use std::rc::Rc;

#[derive(Clone)]
pub struct DiffWorktree {
    pub path: PathBuf,
    pub branch: String,
    pub workspace_id: Option<u64>,
}

#[derive(Clone)]
pub struct FileEntry {
    pub path: String,
    pub change: super::super::git::FileChange,
    pub old_path: Option<String>,
    pub added: u32,
    pub removed: u32,
    pub is_binary: bool,
}

#[derive(Clone)]
pub enum FileListState {
    Loading,
    Loaded(Rc<Vec<FileEntry>>),
    Failed(String),
}

pub fn aggregate_file_lists(
    lists: &[(String, usize, PathBuf, FileListState)],
) -> (usize, usize, u32, u32) {
    lists
        .iter()
        .filter_map(|(_, _, _, state)| match state {
            FileListState::Loaded(files) if !files.is_empty() => Some(files),
            _ => None,
        })
        .fold((0usize, 0usize, 0u32, 0u32), |(b, fc, a, r), files| {
            let (file_added, file_removed) = files.iter().fold((0u32, 0u32), |(a, r), file| {
                (a + file.added, r + file.removed)
            });
            (b + 1, fc + files.len(), a + file_added, r + file_removed)
        })
}
