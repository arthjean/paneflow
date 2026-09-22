use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use paneflow_config::schema::{HostInstanceToken, SessionGeneration, SessionId, WorkspaceId};
use serde::{Deserialize, Serialize};

use crate::process::ProcessIdentity;
use crate::runtime_observer::RuntimeObservation;

pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostedSessionRuntime {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_observation: Option<RuntimeObservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_binding: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLaunch {
    pub shell: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SessionLifecycle {
    Starting,
    Running,
    Exited {
        code: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signal: Option<String>,
    },
    Failed {
        reason: String,
    },
    Lost,
    Unverified {
        reason: String,
    },
}

impl SessionLifecycle {
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Starting | Self::Running)
    }

    pub fn holds_ownership(&self) -> bool {
        matches!(
            self,
            Self::Starting | Self::Running | Self::Unverified { .. }
        )
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Exited { .. } => "exited",
            Self::Failed { .. } => "failed",
            Self::Lost => "lost",
            Self::Unverified { .. } => "unverified",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookRecord {
    pub hook_event_name: String,
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub runtime_generation: SessionGeneration,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emitted_at_ms: Option<u64>,
    pub received_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionManifest {
    pub schema: u32,
    pub session: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceId>,
    pub generation: SessionGeneration,
    pub host_instance: HostInstanceToken,
    pub cwd: String,
    pub launch: SessionLaunch,
    pub lifecycle: SessionLifecycle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<ProcessIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_hook: Option<HookRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_started_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen_changed_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen_activity: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub menu_prompt_active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<HostedSessionRuntime>,
    #[serde(default)]
    pub host_protocol_version: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub host_build_id: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("manifest read failed: {0}")]
    Io(#[from] io::Error),
    #[error("manifest exceeds {MAX_MANIFEST_BYTES} bytes")]
    Oversized,
    #[error("manifest is not valid JSON: {0}")]
    Malformed(String),
    #[error("manifest schema {found} is not supported (expected {MANIFEST_SCHEMA_VERSION})")]
    UnsupportedSchema { found: u32 },
    #[error("manifest file name does not match its session id {0}")]
    IdentityMismatch(SessionId),
}

pub fn manifest_path(home: &Path, session: &SessionId) -> PathBuf {
    paneflow_home::host_session_manifest_path_in(home, session.as_str())
}

pub fn write_manifest(home: &Path, manifest: &SessionManifest) -> io::Result<PathBuf> {
    let path = manifest_path(home, &manifest.session);
    let json = serde_json::to_vec_pretty(manifest).map_err(io::Error::other)?;
    write_atomically(&path, &json)?;
    Ok(path)
}

pub fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("record.json");
    let tmp = parent.join(format!(".{file_name}.tmp.{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    let mut attempt = 0;
    loop {
        match std::fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied && attempt < 5 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(error);
            }
        }
    }
}

pub fn write_last_hook_event(
    home: &Path,
    session: &SessionId,
    hook_event_name: &str,
    tool_name: Option<&str>,
    runtime_generation: SessionGeneration,
) -> io::Result<PathBuf> {
    let path = paneflow_home::host_session_data_dir_in(home, session.as_str())
        .join("last-hook-event.json");
    let mut seed = serde_json::Map::new();
    seed.insert(
        "hook_event_name".into(),
        serde_json::Value::String(hook_event_name.to_string()),
    );
    if let Some(tool_name) = tool_name {
        seed.insert(
            "tool_name".into(),
            serde_json::Value::String(tool_name.to_string()),
        );
    }
    seed.insert(
        "runtime_generation".into(),
        serde_json::Value::from(runtime_generation.get()),
    );
    let bytes = serde_json::to_vec(&serde_json::Value::Object(seed)).map_err(io::Error::other)?;
    write_atomically(&path, &bytes)?;
    Ok(path)
}

pub fn remove_session_data(home: &Path, session: &SessionId) {
    let path = paneflow_home::host_session_data_dir_in(home, session.as_str());
    match std::fs::remove_dir_all(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => log::warn!(
            "paneflow-host: cannot delete the session data directory {}: {error}",
            path.display()
        ),
    }
}

pub fn read_manifest(path: &Path) -> Result<SessionManifest, ManifestError> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    let metadata = file.metadata()?;
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err(ManifestError::Oversized);
    }
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(ManifestError::Oversized);
    }
    let manifest: SessionManifest =
        serde_json::from_slice(&bytes).map_err(|e| ManifestError::Malformed(e.to_string()))?;
    if manifest.schema != MANIFEST_SCHEMA_VERSION {
        return Err(ManifestError::UnsupportedSchema {
            found: manifest.schema,
        });
    }
    let expected_name = format!("{}.json", manifest.session);
    if path.file_name().and_then(|n| n.to_str()) != Some(expected_name.as_str()) {
        return Err(ManifestError::IdentityMismatch(manifest.session));
    }
    Ok(manifest)
}

