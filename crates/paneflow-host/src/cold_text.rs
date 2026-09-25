use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use paneflow_config::schema::SessionId;

pub const FINAL_OUTPUT_FILE: &str = "final-output.txt";

pub const COLD_TEXT_BUDGET_BYTES: u64 = 64 * 1024 * 1024;

pub fn path(home: &Path, session: &SessionId) -> PathBuf {
    paneflow_home::host_session_data_dir_in(home, session.as_str()).join(FINAL_OUTPUT_FILE)
}

pub fn write(home: &Path, session: &SessionId, text: &str) -> io::Result<u64> {
    let path = path(home, session);
    let directory = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "the cold text path has no parent",
        )
    })?;
    if !directory.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "the session data directory {} is gone; the final output is not written",
                directory.display()
            ),
        ));
    }
    crate::manifest::write_atomically_in_existing_dir(&path, text.as_bytes())?;
    Ok(text.len() as u64)
}

pub fn read(home: &Path, session: &SessionId) -> Option<String> {
    let path = path(home, session);
    let bytes = std::fs::read(&path).ok()?;
    if bytes.len() > crate::runtime::FINAL_TEXT_MAX_BYTES {
        return None;
    }
    String::from_utf8(bytes).ok()
}

struct ColdFile {
    path: PathBuf,
    bytes: u64,
    modified: SystemTime,
}

fn cold_files(home: &Path) -> Vec<ColdFile> {
    let root = paneflow_home::host_session_data_root_in(home);
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path().join(FINAL_OUTPUT_FILE);
            let metadata = std::fs::metadata(&path).ok()?;
            Some(ColdFile {
                path,
                bytes: metadata.len(),
                modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            })
        })
        .collect()
}

pub fn enforce_budget(home: &Path, budget_bytes: u64) -> usize {
    let mut files = cold_files(home);
    let mut total: u64 = files.iter().map(|file| file.bytes).sum();
    if total <= budget_bytes {
        return 0;
    }
    files.sort_by_key(|file| file.modified);
    let mut evicted = 0;
    for file in files {
        if total <= budget_bytes {
            break;
        }
        match std::fs::remove_file(&file.path) {
            Ok(()) => {
                total = total.saturating_sub(file.bytes);
                evicted += 1;
            }
            Err(error) => log::warn!(
                "paneflow-host: cannot evict the final output {}: {error}",
                file.path.display()
            ),
        }
    }
    evicted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(home: &Path, session: &SessionId, text: &str) {
        std::fs::create_dir_all(paneflow_home::host_session_data_dir_in(
            home,
            session.as_str(),
        ))
        .unwrap();
        write(home, session, text).unwrap();
    }

    #[test]
    fn eviction_drops_the_oldest_final_output_first_and_keeps_identities() {
        let home = tempfile::tempdir().unwrap();
        let old = SessionId::new();
        let new = SessionId::new();
        seed(home.path(), &old, &"o".repeat(600));
        let old_path = path(home.path(), &old);
        let earlier = SystemTime::now() - std::time::Duration::from_secs(120);
        let file = std::fs::File::options()
            .write(true)
            .open(&old_path)
            .unwrap();
        file.set_modified(earlier).unwrap();
        drop(file);
        seed(home.path(), &new, &"n".repeat(600));
        assert_eq!(enforce_budget(home.path(), 1_000), 1);
        assert_eq!(read(home.path(), &old), None);
        assert_eq!(read(home.path(), &new).unwrap().len(), 600);
        assert!(
            paneflow_home::host_session_data_dir_in(home.path(), old.as_str()).is_dir(),
            "evicting text never deletes the session identity"
        );
    }

    #[test]
    fn a_missing_data_directory_refuses_the_write_instead_of_recreating_it() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let error = write(home.path(), &session, "late").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(!paneflow_home::host_session_data_dir_in(home.path(), session.as_str()).exists());
    }
}
