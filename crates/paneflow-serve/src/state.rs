use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use paneflow_config::schema::{SessionGeneration, SessionId, WorkspaceId};
use paneflow_host::agent::{AgentEvent, AgentEventKind};
use paneflow_host::manifest::{HookRecord, SessionLifecycle, SessionManifest};
use paneflow_host::{ProcessIdentity, ProcessVerdict};
use paneflow_ipc_client::agent::AgentStateSource;
use serde_json::{Value, json};

use crate::activity::{AgentDecision, AgentSummary, apply_event};
use crate::protocol::restart_recommendation;

pub const MAX_SEED_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivitySource {
    Hooks,
    Screen,
    None,
}

impl ActivitySource {
    pub fn wire_str(self) -> &'static str {
        match self {
            Self::Hooks => "hooks",
            Self::Screen => "screen",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Live,
    Stopped,
    NonResumable,
}

impl Health {
    pub fn wire_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Stopped => "stopped",
            Self::NonResumable => "non_resumable",
        }
    }

    pub fn may_be_signaled(self) -> bool {
        matches!(self, Self::Live)
    }
}

#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub session: SessionId,
    pub generation: SessionGeneration,
    pub workspace: Option<WorkspaceId>,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub lifecycle: SessionLifecycle,
    pub process: Option<ProcessIdentity>,
    pub core_protocol: u32,
    pub core_build_id: String,
    pub activity: Option<AgentSummary>,
    pub activity_source: ActivitySource,
    pub health: Health,
    pub updated_at_ms: u64,
}

impl SessionEntry {
    fn from_manifest(manifest: SessionManifest, now_ms: u64) -> Self {
        let mut entry = Self {
            session: manifest.session,
            generation: manifest.generation,
            workspace: manifest.workspace,
            title: manifest.title,
            cwd: Some(manifest.current_cwd.unwrap_or(manifest.cwd)),
            lifecycle: manifest.lifecycle,
            process: manifest.process,
            core_protocol: manifest.host_protocol_version,
            core_build_id: manifest.host_build_id,
            activity: None,
            activity_source: ActivitySource::None,
            health: Health::NonResumable,
            updated_at_ms: manifest.updated_at_ms,
        };
        entry.refresh_health();
        let _ = now_ms;
        entry
    }

    pub fn refresh_health(&mut self) {
        self.health = match self.process.map(|identity| identity.verify()) {
            Some(ProcessVerdict::Live) => Health::Live,
            Some(ProcessVerdict::Gone) => Health::Stopped,
            Some(ProcessVerdict::Unverifiable) | None => Health::NonResumable,
        };
    }

    pub fn live(&self) -> bool {
        self.health == Health::Live && self.lifecycle.is_running()
    }

    pub fn status(&self) -> &'static str {
        match self.activity.as_ref().map(|summary| summary.state.as_str()) {
            Some("thinking") => "busy",
            Some("waiting_for_input") => "attention",
            Some("errored") => "errored",
            Some("finished") => "idle",
            _ => "idle",
        }
    }

    pub fn to_value(&self) -> Value {
        let mut value = json!({
            "session": self.session,
            "generation": self.generation,
            "workspace": self.workspace,
            "title": self.title,
            "cwd": self.cwd,
            "lifecycle": self.lifecycle,
            "live": self.live(),
            "health": self.health.wire_str(),
            "status": self.status(),
            "activity": self.activity,
            "activity_source": self.activity_source.wire_str(),
            "core_protocol": self.core_protocol,
            "core_build_id": self.core_build_id,
            "updated_at_ms": self.updated_at_ms,
        });
        if let Some(recommendation) = restart_recommendation(self.core_protocol)
            && let Some(map) = value.as_object_mut()
        {
            map.insert("restart_recommended".to_string(), recommendation);
        }
        value
    }
}

#[derive(Debug, Default)]
pub struct WorkerState {
    sessions: BTreeMap<SessionId, SessionEntry>,
}

