use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use paneflow_agent_config::runtime_catalog::{Runtime, RuntimeLifecycleSource};
use paneflow_config::schema::{SessionGeneration, SessionId, WorkspaceId};
use paneflow_host::agent::AgentEvent;
use paneflow_host::manifest::{SessionLifecycle, SessionManifest};
use paneflow_host::{ProcessIdentity, ProcessVerdict};
use paneflow_ipc_client::agent::{AgentState, next_waiting_since};
use serde_json::{Value, json};

use crate::activity::{AgentDecision, AgentSummary, apply_event};
use crate::hook_assets;
use crate::hook_state::{ActivityEngine, HookEventInput, HookState, Notice, Outcome, at_unix_ms};
use crate::notifications::{ActivityLog, Notification, notification_for};
use crate::protocol::restart_recommendation;

pub const MAX_SEED_BYTES: u64 = hook_assets::MAX_SEED_BYTES;

pub const SCREEN_WORKING: &str = "working";
pub const SCREEN_IDLE: &str = "idle";

pub type MenuEvidence<'a> = dyn Fn(&SessionId) -> Option<bool> + 'a;

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

    pub fn is_hooks(self) -> bool {
        matches!(self, Self::Hooks)
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
    pub generation_started_at_ms: Option<u64>,
    launch_hook_capable: bool,
    pub workspace: Option<WorkspaceId>,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub lifecycle: SessionLifecycle,
    pub process: Option<ProcessIdentity>,
    pub core_protocol: u32,
    pub core_build_id: String,
    pub activity: Option<AgentSummary>,
    pub activity_source: ActivitySource,
    pub status: Status,
    pub outcome: Option<String>,
    pub health: Health,
    pub activity_signal: u64,
    pub output_changed_at_ms: Option<u64>,
    pub screen_activity: Option<String>,
    pub menu_prompt_active: bool,
    pub observed_runtime: Option<String>,
    pub updated_at_ms: u64,
}

impl SessionEntry {
    fn from_manifest(manifest: SessionManifest) -> Self {
        let launch_hook_capable = command_is_hook_capable(&manifest.launch.shell);
        let mut entry = Self {
            session: manifest.session,
            generation: manifest.generation,
            generation_started_at_ms: manifest.generation_started_at_ms,
            launch_hook_capable,
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
            activity_source: ActivitySource::None,
            status: Status::Idle,
            outcome: None,
            health: Health::NonResumable,
            activity_signal: manifest.screen_changed_at_ms.unwrap_or_default(),
            output_changed_at_ms: None,
            screen_activity: manifest.screen_activity,
            menu_prompt_active: manifest.menu_prompt_active,
            observed_runtime: manifest.observed_runtime,
            updated_at_ms: manifest.updated_at_ms,
        };
        entry.refresh_health();
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
        self.status.wire_str()
    }

    pub fn runtime(&self) -> Option<&'static Runtime> {
        self.activity
            .as_ref()
            .and_then(|summary| runtime_for_tool(&summary.tool))
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
            "outcome": self.outcome,
            "runtime_id": self.runtime().map(|runtime| runtime.id),
            "menu_prompt_active": self.menu_prompt_active,
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

pub fn runtime_for_tool(tool: &str) -> Option<&'static Runtime> {
    paneflow_agent_config::runtime_catalog::runtime_by_command_alias(tool)
        .or_else(|| paneflow_agent_config::runtime_catalog::runtime_by_slug(tool))
        .or_else(|| paneflow_agent_config::runtime_catalog::runtime_by_process_alias(tool))
        .or_else(|| paneflow_agent_config::runtime_catalog::runtime_by_id(tool))
}

