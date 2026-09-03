use std::io::{self, Write};
use std::path::Path;

use tempfile::NamedTempFile;
use uuid::Uuid;

const TELEMETRY_ID_FILE: &str = "telemetry_id";

enum ReadState {
    Found(String),
    Missing,
    Unusable(String),
}

pub fn telemetry_id_at(base: &Path) -> (String, bool) {
    let file = base.join(TELEMETRY_ID_FILE);
    match read_persisted_id(&file) {
        ReadState::Found(id) => (id, false),
        ReadState::Unusable(reason) => (ephemeral_id(&reason), false),
        ReadState::Missing => initialize_id(base, &file),
    }
}

pub fn ephemeral_id(reason: &str) -> String {
    log::debug!("paneflow: telemetry running session-scoped ({reason})");
    Uuid::new_v4().to_string()
}

fn read_persisted_id(file: &Path) -> ReadState {
    match std::fs::read_to_string(file) {
        Ok(contents) => {
            let trimmed = contents.trim();
            match Uuid::parse_str(trimmed) {
                Ok(id) if id.get_version_num() == 4 => ReadState::Found(id.to_string()),
                _ => ReadState::Unusable(format!(
                    "telemetry_id file {} did not contain a UUID v4",
                    file.display()
                )),
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => ReadState::Missing,
        Err(error) => ReadState::Unusable(format!(
            "could not read telemetry_id at {} ({error})",
            file.display()
        )),
    }
}

fn initialize_id(base: &Path, file: &Path) -> (String, bool) {
    let fresh = Uuid::new_v4().to_string();
    let mut temporary = match NamedTempFile::new_in(base) {
        Ok(temporary) => temporary,
        Err(error) => {
            log_persist_failure(file, &error);
            return (fresh, false);
        }
    };
    if let Err(error) = temporary
        .write_all(fresh.as_bytes())
        .and_then(|()| temporary.as_file().sync_all())
    {
        log_persist_failure(file, &error);
        return (fresh, false);
    }

    match temporary.persist_noclobber(file) {
        Ok(_) => (fresh, true),
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
            match read_persisted_id(file) {
                ReadState::Found(id) => (id, false),
                ReadState::Missing => {
                    log_persist_failure(file, &error.error);
                    (fresh, false)
                }
                ReadState::Unusable(reason) => (ephemeral_id(&reason), false),
            }
        }
        Err(error) => {
            log_persist_failure(file, &error.error);
            (fresh, false)
        }
    }
}

fn log_persist_failure(file: &Path, error: &io::Error) {
    log::debug!(
        "paneflow: could not persist telemetry_id at {} ({error}); using ephemeral id for this session",
        file.display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Arc, Barrier};
    use tempfile::TempDir;

    fn parses_as_v4_uuid(value: &str) -> bool {
        Uuid::parse_str(value).is_ok_and(|id| id.get_version_num() == 4)
    }

    #[test]
    fn first_call_publishes_complete_v4_uuid() {
        let dir = TempDir::new().unwrap();
        let (id, first_run) = telemetry_id_at(dir.path());
        assert!(parses_as_v4_uuid(&id));
        assert!(first_run);
        assert_eq!(
            fs::read_to_string(dir.path().join(TELEMETRY_ID_FILE))
                .unwrap()
                .trim(),
            id
        );
    }

    #[test]
    fn subsequent_call_returns_canonical_persisted_id() {
        let dir = TempDir::new().unwrap();
        let (first_id, first_run) = telemetry_id_at(dir.path());
        let (second_id, second_run) = telemetry_id_at(dir.path());
        assert_eq!(first_id, second_id);
        assert!(first_run);
        assert!(!second_run);
    }

    #[test]
    fn concurrent_initializers_converge_on_one_id_and_one_first_run() {
        const THREADS: usize = 8;
        let dir = TempDir::new().unwrap();
        let base = Arc::new(dir.path().to_path_buf());
        let barrier = Arc::new(Barrier::new(THREADS));
        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let base = Arc::clone(&base);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    telemetry_id_at(&base)
                })
            })
            .collect();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();

        assert!(results.iter().all(|(id, _)| id == &results[0].0));
        assert_eq!(results.iter().filter(|(_, first)| *first).count(), 1);
    }

    #[test]
    fn corrupt_file_yields_ephemeral_and_is_preserved() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join(TELEMETRY_ID_FILE);
        fs::write(&file, "not-a-uuid-garbage").unwrap();
        let (id, first_run) = telemetry_id_at(dir.path());
        assert!(parses_as_v4_uuid(&id));
        assert!(!first_run);
        assert_eq!(fs::read_to_string(file).unwrap(), "not-a-uuid-garbage");
    }

    #[test]
    fn non_v4_uuid_is_rejected_and_preserved() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join(TELEMETRY_ID_FILE);
        let nil = Uuid::nil().to_string();
        fs::write(&file, &nil).unwrap();
        let (id, first_run) = telemetry_id_at(dir.path());
        assert!(parses_as_v4_uuid(&id));
        assert!(!first_run);
        assert_eq!(fs::read_to_string(file).unwrap(), nil);
    }

    #[test]
    fn missing_directory_yields_ephemeral_without_creating_parent() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("nope");
        let (id, first_run) = telemetry_id_at(&missing);
        assert!(parses_as_v4_uuid(&id));
        assert!(!first_run);
        assert!(!missing.exists());
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_but_writable_existing_file_is_not_overwritten() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let file = dir.path().join(TELEMETRY_ID_FILE);
        let seed = Uuid::new_v4().to_string();
        fs::write(&file, &seed).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o200)).unwrap();

        let (id, first_run) = telemetry_id_at(dir.path());
        assert!(parses_as_v4_uuid(&id));
        assert!(!first_run);

        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(fs::read_to_string(file).unwrap(), seed);
    }

    #[test]
    fn ephemeral_id_is_a_v4_uuid() {
        assert!(parses_as_v4_uuid(&ephemeral_id("test")));
    }
}