impl WorkerState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    pub fn get(&self, session: &SessionId) -> Option<&SessionEntry> {
        self.sessions.get(session)
    }

    pub fn contains(&self, session: &SessionId) -> bool {
        self.sessions.contains_key(session)
    }

    pub fn entries(&self) -> impl Iterator<Item = &SessionEntry> {
        self.sessions.values()
    }

    pub fn snapshot(&self) -> Vec<Value> {
        self.sessions.values().map(SessionEntry::to_value).collect()
    }

    pub fn rebuild_from_home(&mut self, home: &Path) -> usize {
        let paths = match paneflow_host::manifest::list_manifest_paths(home) {
            Ok(paths) => paths,
            Err(error) => {
                log::warn!("paneflow-serve: cannot list session manifests: {error}");
                return 0;
            }
        };
        let now = now_ms();
        let mut rebuilt = BTreeMap::new();
        for path in paths {
            let manifest = match paneflow_host::manifest::read_manifest(&path) {
                Ok(manifest) => manifest,
                Err(error) => {
                    log::warn!(
                        "paneflow-serve: skipping manifest {}: {error}",
                        path.display()
                    );
                    continue;
                }
            };
            let seed = manifest.last_hook.clone();
            let mut entry = SessionEntry::from_manifest(manifest, now);
            if let Some(record) = seed {
                let seed_at = seed_mtime(home, &entry.session).unwrap_or(record.received_at_ms);
                replay_seed(&mut entry, &record, seed_at);
            }
            rebuilt.insert(entry.session.clone(), entry);
        }
        self.sessions = rebuilt;
        self.sessions.len()
    }

    pub fn refresh_health(&mut self) {
        for entry in self.sessions.values_mut() {
            entry.refresh_health();
            if entry.health == Health::Stopped && entry.lifecycle.is_running() {
                entry.lifecycle = SessionLifecycle::Lost;
                entry.updated_at_ms = now_ms();
            }
        }
    }

    pub fn apply_core_snapshot(&mut self, entries: &[Value]) {
        let mut seen = Vec::new();
        for raw in entries {
            let Some(session) = raw["session"]
                .as_str()
                .and_then(|id| SessionId::parse(id).ok())
            else {
                continue;
            };
            seen.push(session.clone());
            let held = self.sessions.remove(&session);
            let mut entry = merge_core_row(session, raw, held);
            entry.refresh_health();
            self.sessions.insert(entry.session.clone(), entry);
        }
        self.sessions.retain(|session, _| seen.contains(session));
    }

    pub fn apply_core_event(&mut self, frame: &Value) -> Option<Value> {
        let event = AgentEvent::from_params(frame).ok()?;
        let entry = self.sessions.get_mut(&event.session)?;
        if let Some(requested) = event.generation
            && requested != entry.generation
        {
            log::debug!(
                "paneflow-serve: event for {} names generation {requested}, the session is at {}",
                event.session,
                entry.generation
            );
            return None;
        }
        let now = now_ms();
        match apply_event(entry.activity.as_ref(), &event, now) {
            AgentDecision::Stale(reason) => {
                log::debug!(
                    "paneflow-serve: refused an event for {}: {reason}",
                    event.session
                );
                None
            }
            AgentDecision::Clear => {
                entry.activity = None;
                entry.activity_source = ActivitySource::None;
                entry.updated_at_ms = now;
                Some(entry.to_value())
            }
            AgentDecision::Update(summary) => {
                entry.activity_source = source_of(event.source);
                entry.activity = Some(*summary);
                entry.updated_at_ms = now;
                Some(entry.to_value())
            }
        }
    }
}

fn source_of(source: AgentStateSource) -> ActivitySource {
    match source {
        AgentStateSource::Hook => ActivitySource::Hooks,
        AgentStateSource::Terminal | AgentStateSource::SessionRegistry => ActivitySource::Screen,
    }
}

