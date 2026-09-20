use std::fs;
use std::io;
use std::path::Path;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use paneflow_host::hook_assets::{
    AssetLock, CANCELLATION_FILE, Cancellation, lock_exclusive, read_cancellation,
};
use paneflow_host::hook_assets::{MAX_MARKER_BYTES, read_capped};
pub use paneflow_ipc_client::ai_hook::{BACKGROUND_DIR, is_safe_activity_id};

pub const SEED_FILE: &str = "last-hook-event.json";
pub const EXPIRY_FILE: &str = "hook-expiry.json";

pub const MAX_SEED_BYTES: u64 = 64 * 1024;
const MAX_BACKGROUND_MARKER_BYTES: u64 = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedEvent {
    pub hook_event_name: String,
    pub tool_name: Option<String>,
    pub notification_type: Option<String>,
    pub runtime_generation: Option<u64>,
    pub modified_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundActivityMarker {
    pub id: String,
    pub started_at: SystemTime,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackgroundActivity {
    pub markers: Vec<BackgroundActivityMarker>,
    pub changed_at: Option<SystemTime>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
struct Expiry {
    runtime_generation: u64,
    through_ms: u64,
}

pub fn read_last_hook_event(session_dir: &Path) -> Option<SeedEvent> {
    let path = session_dir.join(SEED_FILE);
    let (bytes, modified_at) = read_capped(&path, MAX_SEED_BYTES)?;
    let value = match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) => value,
        Err(error) => {
            log::warn!(
                "paneflow-serve: {} is not valid JSON and is ignored: {error}",
                path.display()
            );
            return None;
        }
    };
    let hook_event_name = value
        .get("hook_event_name")
        .and_then(Value::as_str)?
        .to_owned();
    Some(SeedEvent {
        hook_event_name,
        tool_name: value
            .get("tool_name")
            .and_then(Value::as_str)
            .map(str::to_owned),
        notification_type: value
            .get("notification_type")
            .and_then(Value::as_str)
            .map(str::to_owned),
        runtime_generation: value.get("runtime_generation").and_then(Value::as_u64),
        modified_at,
    })
}

fn read_expiry(path: &Path) -> Option<Expiry> {
    let (bytes, _) = read_capped(path, MAX_MARKER_BYTES)?;
    serde_json::from_slice(&bytes).ok()
}

pub fn hook_turn_expired(session_dir: &Path, generation: u64, event_at: SystemTime) -> bool {
    read_expiry(&session_dir.join(EXPIRY_FILE)).is_some_and(|expiry| {
        expiry.runtime_generation == generation
            && crate::hook_state::unix_ms(event_at) <= expiry.through_ms
    })
}

pub fn record_hook_expiry(
    session_dir: &Path,
    generation: u64,
    through: SystemTime,
) -> io::Result<()> {
    if !session_dir.is_dir() {
        return Ok(());
    }
    let path = session_dir.join(EXPIRY_FILE);
    let _lock = lock_exclusive(&path)?;
    let through_ms = crate::hook_state::unix_ms(through);
    if read_expiry(&path).is_some_and(|held| {
        held.runtime_generation > generation
            || (held.runtime_generation == generation && held.through_ms >= through_ms)
    }) {
        return Ok(());
    }
    let bytes = serde_json::to_vec(&Expiry {
        runtime_generation: generation,
        through_ms,
    })
    .map_err(io::Error::other)?;
    paneflow_host::manifest::write_atomically(&path, &bytes)
}

pub fn read_background_activity(
    session_dir: &Path,
    generation: u64,
) -> io::Result<BackgroundActivity> {
    let directory =
        paneflow_ipc_client::ai_hook::background_generation_dir(session_dir, generation);
    let changed_at = fs::metadata(&directory)
        .and_then(|metadata| metadata.modified())
        .ok();
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(BackgroundActivity::default());
        }
        Err(error) => return Err(error),
    };
    let mut markers = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json")
            || !entry.file_type()?.is_file()
        {
            continue;
        }
        let Some((bytes, started_at)) = read_capped(&path, MAX_BACKGROUND_MARKER_BYTES) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        let Some(id) = value.get("activity_id").and_then(Value::as_str) else {
            continue;
        };
        if !is_safe_activity_id(id)
            || path.file_stem().and_then(|stem| stem.to_str()) != Some(id)
            || value.get("runtime_generation").and_then(Value::as_u64) != Some(generation)
        {
            continue;
        }
        markers.push(BackgroundActivityMarker {
            id: id.to_owned(),
            started_at,
        });
    }
    Ok(BackgroundActivity {
        markers,
        changed_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write(dir: &Path, name: &str, value: &Value) {
        fs::write(dir.join(name), serde_json::to_vec(value).unwrap()).unwrap();
    }

    #[test]
    fn a_seed_is_read_with_its_own_mtime_and_refused_when_oversized_or_unparsable() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_last_hook_event(dir.path()), None);

        write(
            dir.path(),
            SEED_FILE,
            &json!({"hook_event_name": "UserPromptSubmit", "runtime_generation": 3}),
        );
        let seed = read_last_hook_event(dir.path()).expect("a valid seed is read");
        assert_eq!(seed.hook_event_name, "UserPromptSubmit");
        assert_eq!(seed.runtime_generation, Some(3));

        fs::write(dir.path().join(SEED_FILE), b"{not json").unwrap();
        assert_eq!(read_last_hook_event(dir.path()), None);

        fs::write(
            dir.path().join(SEED_FILE),
            vec![b'x'; MAX_SEED_BYTES as usize + 1],
        )
        .unwrap();
        assert_eq!(read_last_hook_event(dir.path()), None);
    }

    #[test]
    fn an_expiry_watermark_is_generation_scoped_and_never_moves_backwards() {
        let dir = tempfile::tempdir().unwrap();
        let through = crate::hook_state::at_unix_ms(5_000);
        record_hook_expiry(dir.path(), 2, through).unwrap();
        assert!(hook_turn_expired(dir.path(), 2, through));
        assert!(hook_turn_expired(
            dir.path(),
            2,
            crate::hook_state::at_unix_ms(4_000)
        ));
        assert!(!hook_turn_expired(
            dir.path(),
            2,
            crate::hook_state::at_unix_ms(6_000)
        ));
        assert!(
            !hook_turn_expired(dir.path(), 3, through),
            "a watermark never fences a generation it does not name"
        );

        record_hook_expiry(dir.path(), 2, crate::hook_state::at_unix_ms(1_000)).unwrap();
        assert_eq!(
            read_expiry(&dir.path().join(EXPIRY_FILE))
                .map(|expiry| expiry.through_ms)
                .unwrap(),
            5_000
        );
    }

    #[test]
    fn background_markers_need_a_safe_identity_a_matching_name_and_the_current_generation() {
        let dir = tempfile::tempdir().unwrap();
        let markers = dir.path().join(BACKGROUND_DIR).join("3");
        fs::create_dir_all(&markers).unwrap();
        for (file, id, generation) in [
            ("child", "child", 3),
            ("stale", "stale", 2),
            ("renamed", "mismatch", 3),
            ("unsafe", "../escape", 3),
        ] {
            fs::write(
                markers.join(format!("{file}.json")),
                serde_json::to_vec(&json!({"activity_id": id, "runtime_generation": generation}))
                    .unwrap(),
            )
            .unwrap();
        }
        fs::write(markers.join("partial.json"), b"{").unwrap();

        let live = read_background_activity(dir.path(), 3).unwrap();
        assert_eq!(live.markers.len(), 1);
        assert_eq!(live.markers[0].id, "child");
        assert!(live.changed_at.is_some());
        assert!(
            read_background_activity(dir.path(), 4)
                .unwrap()
                .markers
                .is_empty()
        );
        let absent = read_background_activity(dir.path(), 9).unwrap();
        assert!(
            absent.markers.is_empty() && absent.changed_at.is_none(),
            "a generation with no directory is empty, not an error"
        );
    }
}
