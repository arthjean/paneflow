use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use paneflow_agent_config::runtime_catalog::Runtime;
use paneflow_config::schema::{SessionGeneration, SessionId, WorkspaceId};
use paneflow_host::agent::AgentEvent;
use paneflow_host::manifest::{SessionLifecycle, SessionManifest};
use paneflow_host::program_status::DeclaredStatus;
use paneflow_host::runtime_observer::RuntimeObservation;
use paneflow_host::{ProcessIdentity, ProcessVerdict};
use paneflow_ipc_client::agent::{AgentState, next_waiting_since};
use serde_json::{Value, json};

use crate::activity::{AgentDecision, AgentSummary, apply_event, reconcile_adopted};
use crate::notifications::{ActivityLog, Notice, Notification, notification_for};
use crate::protocol::restart_recommendation;

pub const DECLARED_WORKING: &str = "working";
pub const DECLARED_BLOCKED: &str = "blocked";
pub const DECLARED_DONE: &str = "done";
pub const DECLARED_ERROR: &str = "error";

pub const OUTCOME_COMPLETED: &str = "completed";
pub const OUTCOME_FAILED: &str = "failed";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivitySource {
    Declared,
    None,
}

impl ActivitySource {
    pub fn wire_str(self) -> &'static str {
        match self {
            Self::Declared => "declared",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Busy,
    Idle,
    Attention,
    Errored,
}

impl Status {
    pub fn wire_str(self) -> &'static str {
        match self {
            Self::Busy => "busy",
            Self::Idle => "idle",
            Self::Attention => "attention",
            Self::Errored => "errored",
        }
    }

    fn agent_state(self) -> AgentState {
        match self {
            Self::Busy => AgentState::Thinking,
            Self::Idle => AgentState::Finished,
            Self::Attention => AgentState::WaitingForInput,
            Self::Errored => AgentState::Errored,
        }
    }

    fn in_turn(self) -> bool {
        matches!(self, Self::Busy | Self::Attention)
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
}

#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub hook_revision: u64,
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
    declared_activity: bool,
    pub activity_source: ActivitySource,
    pub status: Status,
    pub outcome: Option<String>,
    pub health: Health,
    pub declared_status: Option<DeclaredStatus>,
    pub observed_runtime: Option<RuntimeObservation>,
    pub foreground_runtime: Option<&'static str>,
    pub state_seq: u64,
    pub unread: bool,
    pub updated_at_ms: u64,
}

impl SessionEntry {
    fn from_manifest(manifest: SessionManifest) -> Self {
        let observed_runtime = manifest
            .runtime
            .and_then(|runtime| runtime.current_observation);
        let mut entry = Self {
            hook_revision: 0,
            session: manifest.session,
            generation: manifest.generation,
            workspace: manifest.workspace,
            title: manifest.title,
            cwd: Some(manifest.current_cwd.unwrap_or(manifest.cwd)),
            lifecycle: manifest.lifecycle,
            process: manifest.process,
            core_protocol: manifest.host_protocol_version,
            core_build_id: manifest.host_build_id,
            activity: manifest
                .last_hook
                .as_ref()
                .map(|hook| AgentSummary::declared(&hook.tool, manifest.updated_at_ms)),
            declared_activity: false,
            activity_source: ActivitySource::None,
            status: Status::Idle,
            outcome: None,
            health: Health::NonResumable,
            declared_status: manifest.declared_status,
            foreground_runtime: foreground_runtime_id(observed_runtime.as_ref()),
            observed_runtime,
            state_seq: 0,
            unread: false,
            updated_at_ms: manifest.updated_at_ms,
        };
        entry.adopt_declared_verdict();
        entry.refresh_health();
        entry
    }

    fn adopt_declared_verdict(&mut self) {
        let (status, source) = self.declared_verdict();
        self.status = status;
        self.activity_source = source;
        self.outcome = self
            .agent_declaration()
            .and_then(|declared| declared_outcome(declared, None));
    }

    fn agent_declaration(&self) -> Option<&DeclaredStatus> {
        self.runtime().and(self.declared_status.as_ref())
    }

    fn declared_verdict(&self) -> (Status, ActivitySource) {
        let Some(declared) = self.agent_declaration() else {
            return (Status::Idle, ActivitySource::None);
        };
        let running = self.lifecycle.is_running();
        let status = match declared.state.as_str() {
            DECLARED_ERROR => Status::Errored,
            DECLARED_WORKING if running => Status::Busy,
            DECLARED_BLOCKED if running => Status::Attention,
            _ => Status::Idle,
        };
        (status, ActivitySource::Declared)
    }

    fn reconcile_declared_activity(&mut self, source: ActivitySource, now_ms: u64) -> bool {
        if source == ActivitySource::Declared {
            if self.activity.is_some() {
                return false;
            }
            let Some(observation) = self.observed_runtime.as_ref() else {
                return false;
            };
            let Some(tool) = observation
                .runtime()
                .and_then(|runtime| runtime.detection.command_aliases.first())
            else {
                return false;
            };
            let mut summary = AgentSummary::declared(tool, now_ms);
            summary.pid = Some(observation.pid);
            self.activity = Some(summary);
            self.declared_activity = true;
            return true;
        }
        if self.declared_activity && self.activity.is_some() {
            self.activity = None;
            self.declared_activity = false;
            return true;
        }
        false
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
        self.status.wire_str()
    }