fn merge_core_row(session: SessionId, raw: &Value, held: Option<SessionEntry>) -> SessionEntry {
    let generation =
        serde_json::from_value(raw["generation"].clone()).unwrap_or(SessionGeneration::FIRST);
    let lifecycle =
        serde_json::from_value(raw["lifecycle"].clone()).unwrap_or(SessionLifecycle::Lost);
    let workspace = raw["workspace"]
        .as_str()
        .and_then(|id| WorkspaceId::parse(id).ok());
    let held_activity = held
        .as_ref()
        .filter(|entry| entry.generation == generation)
        .and_then(|entry| entry.activity.clone());
    let held_source = held
        .as_ref()
        .filter(|entry| entry.generation == generation)
        .map(|entry| entry.activity_source)
        .unwrap_or(ActivitySource::None);
    let process = serde_json::from_value(raw["process"].clone())
        .ok()
        .or_else(|| held.as_ref().and_then(|entry| entry.process));
    let mut entry = SessionEntry {
        session,
        generation,
        workspace,
        title: raw["title"].as_str().map(str::to_owned),
        cwd: raw["cwd"].as_str().map(str::to_owned),
        lifecycle,
        process,
        core_protocol: raw["host_protocol_version"]
            .as_u64()
            .or_else(|| raw["core_protocol"].as_u64())
            .and_then(|value| u32::try_from(value).ok())
            .or_else(|| held.as_ref().map(|entry| entry.core_protocol))
            .unwrap_or(0),
        core_build_id: raw["host_build_id"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| held.as_ref().map(|entry| entry.core_build_id.clone()))
            .unwrap_or_default(),
        activity: held_activity,
        activity_source: held_source,
        health: Health::NonResumable,
        updated_at_ms: raw["updated_at_ms"].as_u64().unwrap_or_else(now_ms),
    };
    if entry.activity.is_none() {
        if let Ok(record) = serde_json::from_value::<HookRecord>(raw["last_hook"].clone()) {
            replay_seed(&mut entry, &record, record.received_at_ms);
        } else if let Ok(summary) = serde_json::from_value::<AgentSummary>(raw["agent"].clone()) {
            entry.activity_source = AgentStateSource::parse(&summary.source)
                .map(source_of)
                .unwrap_or(ActivitySource::None);
            entry.updated_at_ms = summary.updated_at_ms;
            entry.activity = Some(summary);
        }
    }
    entry
}

fn replay_seed(entry: &mut SessionEntry, record: &HookRecord, seed_at_ms: u64) {
    if record.runtime_generation != entry.generation {
        return;
    }
    let Some(kind) = seed_event_kind(&record.hook_event_name) else {
        return;
    };
    let event = AgentEvent {
        session: entry.session.clone(),
        generation: Some(record.runtime_generation),
        kind,
        tool: record.tool.clone(),
        pid: record.pid,
        tool_name: record.tool_name.clone(),
        message: None,
        summary: None,
        exit_code: (kind == AgentEventKind::Exit).then_some(0),
        emitted_at_ms: record.emitted_at_ms.or(Some(seed_at_ms)),
        received_at_ms: Some(seed_at_ms),
        source: AgentStateSource::Hook,
        event_source: None,
        payload: json!({
            "session_id": record.provider_session_id,
            "transcript_path": record.transcript_path,
        }),
    };
    if let AgentDecision::Update(summary) = apply_event(None, &event, seed_at_ms) {
        entry.activity = Some(*summary);
        entry.activity_source = ActivitySource::Hooks;
        entry.updated_at_ms = seed_at_ms;
    }
}

fn seed_event_kind(raw: &str) -> Option<AgentEventKind> {
    if let Some(kind) = AgentEventKind::parse(raw) {
        return Some(kind);
    }
    let normalized: String = raw
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|character| !matches!(character, '-' | '_' | ' '))
        .collect();
    match normalized.as_str() {
        "userpromptsubmit" | "userpromptsubmitted" | "beforesubmitprompt" => {
            Some(AgentEventKind::PromptSubmit)
        }
        "pretooluse" | "posttooluse" => Some(AgentEventKind::ToolUse),
        "notification" | "permissionrequest" => Some(AgentEventKind::Notification),
        "stop" | "stopfailure" | "interrupt" | "postllmcall" => Some(AgentEventKind::Stop),
        "exit" => Some(AgentEventKind::Exit),
        "sessionend" => Some(AgentEventKind::SessionEnd),
        _ => None,
    }
}

