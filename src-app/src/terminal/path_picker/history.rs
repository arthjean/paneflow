use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

const MAX_ENTRIES: usize = 50;

static HISTORY: Mutex<Option<Vec<PathBuf>>> = Mutex::new(None);

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct HistoryFile {
    #[serde(default)]
    paths: Vec<PathBuf>,
}

pub(super) fn recent() -> Vec<PathBuf> {
    let mut history = lock();
    history.get_or_insert_with(load).clone()
}

pub(super) fn record(path: PathBuf) {
    let mut history = lock();
    let paths = history.get_or_insert_with(load);
    remember(paths, path);
    if let Some(file) = paneflow_home::path_history_path() {
        write(&file, paths);
    }
}

fn lock() -> MutexGuard<'static, Option<Vec<PathBuf>>> {
    HISTORY.lock().unwrap_or_else(PoisonError::into_inner)
}

fn load() -> Vec<PathBuf> {
    paneflow_home::path_history_path()
        .map(|file| read(&file))
        .unwrap_or_default()
}

fn remember(paths: &mut Vec<PathBuf>, path: PathBuf) {
    paths.retain(|known| known != &path);
    paths.insert(0, path);
    paths.truncate(MAX_ENTRIES);
}

fn read(file: &Path) -> Vec<PathBuf> {
    let Ok(raw) = std::fs::read_to_string(file) else {
        return Vec::new();
    };
    match serde_json::from_str::<HistoryFile>(&raw) {
        Ok(parsed) => parsed.paths,
        Err(error) => {
            log::warn!(
                "path history: {} is not valid JSON ({error}), starting empty",
                file.display()
            );
            Vec::new()
        }
    }
}

fn write(file: &Path, paths: &[PathBuf]) {
    if let Some(parent) = file.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        log::warn!(
            "path history: could not create {}: {error}",
            parent.display()
        );
        return;
    }
    let contents = HistoryFile {
        paths: paths.to_vec(),
    };
    match serde_json::to_string_pretty(&contents) {
        Ok(json) => {
            if let Err(error) = std::fs::write(file, json) {
                log::warn!("path history: could not write {}: {error}", file.display());
            }
        }
        Err(error) => log::warn!("path history: could not serialize: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembering_moves_a_path_to_the_front_without_duplicates() {
        let mut paths = vec![PathBuf::from("a"), PathBuf::from("b"), PathBuf::from("c")];
        remember(&mut paths, PathBuf::from("b"));
        assert_eq!(
            paths,
            [PathBuf::from("b"), PathBuf::from("a"), PathBuf::from("c")]
        );
    }

    #[test]
    fn remembering_keeps_only_the_newest_entries() {
        let mut paths = Vec::new();
        for index in 0..MAX_ENTRIES + 5 {
            remember(&mut paths, PathBuf::from(index.to_string()));
        }
        assert_eq!(paths.len(), MAX_ENTRIES);
        assert_eq!(paths[0], PathBuf::from((MAX_ENTRIES + 4).to_string()));
    }

    #[test]
    fn the_file_round_trips_and_tolerates_absence_and_garbage() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("nested").join("path-history.json");
        assert!(read(&file).is_empty());
        let paths = vec![dir.path().join("one"), dir.path().join("two")];
        write(&file, &paths);
        assert_eq!(read(&file), paths);
        std::fs::write(&file, "not json").expect("garbage");
        assert!(read(&file).is_empty());
    }
}
