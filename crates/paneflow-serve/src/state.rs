use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use paneflow_agent_config::runtime_catalog::{Runtime, RuntimeLifecycleSource};
use paneflow_config::schema::{SessionGeneration, SessionId, WorkspaceId};
use paneflow_host::agent::AgentEvent;
use paneflow_host::manifest::{SessionLifecycle, SessionManifest};
use paneflow_host::runtime_observer::RuntimeObservation;
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
    pub hook_revision: u64,
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
    pub observed_runtime: Option<RuntimeObservation>,
    pub unread: bool,
    pub updated_at_ms: u64,
}

impl SessionEntry {
    fn from_manifest(manifest: SessionManifest) -> Self {
        let launch_hook_capable = command_is_hook_capable(&manifest.launch.shell);
        let mut entry = Self {
            hook_revision: 0,
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
            observed_runtime: manifest
                .runtime
                .and_then(|runtime| runtime.current_observation),
            unread: false,
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
            .or_else(|| {
                self.observed_runtime
                    .as_ref()
                    .and_then(RuntimeObservation::runtime)
            })
    }

    fn foreground_identity(&self) -> Option<String> {
        self.observed_runtime
            .as_ref()
            .map(RuntimeObservation::identity)
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
            "hook_revision": self.hook_revision,
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

    pub fn menu_attention_detection(&self) -> bool {
        self.menu_attention_detection
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
        let live: BTreeSet<SessionId> = self.sessions.keys().cloned().collect();
        self.engine.retain_sessions(&live);
        for frame in accepted_events {
            self.apply_core_event(&frame);
        }
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
        let mut recovered = BTreeMap::new();
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
            let mut entry = merge_core_row(session.clone(), raw, held);
            entry.refresh_health();
            self.sessions.insert(entry.session.clone(), entry);
            for field in ["activity_event", "event"] {
                if let Some(frame) = raw["last_hook"][field].as_object()
                    && let Some(projection) = self.apply_core_event(&Value::Object(frame.clone()))
                {
                    recovered.insert(session.clone(), projection);
                }
            }
        }
        self.sessions.retain(|session, _| seen.contains(session));
        self.engine.retain_sessions(&seen);
        let now = SystemTime::now();
        seen.into_iter()
            .filter_map(|session| {
                let mut projection = self.derive(&session, now, &|_| None)?;
                let recovered_event = recovered.contains_key(&session);
                if let Some(recovery) = recovered.remove(&session) {
                    projection.changed |= recovery.changed;
                    projection.notification = projection.notification.or(recovery.notification);
                }
                let announce = (projection.changed
                    && (recovered_event || !discovered.contains(&session)))
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

    pub fn apply_cancellation(&mut self, frame: &Value) -> Option<Projection> {
        let session = frame["session"]
            .as_str()
            .and_then(|raw| SessionId::parse(raw).ok())?;
        let generation = self.sessions.get(&session)?.generation.get();
        let marker = crate::hook_assets::Cancellation {
            runtime_generation: frame["runtime_generation"].as_u64()?,
            cancelled_at: frame["cancelled_at"].as_u64()?,
            submitted_at: frame["submitted_at"].as_u64(),
        };
        let dir = self.session_dir(&session);
        self.engine.bind_session_dir(&session, &dir);
        self.engine
            .observe_cancellation(&session, &marker, generation);
        self.derive(&session, SystemTime::now(), &|_| None)
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
        let output_changed_at_ms = entry.output_changed_at_ms;
        let anchor_start_to_output = entry
            .runtime()
            .is_none_or(|runtime| runtime.lifecycle.anchor_start_event_to_output);
        let foreground_identity = entry.foreground_identity();
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
                .observe_foreground_runtime(&session, foreground_identity.as_deref());
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
        let canonical = crate::hook_state::normalize_event_name(&raw_name);
        if revision > 0
            && matches!(
                canonical.as_str(),
                crate::hook_state::EVENT_USER_PROMPT_SUBMIT | crate::hook_state::EVENT_START
            )
        {
            let event_at = at_unix_ms(now_ms);
            let lease_at = if canonical == crate::hook_state::EVENT_USER_PROMPT_SUBMIT
                || anchor_start_to_output
            {
                output_changed_at_ms
                    .map(at_unix_ms)
                    .map_or(event_at, |output| output.max(event_at))
            } else {
                event_at
            };
            self.engine
                .restore_opening_lease(&session, &dir, generation, event_at, lease_at);
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
        let foreground_identity = entry.foreground_identity();
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
                .observe_foreground_runtime(session, foreground_identity.as_deref());
        }
        self.engine.bind_session_dir(session, &dir);
        if hook_capable && entry.hook_revision == 0 {
            self.engine.seed_from_disk(
                session,
                &dir,
                anchor_start_event_to_output,
                generation_started_at_ms,
                generation,
                entry.output_changed_at_ms,
            );
        } else if hook_capable {
            self.engine
                .sync_cancellation_from_disk(session, &dir, generation);
            self.engine.sync_background_from_disk(
                session,
                &dir,
                generation,
                generation_started_at_ms,
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
        let notification = notice.and_then(|notice| {
            notification_for(notice, source.is_hooks(), &runtime_label, body.as_deref())
        });
        if notification
            .as_ref()
            .is_some_and(|notification| notification.kind == crate::notifications::KIND_FINISHED)
        {
            entry.unread = true;
        }
        let value = entry.to_value();
        Some(Projection {
            session: value,
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
        hook_revision: held
            .as_ref()
            .filter(|entry| entry.generation == generation)
            .map_or(0, |entry| entry.hook_revision),
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
        observed_runtime: serde_json::from_value(raw["observed_runtime"].clone())
            .ok()
            .flatten()
            .or_else(|| {
                held.as_ref()
                    .and_then(|entry| entry.observed_runtime.clone())
            }),
        unread: same_generation
            .then(|| held.as_ref().map(|entry| entry.unread))
            .flatten()
            .unwrap_or(false),
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
            hook_revision: 0,
            generation_started_at_ms: Some(LAUNCHED_AT),
            screen_changed_at_ms: None,
            screen_activity: None,
            menu_prompt_active: false,
            runtime: None,
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

    #[test]
    fn revisioned_host_snapshots_recover_a_lost_notification_and_reject_queued_older_events() {
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
        let accepted = |kind: &str, name: &str, timestamp: u64| {
            let mut raw = frame(&session, kind, name, json!({}));
            raw["runtime_generation"] = json!(1);
            raw["emitted_at_ms"] = json!(timestamp);
            host.ingest_agent_event(&AgentEvent::from_params(&raw).unwrap())
                .unwrap()
        };
        let first = accepted("ai.prompt_submit", "UserPromptSubmit", 10);
        state
            .apply_core_event(first.frame.as_ref().unwrap())
            .unwrap();
        assert_eq!(state.get(&session).unwrap().status(), "busy");
        let blocked_seed = paneflow_home::host_session_data_dir_in(home.path(), session.as_str())
            .join(hook_assets::SEED_FILE);
        std::fs::remove_file(&blocked_seed).unwrap();
        std::fs::create_dir(&blocked_seed).unwrap();
        let second = accepted("ai.notification", "PermissionRequest", 11);
        assert_eq!(second.ack["durable"], false);
        assert!(second.ack["persistence_error"].as_str().is_some());
        let projections = state.apply_core_snapshot(&snapshot());
        assert_eq!(state.get(&session).unwrap().status(), "attention");
        assert_eq!(state.get(&session).unwrap().hook_revision, 2);
        assert!(!projections.is_empty());
        assert!(
            state
                .apply_core_event(first.frame.as_ref().unwrap())
                .is_none()
        );
        assert!(
            state
                .apply_core_event(second.frame.as_ref().unwrap())
                .is_none()
        );
        assert_eq!(state.get(&session).unwrap().status(), "attention");
        assert!(state.apply_core_snapshot(&snapshot()).is_empty());
        let mut replacement = WorkerState::new(home.path());
        replacement.rebuild_from_home(home.path());
        assert_eq!(replacement.get(&session).unwrap().hook_revision, 2);
        assert_eq!(replacement.get(&session).unwrap().status(), "attention");
        let mut stale = AgentEvent::from_params(first.frame.as_ref().unwrap()).unwrap();
        stale.kind = paneflow_host::agent::AgentEventKind::SessionEnd;
        assert_eq!(
            host.ingest_agent_event(&stale).unwrap().ack["accepted"],
            false
        );
        assert_eq!(host.agent_snapshot()[0].hook_revision, 2);
        std::fs::remove_dir(&blocked_seed).unwrap();
        let retry = accepted("ai.notification", "PermissionRequest", 11);
        assert_eq!(retry.ack["durable"], true);
        assert_eq!(retry.ack["duplicate"], true);
        assert!(retry.frame.is_none());
        accepted("ai.tool_use", "PreToolUse", 12);
        let mut replacement = WorkerState::new(home.path());
        replacement.rebuild_from_home(home.path());
        assert_eq!(replacement.get(&session).unwrap().hook_revision, 3);
        assert_eq!(replacement.get(&session).unwrap().status(), "attention");
        let mut background = frame(
            &session,
            "ai.stop",
            "Stop",
            json!({"background_tasks": true}),
        );
        background["runtime_generation"] = json!(1);
        background["emitted_at_ms"] = json!(13);
        let pending = host
            .ingest_agent_event(&AgentEvent::from_params(&background).unwrap())
            .unwrap();
        replacement
            .apply_core_event(pending.frame.as_ref().unwrap())
            .unwrap();
        assert_eq!(replacement.get(&session).unwrap().status(), "busy");
        let mut restored = WorkerState::new(home.path());
        restored.rebuild_from_home(home.path());
        assert_eq!(restored.get(&session).unwrap().status(), "busy");
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

    fn running_state(home: &Path, session: &SessionId) -> WorkerState {
        write_manifest(home, &manifest(session.clone(), None)).unwrap();
        let mut state = WorkerState::new(home);
        state.rebuild_from_home(home);
        state
    }

    fn cancellation_frame(
        session: &SessionId,
        cancelled_at: u64,
        submitted_at: Option<u64>,
    ) -> Value {
        json!({
            "type": "cancellation",
            "session": session.to_string(),
            "generation": 1,
            "runtime_generation": 1,
            "cancelled_at": cancelled_at,
            "submitted_at": submitted_at,
        })
    }

    #[test]
    fn an_escape_fence_settles_a_busy_turn_without_completing_it_and_the_next_prompt_rearms() {
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

        let cancelled_at = crate::hook_state::unix_ms(SystemTime::now()) - 2_000;
        let settled = state
            .apply_cancellation(&cancellation_frame(&session, cancelled_at, None))
            .expect("the fence projects");
        assert_eq!(settled.session["status"], "idle");
        assert_eq!(settled.session["outcome"], "cancelled");
        assert!(
            settled.notification.is_none(),
            "a cancelled turn never notifies a completion"
        );

        assert!(
            state
                .apply_core_event(&frame(&session, "ai.stop", "Stop", json!({})))
                .is_none(),
            "a Stop arriving after the fence never completes the turn"
        );
        assert_eq!(
            state.sessions.get(&session).unwrap().to_value()["status"],
            "idle"
        );
        assert_eq!(
            state.sessions.get(&session).unwrap().to_value()["outcome"],
            "cancelled"
        );

        state.apply_cancellation(&cancellation_frame(
            &session,
            cancelled_at,
            Some(cancelled_at + 1_000),
        ));
        let rearmed = state
            .apply_core_event(&frame(
                &session,
                "ai.prompt_submit",
                "UserPromptSubmit",
                json!({}),
            ))
            .expect("the next prompt projects");
        assert_eq!(rearmed.session["status"], "busy");
    }

    #[test]
    fn a_cancellation_for_an_unknown_session_or_generation_changes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = running_state(home.path(), &session);
        state.apply_core_event(&frame(
            &session,
            "ai.prompt_submit",
            "UserPromptSubmit",
            json!({}),
        ));

        assert!(
            state
                .apply_cancellation(&cancellation_frame(&SessionId::new(), 10, None))
                .is_none()
        );
        let mut stale = cancellation_frame(&session, 10, None);
        stale["runtime_generation"] = json!(9);
        state.apply_cancellation(&stale);
        assert_eq!(
            state.sessions.get(&session).unwrap().to_value()["status"],
            "busy"
        );
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
            Some(observation("com.anthropic.claude-code", 10, 5));
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
            Some(observation("com.openai.codex", 11, 6));
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
            Some(observation("com.anthropic.claude-code", 10, 5));
        state.apply_core_event(&frame(
            &session,
            "ai.prompt_submit",
            "UserPromptSubmit",
            json!({}),
        ));
        state.sessions.get_mut(&session).unwrap().observed_runtime =
            Some(observation("com.anthropic.claude-code", 11, 6));
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
    fn an_observed_runtime_without_any_hook_takes_the_screen_verdict() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let raw = json!({
            "session": session,
            "generation": SessionGeneration::FIRST,
            "live": true,
            "lifecycle": SessionLifecycle::Running,
            "screen_activity": SCREEN_WORKING,
            "observed_runtime": observation("com.anthropic.claude-code", 10, 5),
        });
        let mut state = WorkerState::new(home.path());
        state.apply_core_snapshot(&[raw]);
        let entry = state.get(&session).expect("the screen tier projects");
        assert_eq!(
            entry.runtime().map(|runtime| runtime.slug),
            Some("claude-code")
        );
        assert_eq!(entry.status(), "busy");
        assert_eq!(entry.activity_source, ActivitySource::Screen);
        assert_eq!(entry.outcome, None);
    }

    #[test]
    fn a_hook_latch_ignores_the_screen_verdict_that_contradicts_it() {
        let home = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut state = running_state(home.path(), &session);
        state.sessions.get_mut(&session).unwrap().screen_activity = Some(SCREEN_IDLE.to_string());
        state.apply_core_event(&frame(
            &session,
            "ai.prompt_submit",
            "UserPromptSubmit",
            json!({}),
        ));
        let entry = state.get(&session).expect("the latched session projects");
        assert_eq!(entry.status(), "busy");
        assert_eq!(entry.activity_source, ActivitySource::Hooks);
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
