use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use paneflow_ipc_client::ai_hook::background_marker_path;
use serde_json::Value;

use crate::event::HookEvent;

pub(crate) fn record(
    event: HookEvent,
    session_dir: Option<&Path>,
    generation: Option<u64>,
    hook_payload: &Value,
) -> Result<(), String> {
    let (Some(session_dir), Some(generation)) = (session_dir, generation) else {
        return Ok(());
    };
    if !session_dir.is_dir() {
        return Ok(());
    }
    let Some(activity_id) = hook_payload.get("agent_id").and_then(Value::as_str) else {
        return Ok(());
    };
    let Some(path) = background_marker_path(session_dir, generation, activity_id) else {
        return Err(format!("{activity_id}: unusable background agent id"));
    };
    match event {
        HookEvent::SubagentStart => start(&path, activity_id, generation),
        HookEvent::SubagentStop => stop(&path),
        _ => Ok(()),
    }
}

fn start(path: &Path, activity_id: &str, generation: u64) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let marker = serde_json::json!({
        "activity_id": activity_id,
        "runtime_generation": generation,
    });
    let bytes = serde_json::to_vec(&marker).map_err(|error| error.to_string())?;
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => file.write_all(&bytes).map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn stop(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn markers(dir: &Path, generation: u64) -> Vec<String> {
        let path = paneflow_ipc_client::ai_hook::background_generation_dir(dir, generation);
        let Ok(entries) = fs::read_dir(path) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .filter_map(|entry| Some(entry.ok()?.file_name().to_string_lossy().into_owned()))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_child_is_created_on_start_and_only_that_child_is_removed_on_stop() {
        let dir = tempfile::tempdir().unwrap();
        record(
            HookEvent::SubagentStart,
            Some(dir.path()),
            Some(4),
            &json!({"agent_id": "explorer-1"}),
        )
        .unwrap();
        record(
            HookEvent::SubagentStart,
            Some(dir.path()),
            Some(4),
            &json!({"agent_id": "reviewer_2"}),
        )
        .unwrap();
        assert_eq!(
            markers(dir.path(), 4),
            vec!["explorer-1.json".to_string(), "reviewer_2.json".to_string()]
        );

        let written = fs::read_to_string(
            paneflow_ipc_client::ai_hook::background_generation_dir(dir.path(), 4)
                .join("explorer-1.json"),
        )
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&written).unwrap(),
            json!({"activity_id": "explorer-1", "runtime_generation": 4})
        );

        record(
            HookEvent::SubagentStop,
            Some(dir.path()),
            Some(4),
            &json!({"agent_id": "explorer-1"}),
        )
        .unwrap();
        assert_eq!(markers(dir.path(), 4), vec!["reviewer_2.json".to_string()]);
    }

    #[test]
    fn an_unusable_or_missing_identity_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        record(
            HookEvent::SubagentStart,
            Some(dir.path()),
            Some(1),
            &json!({}),
        )
        .expect("an untagged launch stays metadata-only");
        assert!(markers(dir.path(), 1).is_empty());

        for id in ["../escape", &"a".repeat(161), "has space", ""] {
            assert!(
                record(
                    HookEvent::SubagentStart,
                    Some(dir.path()),
                    Some(1),
                    &json!({"agent_id": id}),
                )
                .is_err(),
                "{id:?} is refused"
            );
        }
        assert!(markers(dir.path(), 1).is_empty());
    }

    #[test]
    fn markers_are_written_under_their_own_generation() {
        let dir = tempfile::tempdir().unwrap();
        record(
            HookEvent::SubagentStart,
            Some(dir.path()),
            Some(2),
            &json!({"agent_id": "child"}),
        )
        .unwrap();
        record(
            HookEvent::SubagentStart,
            Some(dir.path()),
            Some(3),
            &json!({"agent_id": "child"}),
        )
        .unwrap();
        assert_eq!(markers(dir.path(), 2), vec!["child.json".to_string()]);
        assert_eq!(markers(dir.path(), 3), vec!["child.json".to_string()]);

        record(
            HookEvent::SubagentStop,
            Some(dir.path()),
            Some(3),
            &json!({"agent_id": "child"}),
        )
        .unwrap();
        assert_eq!(markers(dir.path(), 2), vec!["child.json".to_string()]);
        assert!(markers(dir.path(), 3).is_empty());
    }

    #[test]
    fn a_session_without_a_directory_or_a_generation_records_nothing() {
        let dir = tempfile::tempdir().unwrap();
        record(
            HookEvent::SubagentStart,
            Some(dir.path()),
            None,
            &json!({"agent_id": "child"}),
        )
        .unwrap();
        record(
            HookEvent::SubagentStart,
            None,
            Some(1),
            &json!({"agent_id": "child"}),
        )
        .unwrap();
        record(
            HookEvent::SubagentStart,
            Some(&dir.path().join("gone")),
            Some(1),
            &json!({"agent_id": "child"}),
        )
        .unwrap();
        assert!(markers(dir.path(), 1).is_empty());
    }
}
