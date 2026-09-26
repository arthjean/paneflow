use std::path::Path;

use crate::agent_sessions::{SessionAgent, SessionMeta, clean_session_label};

const LABEL_MAX_CHARS: usize = 80;

pub fn read_sessions_for_cwd_with_omitted(cwd: &str) -> (Vec<SessionMeta>, usize) {
    let Some(home) = paneflow_home::paneflow_home() else {
        return (Vec::new(), 0);
    };
    read_sessions_from_home(&home, cwd)
}

fn read_sessions_from_home(home: &Path, cwd: &str) -> (Vec<SessionMeta>, usize) {
    let paths = paneflow_host::manifest::list_manifest_paths(home).unwrap_or_default();
    let cache_mtime = paths
        .iter()
        .filter_map(|path| std::fs::metadata(path).ok()?.modified().ok())
        .max()
        .or_else(|| {
            std::fs::metadata(paneflow_home::host_sessions_dir_in(home))
                .ok()?
                .modified()
                .ok()
        });
    let cache_key = cache_key(home, cwd);
    if let Some(cache_mtime) = cache_mtime
        && let Some(cached) = crate::agent_sessions::cache::lookup_with_mtime(
            SessionAgent::Codex,
            &cache_key,
            cache_mtime,
        )
    {
        return cached;
    }

    let mut collector = crate::agent_sessions::RecentSessionCollector::new(
        crate::agent_sessions::SIDEBAR_SESSION_RETAINED_PER_SOURCE,
    );
    for path in paths {
        if let Some(session) = session_from_manifest(&path, cwd) {
            collector.push(session);
        }
    }
    let result = collector.finish();
    if let Some(cache_mtime) = cache_mtime {
        crate::agent_sessions::cache::store_result_with_mtime(
            SessionAgent::Codex,
            &cache_key,
            cache_mtime,
            &result.0,
            result.1,
        );
    }
    result
}

fn cache_key(home: &Path, cwd: &str) -> String {
    format!("{}\u{0}{cwd}", home.display())
}

fn session_from_manifest(path: &Path, cwd: &str) -> Option<SessionMeta> {
    let manifest = paneflow_host::manifest::read_manifest(path).ok()?;
    let agent = manifest.last_hook?;
    if !agent.tool.eq_ignore_ascii_case("codex") {
        return None;
    }
    let provider_session_id = agent.provider_session_id?.trim().to_string();
    if !crate::agent_sessions::is_valid_session_id(&provider_session_id) {
        return None;
    }
    if agent
        .transcript_path
        .as_deref()
        .is_none_or(|path| path.trim().is_empty())
    {
        return None;
    }
    let recorded_cwd = manifest.current_cwd.as_deref().unwrap_or(&manifest.cwd);
    if !crate::agent_sessions::cwd_matches(recorded_cwd, cwd) {
        return None;
    }
    let summary = agent
        .tool_name
        .as_deref()
        .and_then(|value| clean_session_label(value, LABEL_MAX_CHARS));
    Some(SessionMeta {
        agent: SessionAgent::Codex,
        session_id: provider_session_id,
        timestamp: crate::agent_sessions::unix_millis_to_iso8601(agent.received_at_ms),
        cwd: recorded_cwd.to_string(),
        summary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_manifest(home: &Path, agent: serde_json::Value) {
        let session = "550e8400-e29b-41d4-a716-446655440000";
        let path = paneflow_home::host_session_manifest_path_in(home, session);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("session directory");
        let value = serde_json::json!({
            "schema": paneflow_host::manifest::MANIFEST_SCHEMA_VERSION,
            "session": session,
            "generation": 1,
            "host_instance": "550e8400-e29b-41d4-a716-446655440001",
            "cwd": "C:\\dev\\paneflow",
            "launch": {"shell": "pwsh", "cols": 120, "rows": 30},
            "lifecycle": {"state": "running"},
            "last_hook": agent,
            "host_protocol_version": paneflow_host::HOST_PROTOCOL_VERSION,
            "host_build_id": "test-build",
            "created_at_ms": 10,
            "updated_at_ms": 20
        });
        std::fs::write(path, serde_json::to_vec_pretty(&value).expect("JSON")).expect("manifest");
    }

    #[test]
    fn codex_sessions_come_from_paneflow_manifests() {
        let home = tempfile::tempdir().expect("home");
        write_manifest(
            home.path(),
            serde_json::json!({
                "hook_event_name": "ai.stop",
                "tool": "codex",
                "tool_name": "Implement the runtime system",
                "runtime_generation": 1,
                "provider_session_id": "019dc9ea-38d7-7372-9cc4-253ce944d41b",
                "transcript_path": "C:\\Users\\Arthur\\.codex\\sessions\\rollout.jsonl",
                "received_at_ms": 42
            }),
        );
        let (sessions, omitted) = read_sessions_from_home(home.path(), "C:\\dev\\paneflow");
        assert_eq!(omitted, 0);
        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0].session_id,
            "019dc9ea-38d7-7372-9cc4-253ce944d41b"
        );
        assert_eq!(
            sessions[0].summary.as_deref(),
            Some("Implement the runtime system")
        );
        assert_eq!(sessions[0].timestamp, "1970-01-01T00:00:00.042Z");
    }

    #[test]
    fn the_session_cache_key_is_scoped_to_the_home_that_owns_the_manifests() {
        let first = tempfile::tempdir().expect("first home");
        let second = tempfile::tempdir().expect("second home");
        let cwd = "C:\\dev\\paneflow";
        assert_ne!(
            cache_key(first.path(), cwd),
            cache_key(second.path(), cwd),
            "two homes reading the same directory never share a cache entry"
        );
        assert_eq!(cache_key(first.path(), cwd), cache_key(first.path(), cwd));
        assert_ne!(
            cache_key(first.path(), cwd),
            cache_key(first.path(), "C:\\dev\\other")
        );
    }

    #[test]
    fn a_foreign_provider_is_ignored() {
        let home = tempfile::tempdir().expect("home");
        write_manifest(
            home.path(),
            serde_json::json!({
                "tool": "claude-code",
                "state": "idle",
                "source": "hook",
                "provider_session_id": "session",
                "updated_at_ms": 42,
                "stale": false
            }),
        );
        assert!(
            read_sessions_from_home(home.path(), "C:\\dev\\paneflow")
                .0
                .is_empty()
        );
    }
}