    pub fn runtime(&self) -> Option<&'static Runtime> {
        self.activity
            .as_ref()
            .and_then(|summary| runtime_for_tool(&summary.tool))
            .or_else(|| {
                self.observed_runtime
                    .as_ref()
                    .and_then(RuntimeObservation::runtime)
            })
    }

    pub fn runtime_label(&self) -> String {
        match self.runtime() {
            Some(runtime) => runtime.label.to_string(),
            None => self
                .activity
                .as_ref()
                .map(|summary| summary.tool.clone())
                .unwrap_or_else(|| "Agent".to_string()),
        }
    }

    fn declared_message(&self) -> Option<String> {
        self.declared_status
            .as_ref()
            .map(|declared| declared.message.trim())
            .filter(|message| !message.is_empty())
            .map(str::to_owned)
    }

    pub fn to_value(&self) -> Value {
        let mut value = json!({
            "session": self.session,
            "generation": self.generation,
            "hook_revision": self.hook_revision,
            "workspace": self.workspace,
            "title": self.title,
            "cwd": self.cwd,
            "lifecycle": self.lifecycle,
            "live": self.live(),
            "health": self.health.wire_str(),
            "status": self.status(),
            "state_seq": self.state_seq,
            "activity": self.activity,
            "activity_source": self.activity_source.wire_str(),
            "outcome": self.outcome,
            "runtime_id": self.runtime().map(|runtime| runtime.id),
            "foreground_runtime_id": self.foreground_runtime,
            "declared_status": self.declared_status,
            "unread": self.unread,
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

pub use paneflow_agent_config::runtime_catalog::runtime_for_tool;

fn declared_outcome(declared: &DeclaredStatus, held: Option<&str>) -> Option<String> {
    match declared.state.as_str() {
        DECLARED_DONE => Some(OUTCOME_COMPLETED.to_string()),
        DECLARED_ERROR => Some(match declared.message.trim() {
            "" => OUTCOME_FAILED.to_string(),
            message => format!("{OUTCOME_FAILED}:{}", failure_reason_text(message)),
        }),
        DECLARED_WORKING => None,
        _ => held.map(str::to_owned),
    }
}

fn notice_between(
    previous: Status,
    current: Status,
    declared: Option<&DeclaredStatus>,
) -> Option<Notice> {
    match (previous, current) {
        (Status::Attention, Status::Attention) | (Status::Errored, Status::Errored) => None,
        (_, Status::Attention | Status::Errored) => Some(Notice::NeedsInput),
        (previous, Status::Idle)
            if previous.in_turn()
                && declared.is_some_and(|declared| declared.state == DECLARED_DONE) =>
        {
            Some(Notice::Finished)
        }
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projection {
    pub session: Value,
    pub notification: Option<Notification>,
    pub changed: bool,
}

#[derive(Debug)]
pub struct WorkerState {
    home: PathBuf,
    sessions: BTreeMap<SessionId, SessionEntry>,
    log: ActivityLog,
}

impl WorkerState {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self {
            home: home.into(),
            sessions: BTreeMap::new(),
            log: ActivityLog::default(),
        }
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

    pub fn activity_log(&self) -> &ActivityLog {
        &self.log
    }

    pub fn snapshot(&self) -> Vec<Value> {
        self.sessions.values().map(SessionEntry::to_value).collect()
    }

    pub fn rebuild_from_home(&mut self, home: &Path) -> usize {
        self.home = home.to_path_buf();
        let paths = match paneflow_host::manifest::list_manifest_paths(home) {
            Ok(paths) => paths,
            Err(error) => {
                log::warn!("paneflow-serve: cannot list session manifests: {error}");
                return 0;
            }
        };
        let mut rebuilt = BTreeMap::new();
        let mut accepted_events = Vec::new();
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
            if let Some(hook) = manifest.last_hook.as_ref() {
                accepted_events.extend(hook.activity_event.clone());
                accepted_events.extend(hook.event.clone());
            }
            let entry = SessionEntry::from_manifest(manifest);
            rebuilt.insert(entry.session.clone(), entry);
        }
        self.sessions = rebuilt;
        for frame in accepted_events {
            self.apply_core_event(&frame);
        }
        self.reconcile_adopted_activity();
        let sessions: Vec<SessionId> = self.sessions.keys().cloned().collect();
        let now = now_ms();
        for session in sessions {
            self.derive(&session, now);
        }
        self.sessions.len()
    }

    fn reconcile_adopted_activity(&mut self) {
        let now = now_ms();
        for entry in self.sessions.values_mut() {
            if let Some(activity) = entry.activity.as_mut().filter(|activity| !activity.stale) {
                reconcile_adopted(activity, &entry.lifecycle, now);
            }
        }
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

    pub fn apply_core_snapshot(&mut self, entries: &[Value]) -> Vec<Projection> {
        self.apply_core_rows(entries, true)
    }

    pub fn apply_core_session(&mut self, entry: &Value) -> Vec<Projection> {
        self.apply_core_rows(std::slice::from_ref(entry), false)
    }

    pub fn forget_core_session(&mut self, session: &SessionId) -> bool {
        self.sessions.remove(session).is_some()
    }

    fn apply_core_rows(&mut self, entries: &[Value], prune: bool) -> Vec<Projection> {
        let mut seen = BTreeSet::new();
        let mut discovered = BTreeSet::new();
        let mut recovered = BTreeMap::new();
        let mut foreground_moved = BTreeSet::new();
        let mut declared_moved = BTreeSet::new();
        for raw in entries {
            let Some(session) = raw["session"]
                .as_str()
                .and_then(|id| SessionId::parse(id).ok())
            else {
                continue;
            };
            seen.insert(session.clone());
            if let Some(held) = self.sessions.get(&session)
                && raw["generation"]
                    .as_u64()
                    .is_some_and(|generation| generation < held.generation.get())
            {
                continue;
            }
            let held = self.sessions.remove(&session);
            if held.is_none() {
                discovered.insert(session.clone());
            }
            let foreground_before = held.as_ref().and_then(|entry| entry.foreground_runtime);
            let declared_before = held
                .as_ref()
                .and_then(|entry| entry.declared_status.clone());
            let mut entry = merge_core_row(session.clone(), raw, held);
            if entry.declared_status != declared_before {
                declared_moved.insert(session.clone());
            }
            entry.refresh_health();
            self.sessions.insert(entry.session.clone(), entry);
            for field in ["activity_event", "event"] {
                if let Some(frame) = raw["last_hook"][field].as_object()
                    && let Some(projection) = self.apply_core_event(&Value::Object(frame.clone()))
                {
                    recovered.insert(session.clone(), projection);
                }
            }
            let foreground_after = self
                .sessions
                .get(&session)
                .and_then(|entry| entry.foreground_runtime);
            if foreground_after != foreground_before {
                foreground_moved.insert(session.clone());
            }
        }
        if prune {
            self.sessions.retain(|session, _| seen.contains(session));
        }
        self.reconcile_adopted_activity();
        let now = now_ms();
        seen.into_iter()
            .filter_map(|session| {
                let mut projection = self.derive(&session, now)?;
                let recovered_event = recovered.contains_key(&session);
                if let Some(recovery) = recovered.remove(&session) {
                    projection.changed |= recovery.changed;
                    projection.notification = projection.notification.or(recovery.notification);
                }
                let announce = (projection.changed
                    && (recovered_event || !discovered.contains(&session)))
                    || projection.notification.is_some()
                    || foreground_moved.contains(&session)
                    || declared_moved.contains(&session);
                announce.then_some(projection)
            })
            .collect()
    }

    pub fn sweep(&mut self) -> Vec<Projection> {
        let sessions: Vec<SessionId> = self.sessions.keys().cloned().collect();
        let now = now_ms();
        sessions
            .into_iter()
            .filter_map(|session| {
                let projection = self.derive(&session, now)?;
                (projection.changed || projection.notification.is_some()).then_some(projection)
            })
            .collect()
    }

    pub fn apply_core_event(&mut self, frame: &Value) -> Option<Projection> {
        let event = AgentEvent::from_params(frame).ok()?;
        let entry = self.sessions.get_mut(&event.session)?;
        let revision = frame["revision"].as_u64().unwrap_or(0);
        if (revision > 0 && revision <= entry.hook_revision)
            || (revision == 0 && entry.hook_revision > 0)
        {
            return None;
        }
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
        entry.hook_revision = revision;
        let now_ms = frame["received_at_ms"].as_u64().unwrap_or_else(now_ms);
        match apply_event(entry.activity.as_ref(), &event, now_ms) {
            AgentDecision::Stale(reason) => {
                log::debug!(
                    "paneflow-serve: refused an event for {}: {reason}",
                    event.session
                );
                return None;
            }
            AgentDecision::Clear => entry.activity = None,
            AgentDecision::Update(summary) => entry.activity = Some(*summary),
        }
        entry.declared_activity = false;
        entry.updated_at_ms = now_ms;
        let mut projection = self.derive(&event.session, now_ms)?;
        projection.changed = true;
        Some(projection)
    }

    fn derive(&mut self, session: &SessionId, now_ms: u64) -> Option<Projection> {
        let entry = self.sessions.get_mut(session)?;
        let (status, source) = entry.declared_verdict();
        let declared = entry.agent_declaration();
        let notice = notice_between(entry.status, status, declared);
        let outcome = match declared {
            Some(declared) => declared_outcome(declared, entry.outcome.as_deref()),
            None => entry.outcome.clone(),
        };
        let outcome_changed = entry.outcome != outcome;
        let declared_row_changed = entry.reconcile_declared_activity(source, now_ms);
        let state = status.agent_state();
        let settled = !declared_row_changed
            && entry.status == status
            && entry.activity_source == source
            && entry.outcome == outcome
            && entry
                .activity
                .as_ref()
                .is_none_or(|summary| summary.state == state.wire_str());
        if entry.status != status {
            entry.state_seq += 1;
        }
        entry.status = status;
        entry.activity_source = source;
        entry.outcome.clone_from(&outcome);
        if let Some(summary) = entry.activity.as_mut() {
            let previous = AgentState::parse(&summary.state);
            summary.waiting_since_ms = next_waiting_since(
                previous
                    .as_ref()
                    .map(|held| (held, summary.waiting_since_ms)),
                &state,
                now_ms,
            );
            summary.state = state.wire_str().to_string();
            summary.source = source_wire(source).to_string();
            if !settled {
                summary.updated_at_ms = now_ms;
            }
        }
        if !settled {
            entry.updated_at_ms = now_ms;
        }
        let body = match notice {
            Some(Notice::Finished) => entry
                .activity
                .as_ref()
                .and_then(|summary| summary.last_result.clone())
                .or_else(|| entry.declared_message()),
            Some(Notice::NeedsInput) => entry.declared_message().or_else(|| {
                entry
                    .activity
                    .as_ref()
                    .and_then(|summary| summary.message.clone())
            }),
            None => None,
        };
        let notification = notice
            .and_then(|notice| notification_for(notice, &entry.runtime_label(), body.as_deref()));
        if notification
            .as_ref()
            .is_some_and(|notification| notification.kind == crate::notifications::KIND_FINISHED)
        {
            entry.unread = true;
        }
        if outcome_changed && let Some(outcome) = outcome.as_deref() {
            self.log.record(session, entry.generation, outcome, now_ms);
        }
        Some(Projection {
            session: entry.to_value(),
            notification,
            changed: !settled,
        })
    }

    pub fn acknowledge(&mut self, sessions: &[SessionId]) -> Vec<Projection> {
        sessions
            .iter()
            .filter_map(|session| {
                let entry = self.sessions.get_mut(session)?;
                if !entry.unread {
                    return None;
                }
                entry.unread = false;
                Some(Projection {
                    session: entry.to_value(),
                    notification: None,
                    changed: true,
                })
            })
            .collect()
    }
}

fn source_wire(source: ActivitySource) -> &'static str {
    match source {
        ActivitySource::Declared => "terminal",
        ActivitySource::None => "hook",
    }
}

const MAX_FAILURE_REASON_BYTES: usize = 128;

fn failure_reason_text(raw: &str) -> String {
    let mut end = raw.len().min(MAX_FAILURE_REASON_BYTES);
    while !raw.is_char_boundary(end) {
        end -= 1;
    }
    raw[..end].to_string()
}

fn foreground_runtime_id(observation: Option<&RuntimeObservation>) -> Option<&'static str> {
    observation
        .and_then(RuntimeObservation::runtime)
        .map(|runtime| runtime.id)
}

fn merge_core_row(session: SessionId, raw: &Value, held: Option<SessionEntry>) -> SessionEntry {
    let generation =
        serde_json::from_value(raw["generation"].clone()).unwrap_or(SessionGeneration::FIRST);
    let lifecycle =
        serde_json::from_value(raw["lifecycle"].clone()).unwrap_or(SessionLifecycle::Lost);
    let workspace = raw["workspace"]
        .as_str()
        .and_then(|id| WorkspaceId::parse(id).ok());
    let same_generation = held
        .as_ref()
        .filter(|entry| entry.generation == generation)
        .is_some();
    let held_activity = same_generation
        .then(|| held.as_ref().and_then(|entry| entry.activity.clone()))
        .flatten();
    let declared_activity =
        held_activity.is_some() && held.as_ref().is_some_and(|entry| entry.declared_activity);
    let process = serde_json::from_value(raw["process"].clone())
        .ok()
        .or_else(|| held.as_ref().and_then(|entry| entry.process));
    let declared = raw["last_hook"]["tool"]
        .as_str()
        .map(|tool| AgentSummary::declared(tool, now_ms()));
    let current_observation: Option<RuntimeObservation> =
        serde_json::from_value(raw["observed_runtime"].clone())
            .ok()
            .flatten();
    let mut entry = SessionEntry {
        hook_revision: held
            .as_ref()
            .filter(|entry| entry.generation == generation)
            .map_or(0, |entry| entry.hook_revision),
        session,
        generation,
        workspace,
        title: raw["title"].as_str().map(str::to_owned),
        cwd: raw["cwd"].as_str().map(str::to_owned),
        lifecycle,
        process,
        core_protocol: raw["host_protocol_version"]
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .or_else(|| held.as_ref().map(|entry| entry.core_protocol))
            .unwrap_or(0),
        core_build_id: raw["host_build_id"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| held.as_ref().map(|entry| entry.core_build_id.clone()))
            .unwrap_or_default(),
        activity: held_activity
            .or(declared)
            .or_else(|| serde_json::from_value::<AgentSummary>(raw["agent"].clone()).ok()),
        declared_activity,
        activity_source: ActivitySource::None,
        status: Status::Idle,
        outcome: None,
        health: Health::NonResumable,
        declared_status: serde_json::from_value(raw["declared_status"].clone())
            .ok()
            .flatten(),
        foreground_runtime: foreground_runtime_id(current_observation.as_ref()),
        observed_runtime: current_observation.or_else(|| {
            held.as_ref()
                .and_then(|entry| entry.observed_runtime.clone())
        }),
        state_seq: held.as_ref().map_or(0, |entry| entry.state_seq),
        unread: false,
        updated_at_ms: now_ms(),
    };
    match held.filter(|_| same_generation) {
        Some(held) => {
            entry.activity_source = held.activity_source;
            entry.status = held.status;
            entry.outcome = held.outcome;
            entry.unread = held.unread;
        }
        None => entry.adopt_declared_verdict(),
    }
    entry
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
    use crate::notifications::{KIND_FINISHED, KIND_NEEDS_INPUT};
    use paneflow_config::schema::HostInstanceToken;
    use paneflow_host::manifest::{
        HookRecord, MANIFEST_SCHEMA_VERSION, SessionLaunch, write_manifest,
    };

    const CLAUDE: &str = "com.anthropic.claude-code";
    const LAUNCHED_AT: u64 = 1_000;

    fn observation(id: &str, pid: u32, started_at: u64) -> RuntimeObservation {
        RuntimeObservation {
            id: id.to_string(),
            pid,
            pid_started_at: Some(started_at),
            process_group: pid,
            process_name: "agent".to_string(),
            argv: None,
        }
    }

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
            name: None,
            title: None,
            current_cwd: None,
            last_hook,
            hook_revision: 0,
            generation_started_at_ms: Some(LAUNCHED_AT),
            screen_changed_at_ms: None,
            declared_status: None,
            runtime: None,
            final_output: None,
            host_protocol_version: paneflow_host::HOST_PROTOCOL_VERSION,
            host_build_id: "test-build".to_string(),
            created_at_ms: 1,
            updated_at_ms: 2,
        }
    }

    fn hook(name: &str, generation: SessionGeneration) -> HookRecord {
        HookRecord {
            event: None,
            activity_event: None,
            hook_event_name: name.to_string(),
            tool: "claude".to_string(),
            tool_name: None,
            pid: Some(42),
            runtime_generation: generation,
            provider_session_id: None,
            transcript_path: None,
            emitted_at_ms: Some(LAUNCHED_AT),
            received_at_ms: LAUNCHED_AT,
        }
    }

    fn frame(session: &SessionId, kind: &str, hook_event_name: &str, payload: Value) -> Value {
        let mut hook_payload = json!({"hook_event_name": hook_event_name});
        if let (Some(map), Some(extra)) = (hook_payload.as_object_mut(), payload.as_object()) {
            for (key, value) in extra {
                map.insert(key.clone(), value.clone());
            }
        }
        json!({
            "session": session.to_string(),
            "kind": kind,
            "tool": "claude",
            "pid": 42,
            "hook_payload": hook_payload,
        })
    }

    fn frame_from_pid(session: &SessionId, kind: &str, hook_event_name: &str, pid: u32) -> Value {
        let mut frame = frame(session, kind, hook_event_name, json!({}));
        frame["pid"] = json!(pid);
        frame
    }

    fn core_row(session: &SessionId) -> Value {
        json!({
            "session": session,
            "generation": SessionGeneration::FIRST,
            "generation_started_at_ms": LAUNCHED_AT,
            "live": true,
            "lifecycle": SessionLifecycle::Running,
            "host_protocol_version": paneflow_host::HOST_PROTOCOL_VERSION,
            "host_build_id": "test-build",
        })
    }

    fn agent_row(session: &SessionId) -> Value {
        let mut row = core_row(session);
        row["observed_runtime"] = serde_json::to_value(observation(CLAUDE, 10, 5)).unwrap();
        row
    }

    fn declared(state: &str, message: &str) -> Value {
        json!({"state": state, "kind": "permission", "app": "claude", "message": message})
    }

    fn with_declared(mut raw: Value, state: &str, message: &str) -> Value {
        raw["declared_status"] = declared(state, message);
        raw
    }

    fn exited(mut row: Value) -> Value {
        row["live"] = json!(false);
        row["lifecycle"] = json!(SessionLifecycle::Exited {
            code: 0,
            signal: None,
        });
        row
    }

    fn running_state(home: &Path, session: &SessionId) -> WorkerState {
        write_manifest(home, &manifest(session.clone(), None)).unwrap();
        let mut state = WorkerState::new(home);
        state.rebuild_from_home(home);
        state
    }

    fn notifications(projections: &[Projection]) -> Vec<&'static str> {
        projections
            .iter()
            .filter_map(|projection| projection.notification.as_ref())
            .map(|notification| notification.kind)
            .collect()
    }

    #[test]
    fn a_declared_turn_runs_busy_and_its_done_notifies_finished_once() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = WorkerState::new(home.path());
        state.apply_core_snapshot(&[agent_row(&session)]);
        assert_eq!(state.get(&session).unwrap().status(), "idle");
        assert_eq!(
            state.get(&session).unwrap().activity_source,
            ActivitySource::None
        );

        let projections =
            state.apply_core_session(&with_declared(agent_row(&session), "working", ""));
        let entry = state.get(&session).unwrap();
        assert_eq!(entry.status(), "busy");
        assert_eq!(entry.activity_source, ActivitySource::Declared);
        assert_eq!(entry.to_value()["activity_source"], "declared");
        let activity = entry.activity.as_ref().expect("the agent gets a row");
        assert_eq!(activity.tool, "claude");
        assert_eq!(activity.state, "thinking");
        assert_eq!(activity.source, "terminal");
        assert!(notifications(&projections).is_empty());

        let projections =
            state.apply_core_session(&with_declared(agent_row(&session), "done", "All set"));
        let entry = state.get(&session).unwrap();
        assert_eq!(entry.status(), "idle");
        assert_eq!(entry.outcome.as_deref(), Some(OUTCOME_COMPLETED));
        assert!(entry.unread);
        assert_eq!(notifications(&projections), [KIND_FINISHED]);
        let notification = projections
            .iter()
            .find_map(|projection| projection.notification.as_ref())
            .unwrap();
        assert_eq!(notification.runtime_label, "Claude Code");
        assert_eq!(notification.body.as_deref(), Some("All set"));
        assert!(
            state
                .activity_log()
                .to_values(1)
                .first()
                .is_some_and(|entry| entry["outcome"] == OUTCOME_COMPLETED)
        );

        let projections =
            state.apply_core_session(&with_declared(agent_row(&session), "done", "All set"));
        assert!(
            notifications(&projections).is_empty(),
            "a held done is quiet"
        );
    }

    #[test]
    fn an_idle_after_a_turn_settles_without_a_finished_notification() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = WorkerState::new(home.path());
        state.apply_core_snapshot(&[with_declared(agent_row(&session), "working", "")]);

        let projections = state.apply_core_session(&with_declared(agent_row(&session), "idle", ""));

        assert_eq!(state.get(&session).unwrap().status(), "idle");
        assert!(
            notifications(&projections).is_empty(),
            "only done reports a finished turn"
        );
    }

    #[test]
    fn a_declared_blocker_asks_for_attention_once_with_its_message() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = WorkerState::new(home.path());
        state.apply_core_snapshot(&[with_declared(agent_row(&session), "working", "")]);

        let projections = state.apply_core_session(&with_declared(
            agent_row(&session),
            "blocked",
            "Apply the plan?",
        ));
        let entry = state.get(&session).unwrap();
        assert_eq!(entry.status(), "attention");
        assert_eq!(entry.activity.as_ref().unwrap().state, "waiting_for_input");
        assert_eq!(notifications(&projections), [KIND_NEEDS_INPUT]);
        let notification = projections
            .iter()
            .find_map(|projection| projection.notification.as_ref())
            .unwrap();
        assert_eq!(notification.body.as_deref(), Some("Apply the plan?"));

        let projections = state.apply_core_session(&with_declared(
            agent_row(&session),
            "blocked",
            "Apply the plan?",
        ));
        assert!(notifications(&projections).is_empty());
    }

    #[test]
    fn an_agent_that_declares_error_is_errored_and_failed_until_the_record_goes() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = WorkerState::new(home.path());
        state.apply_core_snapshot(&[with_declared(agent_row(&session), "working", "")]);

        let projections = state.apply_core_session(&with_declared(
            agent_row(&session),
            "error",
            "2 tests failed",
        ));
        let entry = state.get(&session).unwrap();
        assert_eq!(entry.status(), "errored");
        assert_eq!(entry.to_value()["activity"]["state"], "errored");
        assert_eq!(entry.outcome.as_deref(), Some("failed:2 tests failed"));
        assert_eq!(notifications(&projections), [KIND_NEEDS_INPUT]);

        state.apply_core_session(&agent_row(&session));
        let entry = state.get(&session).unwrap();
        assert_eq!(entry.status(), "idle");
        assert_eq!(entry.activity_source, ActivitySource::None);
        assert_eq!(
            entry.outcome.as_deref(),
            Some("failed:2 tests failed"),
            "the last outcome stays readable once the record goes"
        );
    }

    #[test]
    fn a_program_without_an_agent_keeps_its_declared_status_but_never_moves_the_row() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = WorkerState::new(home.path());
        state.apply_core_snapshot(&[core_row(&session)]);

        for declared_state in ["working", "blocked", "error", "done"] {
            let projections = state.apply_core_session(&with_declared(
                core_row(&session),
                declared_state,
                "Apply the plan?",
            ));
            let entry = state.get(&session).unwrap();
            assert!(entry.runtime().is_none());
            assert_eq!(
                entry.status(),
                "idle",
                "{declared_state}: a program is not an agent"
            );
            assert_eq!(entry.to_value()["activity"], Value::Null);
            assert_eq!(entry.to_value()["declared_status"]["state"], declared_state);
            assert!(
                projections
                    .iter()
                    .any(|projection| projection.session["declared_status"]["state"]
                        == declared_state),
                "a declared change is announced even when the status does not move"
            );
            assert!(
                notifications(&projections).is_empty(),
                "serve never notifies for a program, the desktop does"
            );
        }

        let projections = state.apply_core_session(&core_row(&session));
        assert_eq!(
            state.get(&session).unwrap().to_value()["declared_status"],
            Value::Null
        );
        assert!(!projections.is_empty(), "a removed record is announced too");
    }

    #[test]
    fn a_hook_event_carries_metadata_but_never_moves_the_declared_state() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = running_state(home.path(), &session);

        let projection = state
            .apply_core_event(&frame(
                &session,
                "ai.prompt_submit",
                "UserPromptSubmit",
                json!({"session_id": "provider-1", "transcript_path": "/tmp/t.jsonl"}),
            ))
            .expect("the hook projects");
        assert_eq!(projection.session["status"], "idle");
        assert_eq!(projection.session["activity_source"], "none");
        assert_eq!(projection.session["runtime_id"], CLAUDE);
        let activity = state.get(&session).unwrap().activity.clone().unwrap();
        assert_eq!(activity.provider_session_id.as_deref(), Some("provider-1"));
        assert_eq!(activity.transcript_path.as_deref(), Some("/tmp/t.jsonl"));
        assert!(projection.notification.is_none());

        state.apply_core_session(&with_declared(core_row(&session), "working", ""));
        assert_eq!(
            state.get(&session).unwrap().status(),
            "busy",
            "the hook named the agent, the declaration moves it"
        );

        let projection = state
            .apply_core_event(&frame(
                &session,
                "ai.stop",
                "Stop",
                json!({"last_result": "2 files changed"}),
            ))
            .expect("the stop projects");
        assert_eq!(projection.session["status"], "busy");
        assert!(projection.notification.is_none());

        let projections = state.apply_core_session(&with_declared(core_row(&session), "done", ""));
        let notification = projections
            .iter()
            .find_map(|projection| projection.notification.as_ref())
            .expect("done notifies");
        assert_eq!(notification.kind, KIND_FINISHED);
        assert_eq!(
            notification.body.as_deref(),
            Some("2 files changed"),
            "the hook's last result names the finished turn"
        );
    }

    #[test]
    fn an_exited_agent_never_holds_attention_or_busy() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = WorkerState::new(home.path());
        state.apply_core_snapshot(&[with_declared(agent_row(&session), "blocked", "Apply?")]);
        assert_eq!(state.get(&session).unwrap().status(), "attention");

        let projections = state.apply_core_snapshot(&[exited(with_declared(
            agent_row(&session),
            "blocked",
            "Apply?",
        ))]);
        assert_eq!(state.get(&session).unwrap().status(), "idle");
        assert!(notifications(&projections).is_empty(), "{projections:?}");

        state.apply_core_snapshot(&[exited(with_declared(agent_row(&session), "working", ""))]);
        assert_eq!(state.get(&session).unwrap().status(), "idle");
    }

    #[test]
    fn a_rebuilt_worker_restores_the_declared_state_without_replaying_its_notification() {
        let home = tempfile::tempdir().unwrap();
        let blocked = SessionId::new();
        let ended = SessionId::new();
        for (session, state, lifecycle) in [
            (&blocked, "blocked", SessionLifecycle::Running),
            (&ended, "working", SessionLifecycle::Lost),
        ] {
            let mut held = manifest(
                session.clone(),
                Some(hook("Stop", SessionGeneration::FIRST)),
            );
            held.lifecycle = lifecycle;
            held.declared_status = serde_json::from_value(declared(state, "Apply?")).ok();
            write_manifest(home.path(), &held).unwrap();
        }

        let mut restarted = WorkerState::new(home.path());
        restarted.rebuild_from_home(home.path());

        assert_eq!(restarted.get(&blocked).unwrap().status(), "attention");
        assert_ne!(restarted.get(&ended).unwrap().status(), "busy");
        let snapshot = restarted.snapshot();
        assert!(
            snapshot
                .iter()
                .any(|row| row["declared_status"]["state"] == "blocked"),
            "a worker rebuilt from the manifests serves the held status"
        );
        let projections = restarted.apply_core_snapshot(&[with_declared(
            agent_row(&blocked),
            "blocked",
            "Apply?",
        )]);
        assert!(
            notifications(&projections).is_empty(),
            "a status held before the restart is not announced again"
        );
    }

    #[test]
    fn a_session_discovered_mid_turn_takes_its_state_without_a_notification() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = WorkerState::new(home.path());

        let projections =
            state.apply_core_snapshot(&[with_declared(agent_row(&session), "blocked", "Apply?")]);

        assert_eq!(state.get(&session).unwrap().status(), "attention");
        assert!(notifications(&projections).is_empty());
    }

    #[test]
    fn a_declared_activity_ends_when_the_agent_stops_declaring() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = WorkerState::new(home.path());
        state.apply_core_snapshot(&[with_declared(agent_row(&session), "working", "")]);
        assert!(state.get(&session).unwrap().activity.is_some());

        state.apply_core_snapshot(&[agent_row(&session)]);

        assert!(state.get(&session).unwrap().activity.is_none());
    }

    #[test]
    fn a_hook_event_takes_over_a_declared_activity() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = WorkerState::new(home.path());
        state.apply_core_snapshot(&[with_declared(agent_row(&session), "working", "")]);
        let mut prompt = frame_from_pid(&session, "ai.prompt_submit", "UserPromptSubmit", 10);
        prompt["emitted_at_ms"] = json!(now_ms());
        state.apply_core_event(&prompt).expect("the hook projects");

        state.apply_core_snapshot(&[agent_row(&session)]);

        let entry = state.get(&session).unwrap();
        assert_eq!(entry.activity_source, ActivitySource::None);
        assert!(
            entry.activity.is_some(),
            "a hook-owned activity outlives the declaration"
        );
    }

    #[test]
    fn revisioned_host_snapshots_recover_a_lost_event_and_reject_queued_older_events() {
        let home = tempfile::tempdir().unwrap();
        let host = paneflow_host::host::SessionHost::open(home.path(), Path::new("hook-recovery"))
            .unwrap();
        #[cfg(windows)]
        let (shell, args) = ("cmd.exe", vec!["/Q".to_string(), "/D".to_string()]);
        #[cfg(unix)]
        let (shell, args) = ("/bin/sh", Vec::new());
        let created = host
            .create(paneflow_host::host::CreateSession {
                shell: Some(shell.to_string()),
                args,
                cwd: Some(home.path().display().to_string()),
                ..Default::default()
            })
            .unwrap();
        let session = created.manifest.session;
        let snapshot = || {
            host.agent_snapshot()
                .into_iter()
                .map(|entry| serde_json::to_value(entry).unwrap())
                .collect::<Vec<_>>()
        };
        let mut state = WorkerState::new(home.path());
        state.apply_core_snapshot(&snapshot());
        let agents = host.subscribe_agents();
        let accepted = |kind: &str, name: &str, timestamp: u64| {
            let mut raw = frame(&session, kind, name, json!({}));
            raw["runtime_generation"] = json!(1);
            raw["emitted_at_ms"] = json!(timestamp);
            host.ingest_agent_event(&AgentEvent::from_params(&raw).unwrap())
                .unwrap()
        };
        accepted("ai.prompt_submit", "UserPromptSubmit", 10);
        let first = agents.frames.try_recv().unwrap();
        state.apply_core_event(&first).unwrap();
        assert_eq!(state.get(&session).unwrap().hook_revision, 1);
        accepted("ai.notification", "PermissionRequest", 11);
        let second = agents.frames.try_recv().unwrap();

        let projections = state.apply_core_snapshot(&snapshot());
        assert_eq!(
            state.get(&session).unwrap().hook_revision,
            2,
            "the snapshot recovers the event the stream lost"
        );
        assert!(!projections.is_empty());
        assert!(state.apply_core_event(&first).is_none());
        assert!(state.apply_core_event(&second).is_none());
        assert!(state.apply_core_snapshot(&snapshot()).is_empty());

        let mut replacement = WorkerState::new(home.path());
        replacement.rebuild_from_home(home.path());
        assert_eq!(replacement.get(&session).unwrap().hook_revision, 2);
        assert_eq!(
            replacement
                .get(&session)
                .unwrap()
                .activity
                .as_ref()
                .map(|activity| activity.tool.as_str()),
            Some("claude")
        );
        host.stop(&session, None).unwrap();
    }

    #[test]
    fn an_older_generation_snapshot_cannot_roll_back_a_current_worker_row() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = WorkerState::new(home.path());
        state.apply_core_snapshot(&[
            json!({"session": session, "generation": 2, "lifecycle": "running"}),
        ]);
        state.apply_core_snapshot(&[
            json!({"session": session, "generation": 1, "lifecycle": "running"}),
        ]);
        assert_eq!(
            state.get(&session).unwrap().generation,
            SessionGeneration::FIRST.next()
        );
    }

    #[test]
    fn an_unverifiable_pid_stays_non_resumable_and_is_never_signaled() {
        let mut entry = SessionEntry::from_manifest(manifest(SessionId::new(), None));
        assert_eq!(entry.health, Health::NonResumable);

        entry.process = Some(ProcessIdentity {
            pid: std::process::id(),
            started_at: None,
        });
        entry.refresh_health();
        assert_eq!(entry.health, Health::NonResumable);

        entry.process = Some(ProcessIdentity::capture(std::process::id()));
        entry.refresh_health();
        assert_eq!(entry.health, Health::Live);

        entry.process = Some(ProcessIdentity {
            pid: std::process::id(),
            started_at: Some(1),
        });
        entry.refresh_health();
        assert_eq!(entry.health, Health::NonResumable);
    }

    #[test]
    fn a_stopped_session_is_only_named_when_its_recorded_child_is_provably_absent() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut record = manifest(session.clone(), None);
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        record.process = Some(ProcessIdentity::capture(child.id()));
        assert!(child.wait().unwrap().success());
        write_manifest(home.path(), &record).unwrap();

        let mut state = WorkerState::new(home.path());
        state.rebuild_from_home(home.path());
        state.refresh_health();
        let entry = state.get(&session).expect("rebuilt");
        assert_eq!(entry.health, Health::Stopped);
        assert!(!entry.live());
    }

    #[test]
    fn a_session_served_by_an_older_core_carries_a_restart_token() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut record = manifest(session.clone(), None);
        record.host_protocol_version = crate::protocol::REQUIRED_CORE_PROTOCOL - 1;
        write_manifest(home.path(), &record).unwrap();

        let mut state = WorkerState::new(home.path());
        state.rebuild_from_home(home.path());
        let value = state.get(&session).expect("rebuilt").to_value();
        assert_eq!(
            value["restart_recommended"]["token"],
            crate::protocol::RESTART_RECOMMENDED
        );
    }

    #[test]
    fn a_core_snapshot_adds_a_session_created_after_the_worker_started() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let process = ProcessIdentity::capture(std::process::id());
        let raw = json!({
            "session": session,
            "generation": SessionGeneration::FIRST,
            "generation_started_at_ms": LAUNCHED_AT,
            "live": true,
            "lifecycle": SessionLifecycle::Running,
            "process": process,
            "host_protocol_version": paneflow_host::HOST_PROTOCOL_VERSION,
            "host_build_id": "test-build",
            "last_hook": hook("UserPromptSubmit", SessionGeneration::FIRST),
        });
        let mut state = WorkerState::new(home.path());
        state.apply_core_snapshot(&[raw]);
        let entry = state
            .get(&session)
            .expect("the new core session is projected");
        assert_eq!(entry.health, Health::Live);
        assert_eq!(entry.status(), "idle");
        assert_eq!(entry.activity_source, ActivitySource::None);
        assert_eq!(entry.activity.as_ref().unwrap().tool, "claude");
    }

    fn adopted_row(session: &SessionId, lifecycle: SessionLifecycle, stale: bool) -> Value {
        let mut agent = AgentSummary::declared("claude", 1);
        agent.state = AgentState::Thinking.wire_str().to_string();
        agent.stale = stale;
        json!({
            "session": session,
            "generation": SessionGeneration::FIRST,
            "live": lifecycle.is_running(),
            "lifecycle": lifecycle,
            "agent": agent,
        })
    }

    fn adopted_activity(row: Value, session: &SessionId) -> (AgentSummary, &'static str) {
        let home = tempfile::tempdir().unwrap();
        let mut state = WorkerState::new(home.path());
        state.apply_core_snapshot(&[row]);
        let entry = state.get(session).expect("the adopted session is kept");
        (
            entry
                .activity
                .clone()
                .expect("the adopted activity is kept"),
            entry.status(),
        )
    }

    #[test]
    fn a_snapshot_marks_busy_activity_of_an_ended_session_stale() {
        let session = SessionId::new();
        let (ended, ended_status) = adopted_activity(
            adopted_row(&session, SessionLifecycle::Lost, false),
            &session,
        );
        assert!(ended.stale);
        assert_ne!(ended_status, "busy");

        let (live, _) = adopted_activity(
            adopted_row(&session, SessionLifecycle::Running, false),
            &session,
        );
        assert!(!live.stale);
    }

    #[test]
    fn a_stale_flag_reported_by_an_older_host_is_left_unchanged() {
        let session = SessionId::new();
        for lifecycle in [SessionLifecycle::Running, SessionLifecycle::Lost] {
            let (reported, _) = adopted_activity(adopted_row(&session, lifecycle, true), &session);
            assert!(reported.stale);
        }
    }

    #[test]
    fn two_agents_of_one_tool_keep_their_own_rows_and_closing_one_pane_keeps_the_other() {
        let home = tempfile::tempdir().unwrap();
        let (first, second) = (SessionId::new(), SessionId::new());
        for session in [&first, &second] {
            write_manifest(home.path(), &manifest(session.clone(), None)).unwrap();
        }
        let mut state = WorkerState::new(home.path());
        state.rebuild_from_home(home.path());
        for (session, pid) in [(&first, 101), (&second, 102)] {
            state.apply_core_event(&frame_from_pid(
                session,
                "ai.prompt_submit",
                "UserPromptSubmit",
                pid,
            ));
        }
        state.apply_core_snapshot(&[
            with_declared(core_row(&first), "done", ""),
            with_declared(core_row(&second), "working", ""),
        ]);
        assert_eq!(state.get(&first).unwrap().status(), "idle");
        assert_eq!(state.get(&second).unwrap().status(), "busy");

        state.apply_core_snapshot(&[with_declared(core_row(&second), "working", "")]);
        assert!(state.get(&first).is_none(), "the closed pane's row is gone");
        let kept = state.get(&second).expect("the other pane's row stays");
        assert_eq!(kept.status(), "busy");
        assert_eq!(kept.activity.as_ref().unwrap().pid, Some(102));
    }

    #[test]
    fn a_runtime_observed_without_any_activity_is_announced_once_and_again_when_it_leaves() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = running_state(home.path(), &session);
        let observed_row = |runtime_id: Option<&str>| {
            let mut row = core_row(&session);
            if let Some(runtime_id) = runtime_id {
                row["observed_runtime"] =
                    serde_json::to_value(observation(runtime_id, 40, 7)).unwrap();
            }
            row
        };
        state.apply_core_snapshot(&[observed_row(None)]);

        let observed = state.apply_core_session(&observed_row(Some("com.sourcegraph.amp")));
        let projection = observed
            .iter()
            .find(|projection| projection.session["session"] == json!(session))
            .expect("the observed runtime reaches the desktop");
        assert_eq!(
            projection.session["foreground_runtime_id"],
            "com.sourcegraph.amp"
        );
        assert_eq!(projection.session["activity"], Value::Null);

        assert!(
            state
                .apply_core_session(&observed_row(Some("com.sourcegraph.amp")))
                .is_empty(),
            "an unchanged observation is not announced again"
        );

        let left = state.apply_core_session(&observed_row(None));
        let projection = left
            .iter()
            .find(|projection| projection.session["session"] == json!(session))
            .expect("the agent leaving reaches the desktop");
        assert_eq!(projection.session["foreground_runtime_id"], Value::Null);
        assert_eq!(
            projection.session["runtime_id"], "com.sourcegraph.amp",
            "the last observed runtime stays named while only the foreground leaves"
        );
    }

    #[test]
    fn state_seq_counts_each_reduced_state_transition_and_nothing_else() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = WorkerState::new(home.path());
        state.apply_core_snapshot(&[agent_row(&session)]);
        let start = state.get(&session).unwrap().state_seq;
        state.apply_core_session(&with_declared(agent_row(&session), "working", ""));
        assert_eq!(state.get(&session).unwrap().state_seq, start + 1);
        state.sweep();
        state.apply_core_session(&with_declared(agent_row(&session), "working", ""));
        assert_eq!(
            state.get(&session).unwrap().state_seq,
            start + 1,
            "a sweep or a repeated declaration is not a transition"
        );
        let projections = state.apply_core_session(&with_declared(agent_row(&session), "done", ""));
        let projection = projections
            .iter()
            .find(|projection| projection.session["session"] == json!(session))
            .expect("the completion projects");
        assert_eq!(projection.session["state_seq"], start + 2);
        assert_eq!(projection.session["status"], "idle");
    }
}