fn seed_mtime(home: &Path, session: &SessionId) -> Option<u64> {
    let path = paneflow_home::host_session_data_dir_in(home, session.as_str())
        .join("last-hook-event.json");
    let file = std::fs::File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if metadata.len() > MAX_SEED_BYTES {
        log::warn!(
            "paneflow-serve: the seed of {session} exceeds {MAX_SEED_BYTES} bytes and is ignored"
        );
        return None;
    }
    metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|elapsed| elapsed.as_millis().min(u128::from(u64::MAX)) as u64)
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_config::schema::HostInstanceToken;
    use paneflow_host::manifest::{MANIFEST_SCHEMA_VERSION, SessionLaunch, write_manifest};

    fn manifest(session: SessionId, last_hook: Option<HookRecord>) -> SessionManifest {
        SessionManifest {
            schema: MANIFEST_SCHEMA_VERSION,
            session,
            workspace: None,
            generation: SessionGeneration::FIRST,
            host_instance: HostInstanceToken::new(),
            cwd: "/work".to_string(),
            launch: SessionLaunch {
                shell: "/bin/sh".to_string(),
                args: Vec::new(),
                env: Default::default(),
                cols: 80,
                rows: 24,
            },
            lifecycle: SessionLifecycle::Running,
            process: None,
            title: None,
            current_cwd: None,
            last_hook,
            host_protocol_version: paneflow_host::HOST_PROTOCOL_VERSION,
            host_build_id: "test-build".to_string(),
            created_at_ms: 1,
            updated_at_ms: 2,
        }
    }

    fn hook(name: &str, generation: SessionGeneration) -> HookRecord {
        HookRecord {
            hook_event_name: name.to_string(),
            tool: "claude".to_string(),
            tool_name: None,
            pid: Some(42),
            runtime_generation: generation,
            provider_session_id: None,
            transcript_path: None,
            emitted_at_ms: Some(1_000),
            received_at_ms: 1_000,
        }
    }

    #[test]
    fn a_restart_rebuilds_every_session_from_its_manifest_and_seed() {
        let home = tempfile::tempdir().unwrap();
        let busy = SessionId::new();
        let bare = SessionId::new();
        write_manifest(
            home.path(),
            &manifest(
                busy.clone(),
                Some(hook("ai.prompt_submit", SessionGeneration::FIRST)),
            ),
        )
        .unwrap();
        write_manifest(home.path(), &manifest(bare.clone(), None)).unwrap();

        let mut state = WorkerState::new();
        assert_eq!(state.rebuild_from_home(home.path()), 2);

        let replayed = state.get(&busy).expect("the busy session is rebuilt");
        assert_eq!(replayed.status(), "busy");
        assert_eq!(replayed.activity_source, ActivitySource::Hooks);
        assert_eq!(
            replayed.activity.as_ref().map(|state| state.state.as_str()),
            Some("thinking")
        );

        let untouched = state.get(&bare).expect("a session with no seed is rebuilt");
        assert_eq!(untouched.status(), "idle");
        assert_eq!(untouched.activity_source, ActivitySource::None);
        assert!(untouched.activity.is_none());
    }

    #[test]
    fn a_restart_replays_the_provider_event_name_persisted_by_the_host() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        write_manifest(
            home.path(),
            &manifest(
                session.clone(),
                Some(hook("UserPromptSubmit", SessionGeneration::FIRST)),
            ),
        )
        .unwrap();

        let mut state = WorkerState::new();
        state.rebuild_from_home(home.path());

        let replayed = state.get(&session).expect("the busy session is rebuilt");
        assert_eq!(replayed.status(), "busy");
        assert_eq!(replayed.activity_source, ActivitySource::Hooks);
        assert_eq!(
            replayed.activity.as_ref().map(|state| state.state.as_str()),
            Some("thinking")
        );
    }

    #[test]
    fn a_seed_from_another_generation_is_never_replayed() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut record = manifest(
            session.clone(),
            Some(hook("ai.prompt_submit", SessionGeneration::FIRST)),
        );
        record.generation = SessionGeneration::FIRST.next();
        write_manifest(home.path(), &record).unwrap();

        let mut state = WorkerState::new();
        state.rebuild_from_home(home.path());
        assert!(
            state.get(&session).expect("rebuilt").activity.is_none(),
            "a seed the session has left never speaks for the new generation"
        );
    }

    #[test]
    fn an_unverifiable_pid_stays_non_resumable_and_is_never_signaled() {
        let mut entry = SessionEntry::from_manifest(manifest(SessionId::new(), None), 0);
        assert_eq!(entry.health, Health::NonResumable);
        assert!(!entry.health.may_be_signaled());

        entry.process = Some(ProcessIdentity {
            pid: std::process::id(),
            started_at: None,
        });
        entry.refresh_health();
        assert_eq!(
            entry.health,
            Health::NonResumable,
            "a pid with no recorded start time is never proven live or dead"
        );
        assert!(!entry.health.may_be_signaled());

        entry.process = Some(ProcessIdentity::capture(std::process::id()));
        entry.refresh_health();
        assert_eq!(entry.health, Health::Live);
        assert!(entry.health.may_be_signaled());

        entry.process = Some(ProcessIdentity {
            pid: std::process::id(),
            started_at: Some(1),
        });
        entry.refresh_health();
        assert_eq!(
            entry.health,
            Health::NonResumable,
            "a recycled pid is not the recorded child"
        );
    }

    #[test]
    fn a_stopped_session_is_only_named_when_its_recorded_child_is_provably_absent() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut record = manifest(session.clone(), None);
        record.process = Some(ProcessIdentity {
            pid: u32::MAX - 1,
            started_at: Some(7),
        });
        write_manifest(home.path(), &record).unwrap();

        let mut state = WorkerState::new();
        state.rebuild_from_home(home.path());
        state.refresh_health();
        let entry = state.get(&session).expect("rebuilt");
        assert_eq!(entry.health, Health::Stopped);
        assert!(!entry.live());
        assert!(!entry.health.may_be_signaled());
    }

    #[test]
    fn a_session_served_by_an_older_core_carries_a_restart_token() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut record = manifest(session.clone(), None);
        record.host_protocol_version = crate::protocol::REQUIRED_CORE_PROTOCOL - 1;
        write_manifest(home.path(), &record).unwrap();

        let mut state = WorkerState::new();
        state.rebuild_from_home(home.path());
        let value = state.get(&session).expect("rebuilt").to_value();
        assert_eq!(
            value["restart_recommended"]["token"],
            crate::protocol::RESTART_RECOMMENDED
        );
    }

    #[test]
    fn a_core_snapshot_adds_a_session_created_after_the_worker_started() {
        let session = SessionId::new();
        let process = ProcessIdentity::capture(std::process::id());
        let raw = json!({
            "session": session,
            "generation": SessionGeneration::FIRST,
            "live": true,
            "lifecycle": SessionLifecycle::Running,
            "process": process,
            "host_protocol_version": paneflow_host::HOST_PROTOCOL_VERSION,
            "host_build_id": "test-build",
            "last_hook": hook("ai.prompt_submit", SessionGeneration::FIRST),
        });
        let mut state = WorkerState::new();
        state.apply_core_snapshot(&[raw]);
        let entry = state
            .get(&session)
            .expect("the new core session is projected");
        assert_eq!(entry.health, Health::Live);
        assert_eq!(entry.status(), "busy");
        assert_eq!(entry.activity_source, ActivitySource::Hooks);
    }

    #[test]
    fn an_old_core_snapshot_keeps_its_reduced_state_and_requests_a_restart() {
        let session = SessionId::new();
        let raw = json!({
            "session": session,
            "generation": SessionGeneration::FIRST,
            "live": true,
            "lifecycle": SessionLifecycle::Running,
            "agent": {
                "tool": "claude",
                "state": "thinking",
                "source": "hook",
                "updated_at_ms": 10,
            },
        });
        let mut state = WorkerState::new();
        state.apply_core_snapshot(&[raw]);
        let value = state.get(&session).expect("legacy session").to_value();
        assert_eq!(value["status"], "busy");
        assert_eq!(value["activity_source"], "hooks");
        assert_eq!(
            value["restart_recommended"]["token"],
            crate::protocol::RESTART_RECOMMENDED
        );
    }
}