pub fn list_manifest_paths(home: &Path) -> io::Result<Vec<PathBuf>> {
    let dir = paneflow_home::host_sessions_dir_in(home);
    let mut paths = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(paths),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let is_manifest = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| name.ends_with(".json") && !name.starts_with('.'));
        if is_manifest && entry.file_type()?.is_file() {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(session: SessionId) -> SessionManifest {
        SessionManifest {
            schema: MANIFEST_SCHEMA_VERSION,
            session,
            workspace: Some(WorkspaceId::new()),
            generation: SessionGeneration::FIRST,
            host_instance: HostInstanceToken::new(),
            cwd: "/work".to_string(),
            launch: SessionLaunch {
                shell: "/bin/sh".to_string(),
                args: vec![],
                env: BTreeMap::new(),
                cols: 80,
                rows: 24,
            },
            lifecycle: SessionLifecycle::Running,
            process: Some(ProcessIdentity {
                pid: 4242,
                started_at: Some(99),
            }),
            title: None,
            current_cwd: None,
            last_hook: None,
            generation_started_at_ms: None,
            screen_changed_at_ms: None,
            screen_activity: None,
            menu_prompt_active: false,
            runtime: None,
            host_protocol_version: crate::protocol::HOST_PROTOCOL_VERSION,
            host_build_id: crate::protocol::host_build_id(),
            created_at_ms: 1,
            updated_at_ms: 2,
        }
    }

    #[test]
    fn a_manifest_round_trips_under_its_session_id() {
        let home = tempfile::tempdir().unwrap();
        let manifest = sample(SessionId::new());
        let path = write_manifest(home.path(), &manifest).unwrap();
        assert_eq!(
            path,
            home.path()
                .join("host")
                .join("sessions")
                .join(format!("{}.json", manifest.session))
        );
        assert_eq!(read_manifest(&path).unwrap(), manifest);
        assert_eq!(
            list_manifest_paths(home.path()).unwrap(),
            vec![path.clone()]
        );

        let mut updated = manifest.clone();
        updated.lifecycle = SessionLifecycle::Exited {
            code: 3,
            signal: None,
        };
        write_manifest(home.path(), &updated).unwrap();
        assert_eq!(read_manifest(&path).unwrap(), updated);
        assert!(
            !std::fs::read_dir(path.parent().unwrap()).unwrap().any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".tmp.")),
            "no temporary file survives an atomic replacement"
        );
    }

    #[test]
    fn a_runtime_observation_uses_the_nested_manifest_contract() {
        let mut manifest = sample(SessionId::new());
        manifest.runtime = Some(HostedSessionRuntime {
            current_observation: Some(RuntimeObservation {
                id: "com.anthropic.claude-code".to_string(),
                pid: 42,
                pid_started_at: Some(7),
                process_group: 42,
                process_name: "claude".to_string(),
                argv: Some(vec!["claude".to_string()]),
            }),
            launch_binding: Some("com.anthropic.claude-code".to_string()),
        });
        let value = serde_json::to_value(&manifest).unwrap();
        assert_eq!(
            value["runtime"]["current_observation"]["id"],
            "com.anthropic.claude-code"
        );
        assert_eq!(
            value["runtime"]["launch_binding"],
            "com.anthropic.claude-code"
        );
        assert!(value.get("observed_runtime").is_none());
        assert_eq!(
            serde_json::from_value::<SessionManifest>(value).unwrap(),
            manifest
        );
    }

    #[test]
    fn a_schema_one_manifest_from_before_the_worker_split_still_decodes() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let path = manifest_path(home.path(), &session);
        let mut value = serde_json::to_value(sample(session)).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("last_hook");
        object.remove("host_protocol_version");
        object.remove("host_build_id");
        object.insert(
            "agent".to_string(),
            serde_json::json!({
                "tool": "claude",
                "state": "thinking",
                "source": "hook",
                "updated_at_ms": 10
            }),
        );
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let decoded = read_manifest(&path).unwrap();
        assert_eq!(decoded.schema, MANIFEST_SCHEMA_VERSION);
        assert_eq!(decoded.host_protocol_version, 0);
        assert!(decoded.host_build_id.is_empty());
        assert!(decoded.last_hook.is_none());
        assert!(path.ends_with(format!(
            "{decoded_session}.json",
            decoded_session = decoded.session
        )));
    }

    #[test]
    fn a_manifest_stored_under_another_name_is_rejected_not_adopted() {
        let home = tempfile::tempdir().unwrap();
        let manifest = sample(SessionId::new());
        let dir = paneflow_home::host_sessions_dir_in(home.path());
        std::fs::create_dir_all(&dir).unwrap();
        let wrong = dir.join(format!("{}.json", SessionId::new()));
        std::fs::write(&wrong, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(matches!(
            read_manifest(&wrong),
            Err(ManifestError::IdentityMismatch(_))
        ));
        std::fs::write(&wrong, b"{not json").unwrap();
        assert!(matches!(
            read_manifest(&wrong),
            Err(ManifestError::Malformed(_))
        ));
        std::fs::write(&wrong, vec![b' '; MAX_MANIFEST_BYTES as usize + 1]).unwrap();
        assert!(matches!(
            read_manifest(&wrong),
            Err(ManifestError::Oversized)
        ));
        assert!(wrong.exists(), "a rejected manifest is never deleted");
    }
}