fn screen_fallback(entry: &SessionEntry) -> Option<Status> {
    let runtime = entry.runtime()?;
    runtime.screen?;
    match entry.screen_activity.as_deref()? {
        SCREEN_WORKING => Some(Status::Busy),
        SCREEN_IDLE => Some(Status::Idle),
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
    engine: ActivityEngine,
    log: ActivityLog,
    menu_attention_detection: bool,
}

impl WorkerState {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self {
            home: home.into(),
            sessions: BTreeMap::new(),
            engine: ActivityEngine::new(),
            log: ActivityLog::default(),
            menu_attention_detection: true,
        }
    }

    pub fn set_menu_attention_detection(&mut self, enabled: bool) {
        self.menu_attention_detection = enabled;
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

    pub fn activity_log(&self) -> &ActivityLog {
        &self.log
    }

    pub fn snapshot(&self) -> Vec<Value> {
        self.sessions.values().map(SessionEntry::to_value).collect()
    }

    fn session_dir(&self, session: &SessionId) -> PathBuf {
        paneflow_home::host_session_data_dir_in(&self.home, session.as_str())
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
            let entry = SessionEntry::from_manifest(manifest);
            rebuilt.insert(entry.session.clone(), entry);
        }
        self.sessions = rebuilt;
        let live: BTreeSet<SessionId> = self.sessions.keys().cloned().collect();
        self.engine.retain_sessions(&live);
        let now = SystemTime::now();
        for session in live {
            self.derive(&session, now, &|_| None);
        }
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

    pub fn apply_core_snapshot(&mut self, entries: &[Value]) -> Vec<Projection> {
        let mut seen = BTreeSet::new();
        let mut discovered = BTreeSet::new();
        for raw in entries {
            let Some(session) = raw["session"]
                .as_str()
                .and_then(|id| SessionId::parse(id).ok())
            else {
                continue;
            };
            seen.insert(session.clone());
            let held = self.sessions.remove(&session);
            if held.is_none() {
                discovered.insert(session.clone());
            }
            let mut entry = merge_core_row(session, raw, held);
            entry.refresh_health();
            self.sessions.insert(entry.session.clone(), entry);
        }
        self.sessions.retain(|session, _| seen.contains(session));
        self.engine.retain_sessions(&seen);
        let now = SystemTime::now();
        seen.into_iter()
            .filter_map(|session| {
                let projection = self.derive(&session, now, &|_| None)?;
                let announce = (projection.changed && !discovered.contains(&session))
                    || projection.notification.is_some();
                announce.then_some(projection)
            })
            .collect()
    }

    pub fn sweep(&mut self, now: SystemTime, menu_evidence: &MenuEvidence<'_>) -> Vec<Projection> {
        let sessions: Vec<SessionId> = self.sessions.keys().cloned().collect();
        sessions
            .into_iter()
            .filter_map(|session| {
                let projection = self.derive(&session, now, menu_evidence)?;
                (projection.changed || projection.notification.is_some()).then_some(projection)
            })
            .collect()
    }

    pub fn apply_core_event(&mut self, frame: &Value) -> Option<Projection> {
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
        let now_ms = now_ms();
        let session = event.session.clone();
        let previous_activity = entry.activity.clone();
        let previous_updated_at_ms = entry.updated_at_ms;
        match apply_event(entry.activity.as_ref(), &event, now_ms) {
            AgentDecision::Stale(reason) => {
                log::debug!("paneflow-serve: refused an event for {session}: {reason}");
                return None;
            }
            AgentDecision::Clear => {
                entry.activity = None;
                entry.activity_source = ActivitySource::None;
                entry.status = Status::Idle;
                entry.outcome = None;
                entry.updated_at_ms = now_ms;
                self.engine.remove_session(&session);
                let value = self.sessions.get(&session)?.to_value();
                return Some(Projection {
                    session: value,
                    notification: None,
                    changed: true,
                });
            }
            AgentDecision::Update(summary) => {
                entry.activity = Some(*summary);
                entry.updated_at_ms = now_ms;
            }
        }

        let generation = entry.generation.get();
        let generation_started_at_ms = entry.generation_started_at_ms;
        let launch_hook_capable = entry.launch_hook_capable;
        let observed_runtime = entry.observed_runtime.clone();
        let hook_event_name = frame["hook_payload"]["hook_event_name"]
            .as_str()
            .unwrap_or_else(|| event.kind.wire_str())
            .to_string();
        let notification_type = frame["hook_payload"]["notification_type"]
            .as_str()
            .map(str::to_owned);
        let failure_reason = frame["hook_payload"]["reason"].as_str().map(str::to_owned);
        let background_tasks_pending = payload_has_background_tasks(&frame["hook_payload"]);
        let raw_name = if event.is_interrupt() {
            crate::hook_state::EVENT_STOP_CANCELLED.to_string()
        } else {
            hook_event_name
        };
        let dir = self.session_dir(&session);
        self.engine.bind_session_dir(&session, &dir);
        if !launch_hook_capable {
            self.engine
                .observe_foreground_runtime(&session, observed_runtime.as_deref());
        }
        let accepted = self.engine.apply_hook_event(
            &session,
            HookEventInput {
                raw_name: &raw_name,
                tool_name: event.tool_name.as_deref(),
                notification_type: notification_type.as_deref(),
                failure_reason: failure_reason.as_deref(),
                background_tasks_pending,
                event_generation: event.generation.map(SessionGeneration::get),
                current_generation: generation,
                generation_started_at_ms,
            },
            at_unix_ms(now_ms),
        );
        if !accepted {
            log::debug!(
                "paneflow-serve: the reducer refused {raw_name} for {session}: stale runtime provenance"
            );
            if let Some(entry) = self.sessions.get_mut(&session) {
                entry.activity = previous_activity;
                entry.updated_at_ms = previous_updated_at_ms;
            }
            return None;
        }
        self.derive(&session, at_unix_ms(now_ms), &|_| None)
    }

    fn derive(
        &mut self,
        session: &SessionId,
        now: SystemTime,
        menu_evidence: &MenuEvidence<'_>,
    ) -> Option<Projection> {
        let Some(entry) = self.sessions.get(session) else {
            self.engine.remove_session(session);
            return None;
        };
        let runtime = entry.runtime();
        let hook_capable = runtime
            .is_some_and(|runtime| runtime.lifecycle.source == RuntimeLifecycleSource::Hooks);
        let launch_hook_capable = entry.launch_hook_capable;
        let attention_clears_on_output = runtime
            .map(|runtime| runtime.lifecycle.attention_clears_on_output)
            .unwrap_or(true);
        let anchor_start_event_to_output = runtime
            .map(|runtime| runtime.lifecycle.anchor_start_event_to_output)
            .unwrap_or(true);
        let generation = entry.generation.get();
        let generation_started_at_ms = entry.generation_started_at_ms;
        let activity_signal = entry.activity_signal;
        let menu_prompt_active = entry.menu_prompt_active;
        let observed_runtime = entry.observed_runtime.clone();
        let running = entry.lifecycle.is_running();
        let errored = entry
            .activity
            .as_ref()
            .is_some_and(|summary| summary.errored);
        let dir = self.session_dir(session);

        self.engine
            .observe_runtime_launch(session, generation, generation_started_at_ms);
        if !launch_hook_capable {
            self.engine
                .observe_foreground_runtime(session, observed_runtime.as_deref());
        }
        self.engine.bind_session_dir(session, &dir);
        if hook_capable {
            self.engine.seed_from_disk(
                session,
                &dir,
                anchor_start_event_to_output,
                generation_started_at_ms,
                generation,
                entry.output_changed_at_ms,
            );
        }
        self.engine
            .observe_menu_prompt(session, menu_prompt_active, now);

        let mut source = ActivitySource::None;
        let mut status = if self.engine.is_latched(session) {
            let mut allow_attention_clear = attention_clears_on_output;
            if allow_attention_clear
                && self
                    .engine
                    .attention_has_new_output(session, activity_signal)
            {
                match menu_evidence(session) {
                    Some(menu_visible) => allow_attention_clear = !menu_visible,
                    None => {
                        return self.finish(
                            session,
                            Status::Attention,
                            ActivitySource::Hooks,
                            errored,
                            now,
                        );
                    }
                }
            }
            self.engine
                .note_output_and_sweep(session, activity_signal, allow_attention_clear, now);
            source = ActivitySource::Hooks;
            match self.engine.hook_owned_state(session) {
                Some(HookState::Busy) => Status::Busy,
                Some(HookState::Attention) => Status::Attention,
                Some(HookState::Idle) | None => Status::Idle,
            }
        } else {
            self.engine.clear_output_baseline(session);
            let entry = self.sessions.get(session)?;
            match screen_fallback(entry) {
                Some(verdict) => {
                    source = ActivitySource::Screen;
                    verdict
                }
                None => Status::Idle,
            }
        };

        if self.menu_attention_detection
            && menu_prompt_active
            && matches!(status, Status::Busy | Status::Idle)
        {
            status = Status::Attention;
        }
        if !running && status == Status::Busy {
            status = Status::Idle;
        }
        self.finish(session, status, source, errored, now)
    }

    fn finish(
        &mut self,
        session: &SessionId,
        status: Status,
        source: ActivitySource,
        errored: bool,
        now: SystemTime,
    ) -> Option<Projection> {
        let status = if errored && status == Status::Idle {
            Status::Errored
        } else {
            status
        };
        let notice = self.engine.take_notice(session);
        let outcome = self.engine.outcome(session);
        let generation = self.sessions.get(session)?.generation;
        let outcome_changed = self
            .sessions
            .get(session)
            .is_some_and(|entry| entry.outcome != outcome.as_ref().map(Outcome::wire_string));
        if outcome_changed && let Some(outcome) = outcome.as_ref() {
            self.log.record(
                session,
                generation,
                outcome,
                crate::hook_state::unix_ms(now),
            );
        }
        let entry = self.sessions.get_mut(session)?;
        let wire_outcome = outcome.as_ref().map(Outcome::wire_string);
        let state = status.agent_state();
        let settled = entry.status == status
            && entry.activity_source == source
            && entry.outcome == wire_outcome
            && entry
                .activity
                .as_ref()
                .is_none_or(|summary| summary.state == state.wire_str());
        entry.status = status;
        entry.activity_source = source;
        entry.outcome = wire_outcome;
        let now_ms = crate::hook_state::unix_ms(now);
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
        let runtime_label = entry.runtime_label();
        let body = entry.activity.as_ref().and_then(|summary| match notice {
            Some(Notice::Finished) => summary.last_result.clone(),
            Some(Notice::NeedsInput) => summary.message.clone(),
            None => None,
        });
        let value = entry.to_value();
        let notification = notice.and_then(|notice| {
            notification_for(notice, source.is_hooks(), &runtime_label, body.as_deref())
        });
        Some(Projection {
            session: value,
            notification,
            changed: !settled,
        })
    }
}

fn source_wire(source: ActivitySource) -> &'static str {
    match source {
        ActivitySource::Hooks => "hook",
        ActivitySource::Screen => "terminal",
        ActivitySource::None => "hook",
    }
}

fn command_is_hook_capable(command: &str) -> bool {
    let alias = Path::new(command)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(command);
    runtime_for_tool(alias)
        .is_some_and(|runtime| runtime.lifecycle.source == RuntimeLifecycleSource::Hooks)
}

fn payload_has_background_tasks(payload: &Value) -> bool {
    match &payload["background_tasks"] {
        Value::Array(tasks) => !tasks.is_empty(),
        Value::Number(count) => count.as_u64().is_some_and(|count| count > 0),
        Value::Bool(pending) => *pending,
        _ => false,
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
    let same_generation = held
        .as_ref()
        .filter(|entry| entry.generation == generation)
        .is_some();
    let held_activity = same_generation
        .then(|| held.as_ref().and_then(|entry| entry.activity.clone()))
        .flatten();
    let process = serde_json::from_value(raw["process"].clone())
        .ok()
        .or_else(|| held.as_ref().and_then(|entry| entry.process));
    let declared = raw["last_hook"]["tool"]
        .as_str()
        .map(|tool| AgentSummary::declared(tool, now_ms()));
    SessionEntry {
        session,
        generation,
        generation_started_at_ms: raw["generation_started_at_ms"].as_u64().or_else(|| {
            held.as_ref()
                .and_then(|entry| entry.generation_started_at_ms)
        }),
        launch_hook_capable: raw["launch_shell"]
            .as_str()
            .map(command_is_hook_capable)
            .or_else(|| held.as_ref().map(|entry| entry.launch_hook_capable))
            .unwrap_or(false),
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
        activity: held_activity
            .or(declared)
            .or_else(|| serde_json::from_value::<AgentSummary>(raw["agent"].clone()).ok()),
        activity_source: same_generation
            .then(|| held.as_ref().map(|entry| entry.activity_source))
            .flatten()
            .unwrap_or(ActivitySource::None),
        status: same_generation
            .then(|| held.as_ref().map(|entry| entry.status))
            .flatten()
            .unwrap_or(Status::Idle),
        outcome: same_generation
            .then(|| held.as_ref().and_then(|entry| entry.outcome.clone()))
            .flatten(),
        health: Health::NonResumable,
        activity_signal: raw["screen_changed_at_ms"].as_u64().unwrap_or_default(),
        output_changed_at_ms: raw["output_changed_at_ms"].as_u64(),
        screen_activity: raw["screen_activity"].as_str().map(str::to_owned),
        menu_prompt_active: raw["menu_prompt_active"].as_bool().unwrap_or_default(),
        observed_runtime: raw["observed_runtime"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| {
                held.as_ref()
                    .and_then(|entry| entry.observed_runtime.clone())
            }),
        updated_at_ms: raw["updated_at_ms"].as_u64().unwrap_or_else(now_ms),
    }
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
    use crate::hook_state::HOOK_IDLE_TIMEOUT;
    use crate::notifications::{KIND_FINISHED, KIND_NEEDS_INPUT};
    use paneflow_config::schema::HostInstanceToken;
    use paneflow_host::manifest::{
        HookRecord, MANIFEST_SCHEMA_VERSION, SessionLaunch, write_manifest,
    };

    const LAUNCHED_AT: u64 = 1_000;

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
            generation_started_at_ms: Some(LAUNCHED_AT),
            screen_changed_at_ms: None,
            screen_activity: None,
            menu_prompt_active: false,
            observed_runtime: None,
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

    fn seed(dir: &Path, name: &str, generation: u64) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join(hook_assets::SEED_FILE),
            serde_json::to_vec(&json!({"hook_event_name": name, "runtime_generation": generation}))
                .unwrap(),
        )
        .unwrap();
    }

    fn set_seed_modified_at(dir: &Path, modified_at: SystemTime) {
        let seed = std::fs::File::options()
            .write(true)
            .open(dir.join(hook_assets::SEED_FILE))
            .unwrap();
        seed.set_times(std::fs::FileTimes::new().set_modified(modified_at))
            .unwrap();
    }

    fn running_state(home: &Path, session: &SessionId) -> WorkerState {
        write_manifest(home, &manifest(session.clone(), None)).unwrap();
        let mut state = WorkerState::new(home);
        state.rebuild_from_home(home);
        state
    }

    #[test]
    fn a_prompt_latches_the_session_busy_and_a_stop_completes_it_with_one_notification() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = running_state(home.path(), &session);

        let busy = state
            .apply_core_event(&frame(
                &session,
                "ai.prompt_submit",
                "UserPromptSubmit",
                json!({}),
            ))
            .expect("the opening hook projects");
        assert_eq!(busy.session["status"], "busy");
        assert_eq!(busy.session["activity_source"], "hooks");
        assert_eq!(busy.session["runtime_id"], "com.anthropic.claude-code");
        assert!(busy.notification.is_none());

        let done = state
            .apply_core_event(&frame(
                &session,
                "ai.stop",
                "Stop",
                json!({"last_result": "3 files changed"}),
            ))
            .expect("the stop projects");
        assert_eq!(done.session["status"], "idle");
        assert_eq!(done.session["outcome"], "completed");
        let notification = done.notification.expect("a real completion notifies");
        assert_eq!(notification.kind, KIND_FINISHED);
        assert_eq!(notification.runtime_label, "Claude Code");
        assert_eq!(notification.body.as_deref(), Some("3 files changed"));
        assert_eq!(
            state
                .activity_log()
                .entries()
                .map(|entry| entry.outcome.as_str())
                .collect::<Vec<_>>(),
            vec!["completed"]
        );

        state.apply_core_event(&frame(
            &session,
            "ai.prompt_submit",
            "UserPromptSubmit",
            json!({}),
        ));
        let second = state
            .apply_core_event(&frame(
                &session,
                "ai.stop",
                "Stop",
                json!({"last_assistant_message": "second turn"}),
            ))
            .expect("the second stop projects");
        assert_eq!(
            second.notification.unwrap().body.as_deref(),
            Some("second turn")
        );
        assert_eq!(state.activity_log().len(), 2);
    }

    #[test]
    fn a_stop_failure_settles_without_a_notification_and_records_its_reason() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = running_state(home.path(), &session);
        state.apply_core_event(&frame(
            &session,
            "ai.prompt_submit",
            "UserPromptSubmit",
            json!({}),
        ));

        let failed = state
            .apply_core_event(&frame(
                &session,
                "ai.stop",
                "StopFailure",
                json!({"reason": "matcher"}),
            ))
            .expect("the failure projects");
        assert_eq!(failed.session["status"], "idle");
        assert_eq!(failed.session["outcome"], "failed:matcher");
        assert!(failed.notification.is_none());
    }

    #[test]
    fn a_permission_request_asks_for_input_once_and_names_its_message() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = running_state(home.path(), &session);
        state.apply_core_event(&frame(
            &session,
            "ai.prompt_submit",
            "UserPromptSubmit",
            json!({}),
        ));

        let asking = state
            .apply_core_event(&frame(
                &session,
                "ai.notification",
                "PermissionRequest",
                json!({"message": "Approve edit?"}),
            ))
            .expect("the permission request projects");
        assert_eq!(asking.session["status"], "attention");
        let notification = asking.notification.expect("a need for input notifies");
        assert_eq!(notification.kind, KIND_NEEDS_INPUT);
        assert_eq!(notification.body.as_deref(), Some("Approve edit?"));

        let again = state
            .apply_core_event(&frame(
                &session,
                "ai.notification",
                "Notification",
                json!({"notification_type": "permission_prompt"}),
            ))
            .expect("the duplicate still projects");
        assert!(
            again.notification.is_none(),
            "two signals for one question are one need for input"
        );
    }

    #[test]
    fn a_subagent_edge_latches_without_completing_the_main_turn() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = running_state(home.path(), &session);
        state.apply_core_event(&frame(
            &session,
            "ai.prompt_submit",
            "UserPromptSubmit",
            json!({}),
        ));
        let child = state
            .apply_core_event(&frame(&session, "ai.session_start", "HookSeen", json!({})))
            .expect("the child edge projects");
        assert_eq!(child.session["status"], "busy");
        assert!(child.notification.is_none());
    }

    #[test]
    fn a_stop_carrying_background_tasks_keeps_the_pane_busy_until_the_list_empties() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = running_state(home.path(), &session);
        state.apply_core_event(&frame(
            &session,
            "ai.prompt_submit",
            "UserPromptSubmit",
            json!({}),
        ));

        let pending = state
            .apply_core_event(&frame(
                &session,
                "ai.stop",
                "Stop",
                json!({"background_tasks": ["build"]}),
            ))
            .expect("the stop projects");
        assert_eq!(pending.session["status"], "busy");
        assert_eq!(pending.session["activity_source"], "hooks");
        assert!(pending.notification.is_none());

        let settled = state
            .apply_core_event(&frame(
                &session,
                "ai.stop",
                "Stop",
                json!({"background_tasks": []}),
            ))
            .expect("the final stop projects");
        assert_eq!(settled.session["status"], "idle");
        assert!(settled.notification.is_some());
    }

    #[test]
    fn a_quarantined_legacy_stop_has_no_summary_side_effects() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = running_state(home.path(), &session);
        let entry = state.sessions.get_mut(&session).unwrap();
        entry.generation = SessionGeneration::FIRST.next();
        entry.generation_started_at_ms = Some(now_ms());

        let rejected = state.apply_core_event(&frame(
            &session,
            "ai.stop",
            "Stop",
            json!({"last_assistant_message": "stale result"}),
        ));
        assert!(rejected.is_none());
        let entry = state.get(&session).unwrap();
        assert_eq!(entry.status(), "idle");
        assert!(
            entry
                .activity
                .as_ref()
                .is_none_or(|activity| activity.last_result.is_none()),
            "a rejected event cannot alter controller metadata"
        );
    }

    #[test]
    fn a_lost_stop_expires_after_the_lease_without_a_completion_notification() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = running_state(home.path(), &session);
        std::fs::create_dir_all(paneflow_home::host_session_data_dir_in(
            home.path(),
            session.as_str(),
        ))
        .unwrap();
        state.apply_core_event(&frame(
            &session,
            "ai.prompt_submit",
            "UserPromptSubmit",
            json!({}),
        ));
        assert_eq!(state.get(&session).unwrap().status(), "busy");

        let later = SystemTime::now() + HOOK_IDLE_TIMEOUT + HOOK_IDLE_TIMEOUT;
        state.sweep(SystemTime::now(), &|_| Some(false));
        let settled = state.sweep(later, &|_| Some(false));
        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].session["status"], "idle");
        assert_eq!(settled[0].session["outcome"], "expired");
        assert!(settled[0].notification.is_none());
        assert!(
            state
                .session_dir(&session)
                .join(hook_assets::EXPIRY_FILE)
                .is_file(),
            "the expiry watermark survives a restart"
        );
    }

    #[test]
    fn raw_output_never_rearms_the_screen_change_lease() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = running_state(home.path(), &session);
        state.apply_core_event(&frame(
            &session,
            "ai.prompt_submit",
            "UserPromptSubmit",
            json!({}),
        ));
        let opened_at = SystemTime::now();
        state.sweep(opened_at, &|_| Some(false));
        state
            .sessions
            .get_mut(&session)
            .unwrap()
            .output_changed_at_ms = Some(now_ms() + 1_000);

        let settled = state.sweep(
            opened_at + HOOK_IDLE_TIMEOUT + std::time::Duration::from_secs(1),
            &|_| Some(false),
        );
        assert_eq!(settled[0].session["status"], "idle");
        assert_eq!(settled[0].session["outcome"], "expired");
    }

    #[test]
    fn a_failed_menu_read_keeps_attention_and_its_output_baseline() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = running_state(home.path(), &session);
        state.apply_core_event(&frame(
            &session,
            "ai.prompt_submit",
            "UserPromptSubmit",
            json!({}),
        ));
        state.apply_core_event(&frame(
            &session,
            "ai.notification",
            "PermissionRequest",
            json!({}),
        ));
        state.sweep(SystemTime::now(), &|_| Some(false));

        state.sessions.get_mut(&session).unwrap().activity_signal = 99;
        let blind = state.sweep(SystemTime::now(), &|_| None);
        assert!(blind.iter().all(|row| row.session["status"] == "attention"));
        assert_eq!(state.get(&session).unwrap().status(), "attention");

        state.sessions.get_mut(&session).unwrap().activity_signal = 100;
        let menu_gone = state.sweep(SystemTime::now(), &|_| Some(false));
        assert_eq!(menu_gone[0].session["status"], "busy");
    }

    #[test]
    fn a_visible_menu_never_lets_changed_output_clear_the_question() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = running_state(home.path(), &session);
        state.apply_core_event(&frame(
            &session,
            "ai.prompt_submit",
            "UserPromptSubmit",
            json!({}),
        ));
        state.apply_core_event(&frame(
            &session,
            "ai.notification",
            "PermissionRequest",
            json!({}),
        ));
        state.sweep(SystemTime::now(), &|_| Some(true));
        state.sessions.get_mut(&session).unwrap().activity_signal = 55;
        state.sweep(SystemTime::now(), &|_| Some(true));
        assert_eq!(state.get(&session).unwrap().status(), "attention");
    }

    #[test]
    fn a_restart_rebuilds_a_busy_turn_from_its_seed_and_leaves_a_bare_session_idle() {
        let home = tempfile::tempdir().unwrap();
        let busy = SessionId::new();
        let bare = SessionId::new();
        write_manifest(
            home.path(),
            &manifest(
                busy.clone(),
                Some(hook("UserPromptSubmit", SessionGeneration::FIRST)),
            ),
        )
        .unwrap();
        write_manifest(home.path(), &manifest(bare.clone(), None)).unwrap();
        seed(
            &paneflow_home::host_session_data_dir_in(home.path(), busy.as_str()),
            "UserPromptSubmit",
            1,
        );

        let mut state = WorkerState::new(home.path());
        assert_eq!(state.rebuild_from_home(home.path()), 2);
        let replayed = state.get(&busy).expect("the busy session is rebuilt");
        assert_eq!(replayed.status(), "busy");
        assert_eq!(replayed.activity_source, ActivitySource::Hooks);

        let untouched = state.get(&bare).expect("a session with no seed is rebuilt");
        assert_eq!(untouched.status(), "idle");
        assert_eq!(untouched.activity_source, ActivitySource::None);
    }

    #[test]
    fn a_seed_holding_a_stop_never_reopens_the_turn() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        write_manifest(
            home.path(),
            &manifest(
                session.clone(),
                Some(hook("Stop", SessionGeneration::FIRST)),
            ),
        )
        .unwrap();
        seed(
            &paneflow_home::host_session_data_dir_in(home.path(), session.as_str()),
            "Stop",
            1,
        );

        let mut state = WorkerState::new(home.path());
        state.rebuild_from_home(home.path());
        assert_eq!(state.get(&session).unwrap().status(), "idle");
        assert_eq!(
            state.get(&session).unwrap().outcome.as_deref(),
            Some("completed")
        );
    }

    #[test]
    fn a_seed_from_a_generation_the_session_has_left_is_never_replayed() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut record = manifest(
            session.clone(),
            Some(hook("UserPromptSubmit", SessionGeneration::FIRST)),
        );
        record.generation = SessionGeneration::FIRST.next();
        write_manifest(home.path(), &record).unwrap();
        seed(
            &paneflow_home::host_session_data_dir_in(home.path(), session.as_str()),
            "UserPromptSubmit",
            1,
        );

        let mut state = WorkerState::new(home.path());
        state.rebuild_from_home(home.path());
        assert_eq!(
            state.get(&session).unwrap().status(),
            "idle",
            "a seed the session has left never speaks for the new generation"
        );
    }

    #[test]
    fn an_expiry_watermark_keeps_a_replayed_turn_idle_after_a_restart() {
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
        let dir = paneflow_home::host_session_data_dir_in(home.path(), session.as_str());
        seed(&dir, "UserPromptSubmit", 1);
        hook_assets::record_hook_expiry(&dir, 1, SystemTime::now() + HOOK_IDLE_TIMEOUT).unwrap();

        let mut state = WorkerState::new(home.path());
        state.rebuild_from_home(home.path());
        assert_eq!(state.get(&session).unwrap().status(), "idle");
    }

    #[test]
    fn a_cancellation_fence_is_restored_before_the_seed_is_applied() {
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
        let dir = paneflow_home::host_session_data_dir_in(home.path(), session.as_str());
        seed(&dir, "UserPromptSubmit", 1);
        std::fs::write(
            dir.join(hook_assets::CANCELLATION_FILE),
            serde_json::to_vec(&json!({
                "runtime_generation": 1,
                "cancelled_at": now_ms() + 60_000,
            }))
            .unwrap(),
        )
        .unwrap();

        let mut state = WorkerState::new(home.path());
        state.rebuild_from_home(home.path());
        assert_eq!(
            state.get(&session).unwrap().status(),
            "idle",
            "an escape fence settles the pane until the next submission"
        );
    }

    #[test]
    fn a_new_foreground_agent_drops_the_latch_the_previous_one_left_behind() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = running_state(home.path(), &session);
        state.sessions.get_mut(&session).unwrap().observed_runtime =
            Some("com.anthropic.claude-code:10:5".to_string());
        state.apply_core_event(&frame(
            &session,
            "ai.prompt_submit",
            "UserPromptSubmit",
            json!({}),
        ));
        assert_eq!(state.get(&session).unwrap().status(), "busy");

        state.sweep(SystemTime::now(), &|_| Some(false));
        assert_eq!(
            state.get(&session).unwrap().status(),
            "busy",
            "an unchanged foreground identity is not an edge"
        );

        state.sessions.get_mut(&session).unwrap().observed_runtime =
            Some("com.openai.codex:11:6".to_string());
        state.sweep(SystemTime::now(), &|_| Some(false));
        assert_eq!(
            state.get(&session).unwrap().status(),
            "idle",
            "a stale latch never speaks for the process that replaced it"
        );
        assert_eq!(
            state.get(&session).unwrap().activity_source,
            ActivitySource::None
        );
    }

    #[test]
    fn a_hook_capable_launch_keeps_its_latch_across_foreground_observations() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut record = manifest(session.clone(), None);
        record.launch.shell = "claude".to_string();
        write_manifest(home.path(), &record).unwrap();
        let mut state = WorkerState::new(home.path());
        state.rebuild_from_home(home.path());
        state.sessions.get_mut(&session).unwrap().observed_runtime =
            Some("com.anthropic.claude-code:10:5".to_string());
        state.apply_core_event(&frame(
            &session,
            "ai.prompt_submit",
            "UserPromptSubmit",
            json!({}),
        ));
        state.sessions.get_mut(&session).unwrap().observed_runtime =
            Some("com.anthropic.claude-code:11:6".to_string());
        state.sweep(SystemTime::now(), &|_| Some(false));
        assert_eq!(state.get(&session).unwrap().status(), "busy");
        assert_eq!(
            state.get(&session).unwrap().activity_source,
            ActivitySource::Hooks
        );
    }

    #[test]
    fn a_worker_started_long_after_a_seeded_turn_settles_it_during_rebuild() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let now = SystemTime::now();
        let old_seed = now - HOOK_IDLE_TIMEOUT - std::time::Duration::from_secs(60);
        let mut record = manifest(
            session.clone(),
            Some(hook("UserPromptSubmit", SessionGeneration::FIRST)),
        );
        record.generation_started_at_ms = Some(crate::hook_state::unix_ms(
            old_seed - std::time::Duration::from_secs(60),
        ));
        write_manifest(home.path(), &record).unwrap();
        let dir = paneflow_home::host_session_data_dir_in(home.path(), session.as_str());
        seed(&dir, "UserPromptSubmit", 1);
        set_seed_modified_at(&dir, old_seed);

        let mut state = WorkerState::new(home.path());
        state.rebuild_from_home(home.path());
        assert_eq!(state.get(&session).unwrap().status(), "idle");
        assert_eq!(
            state.get(&session).unwrap().outcome.as_deref(),
            Some("expired")
        );
    }

    #[test]
    fn recovered_opening_lease_anchors_to_the_last_output_time() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let now = SystemTime::now();
        let seed_at = now - HOOK_IDLE_TIMEOUT - std::time::Duration::from_secs(60);
        let output_at = now - std::time::Duration::from_secs(10);
        let mut record = manifest(
            session.clone(),
            Some(hook("UserPromptSubmit", SessionGeneration::FIRST)),
        );
        record.generation_started_at_ms = Some(crate::hook_state::unix_ms(
            seed_at - std::time::Duration::from_secs(60),
        ));
        let dir = paneflow_home::host_session_data_dir_in(home.path(), session.as_str());
        seed(&dir, "UserPromptSubmit", 1);
        set_seed_modified_at(&dir, seed_at);

        let mut state = WorkerState::new(home.path());
        let mut entry = SessionEntry::from_manifest(record);
        entry.output_changed_at_ms = Some(crate::hook_state::unix_ms(output_at));
        state.sessions.insert(session.clone(), entry);
        state.derive(&session, now, &|_| Some(false));

        assert_eq!(state.get(&session).unwrap().status(), "busy");
        assert_eq!(state.get(&session).unwrap().outcome, None);
    }

    #[test]
    fn an_unverifiable_pid_stays_non_resumable_and_is_never_signaled() {
        let mut entry = SessionEntry::from_manifest(manifest(SessionId::new(), None));
        assert_eq!(entry.health, Health::NonResumable);
        assert!(!entry.health.may_be_signaled());

        entry.process = Some(ProcessIdentity {
            pid: std::process::id(),
            started_at: None,
        });
        entry.refresh_health();
        assert_eq!(entry.health, Health::NonResumable);

        entry.process = Some(ProcessIdentity::capture(std::process::id()));
        entry.refresh_health();
        assert_eq!(entry.health, Health::Live);
        assert!(entry.health.may_be_signaled());

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
        record.process = Some(ProcessIdentity {
            pid: u32::MAX - 1,
            started_at: Some(7),
        });
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
        let dir = paneflow_home::host_session_data_dir_in(home.path(), session.as_str());
        seed(&dir, "UserPromptSubmit", 1);
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
        assert_eq!(entry.status(), "busy");
        assert_eq!(entry.activity_source, ActivitySource::Hooks);
    }

    #[test]
    fn a_recognized_runtime_without_a_latch_takes_the_screen_verdict() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let raw = json!({
            "session": session,
            "generation": SessionGeneration::FIRST,
            "live": true,
            "lifecycle": SessionLifecycle::Running,
            "last_hook": {
                "hook_event_name": "HookSeen",
                "tool": "claude",
                "runtime_generation": SessionGeneration::FIRST,
                "received_at_ms": 1,
            },
            "screen_activity": SCREEN_WORKING,
        });
        let mut state = WorkerState::new(home.path());
        state.apply_core_snapshot(&[raw]);
        let entry = state.get(&session).expect("the screen tier projects");
        assert_eq!(entry.status(), "busy");
        assert_eq!(entry.activity_source, ActivitySource::Screen);
        assert_eq!(entry.outcome, None);
    }

    #[test]
    fn a_menu_the_host_detected_overrides_busy_and_idle_with_attention() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = running_state(home.path(), &session);
        state.apply_core_event(&frame(
            &session,
            "ai.prompt_submit",
            "UserPromptSubmit",
            json!({}),
        ));
        state.sessions.get_mut(&session).unwrap().menu_prompt_active = true;
        let flipped = state.sweep(SystemTime::now(), &|_| Some(true));
        assert_eq!(flipped[0].session["status"], "attention");
        assert_eq!(
            flipped[0]
                .notification
                .as_ref()
                .map(|notification| notification.kind),
            Some(KIND_NEEDS_INPUT)
        );

        state.set_menu_attention_detection(false);
        let ignored = state.sweep(SystemTime::now(), &|_| Some(true));
        assert_eq!(ignored[0].session["status"], "busy");
    }

    #[test]
    fn a_menu_edge_without_a_hook_latch_still_notifies_for_input() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = running_state(home.path(), &session);
        state.sessions.get_mut(&session).unwrap().menu_prompt_active = true;

        let asking = state.sweep(SystemTime::now(), &|_| Some(true));
        assert_eq!(asking[0].session["status"], "attention");
        assert_eq!(asking[0].session["activity_source"], "none");
        assert_eq!(
            asking[0]
                .notification
                .as_ref()
                .map(|notification| notification.kind),
            Some(KIND_NEEDS_INPUT)
        );
    }
}
