use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use paneflow_api::methods::events::{EventIds, PublishedEvent};
use paneflow_api::methods::layout::{
    Adopted, AppliedOp, Attached, ERR_LAYOUT_REFUSED, HOST_UNREACHABLE, LAUNCH_FAILED,
    LaunchParams, Launched, LayoutSnapshot, Lease, NOT_ATTACHED, SweepParams, Swept, View,
    WINDOW_OWNS_LAYOUT,
};
use paneflow_config::schema::{SessionId, SessionState};
use paneflow_layout::{
    Effect, Event, EventScope, IdBlock, IdSource, LEASE_BLOCK, Model, Op, OpError, Snapshot,
    SurfaceId, SurfaceKind,
};
use serde_json::{Map, Value, json};

use crate::launcher::{LaunchError, LaunchInputs, LaunchSupport, SessionLink, SessionStarter};
use crate::registry::ServeError;

pub const SAVE_DEBOUNCE: Duration = Duration::from_millis(150);
pub const MAX_LAYOUT_FRAME_BYTES: usize = 16 * 1024 * 1024;
use crate::sweep::{ListedRecord, layout_sessions, orphan_sweep};

const WINDOW_PROBE: Duration = Duration::from_millis(200);
const LAUNCH_BIND_GRACE: Duration = Duration::from_secs(120);
const IDLE_WAKE: Duration = Duration::from_secs(5);
const HOST_CLIENT: &str = "paneflow-serve-layout";
const UNREADABLE_SUFFIX: &str = "unreadable";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConnectionId(pub u64);

impl ConnectionId {
    pub const INTERNAL: Self = Self(0);
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Loaded {
    Missing,
    Read,
    Unreadable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HostWork {
    Stop(SessionId),
    Rehome {
        session: SessionId,
        workspace: paneflow_config::schema::WorkspaceId,
    },
}

struct Core {
    model: Model,
    loaded: Loaded,
    aside_pending: bool,
    dirty_since: Option<Instant>,
    written_revision: Option<u64>,
    clients: BTreeMap<ConnectionId, String>,
    attached: BTreeMap<ConnectionId, String>,
    leases: HashMap<String, Vec<IdBlock>>,
    reload_before_next: bool,
    live_sweep_due: bool,
    file_stamp: Option<FileStamp>,
    writing: bool,
    write_failure: Option<String>,
    logged_causes: HashSet<String>,
    host_work: VecDeque<HostWork>,
    launching: HashMap<SessionId, Instant>,
    writes: u64,
    closed: bool,
}

pub struct LayoutAuthority {
    home: PathBuf,
    path: PathBuf,
    core_endpoint: PathBuf,
    epoch: String,
    core: Mutex<Core>,
    wake: Condvar,
    support: LaunchSupport,
    starter: Arc<dyn SessionStarter>,
}

pub struct ApplyOutcome {
    pub applied: AppliedOp,
    pub events: Vec<PublishedEvent>,
    pub launch_error: Option<ServeError>,
}

#[derive(Clone, Copy)]
struct Attempt<'a> {
    connection: ConnectionId,
    kind: &'a str,
    op: &'a Op,
    ids_from: Option<u64>,
    now_ms: u64,
    launches_here: bool,
}

struct AheadSession {
    surface: SurfaceId,
    session: SessionId,
    binds: bool,
}

#[derive(Default)]
struct StartedAhead {
    started: Vec<AheadSession>,
    failed: Vec<SurfaceId>,
    error: Option<ServeError>,
}

impl StartedAhead {
    fn take(&mut self, surface: SurfaceId) -> Option<AheadSession> {
        let index = self
            .started
            .iter()
            .position(|started| started.surface == surface)?;
        Some(self.started.swap_remove(index))
    }
}

struct Committed {
    events: Vec<PublishedEvent>,
    revision: u64,
    late: Vec<SurfaceId>,
    state: Option<paneflow_layout::State>,
    launch_error: Option<ServeError>,
}

fn refused(reason: &str, message: impl Into<String>) -> ServeError {
    ServeError {
        code: ERR_LAYOUT_REFUSED,
        message: message.into(),
        data: Some(json!({"reason": reason})),
    }
}

fn op_refused(kind: &str, error: OpError, cause: Option<String>) -> ServeError {
    let message = match &cause {
        Some(cause) => format!("the layout op {kind} was refused: {error} ({cause})"),
        None => format!("the layout op {kind} was refused: {error}"),
    };
    log::warn!("paneflow-serve: {message}");
    ServeError {
        code: ERR_LAYOUT_REFUSED,
        message,
        data: Some(json!({"reason": error.reason(), "op": kind, "cause": cause})),
    }
}

fn op_kind(op: &Value) -> String {
    op.get("op")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string()
}

fn encoded_kind(op: &Op) -> String {
    serde_json::to_value(op).map_or_else(|_| "unknown".to_string(), |value| op_kind(&value))
}

fn refusal_cause(state: &paneflow_layout::State, op: &Op, error: OpError) -> Option<String> {
    match (op, error) {
        (Op::BindSession { surface, session }, OpError::Invalid) => {
            let bound = state.surface(*surface)?.session()?;
            Some(format!(
                "surface {} already shows session {bound}, not {session}",
                surface.get()
            ))
        }
        _ => None,
    }
}

fn invalid_params(message: impl Into<String>) -> ServeError {
    ServeError::invalid_params(message)
}

fn launch_refused(error: &LaunchError) -> ServeError {
    match error {
        LaunchError::Unreachable(message) => refused(
            HOST_UNREACHABLE,
            format!("the terminal host is unreachable: {message}"),
        ),
        LaunchError::Refused(message) => refused(
            LAUNCH_FAILED,
            format!("the terminal host refused the session: {message}"),
        ),
    }
}

pub(crate) fn orchestration_refused(method: &str) -> ServeError {
    ServeError {
        code: crate::protocol::ERR_METHOD_NOT_FOUND,
        message: format!(
            "{method} orchestration disabled; set PANEFLOW_IPC_ORCHESTRATION=1 \
             or PANEFLOW_IPC_SCRIPTING=1 to enable command, prompt, or env"
        ),
        data: None,
    }
}

fn decode_param<T: serde::de::DeserializeOwned>(
    value: Option<Value>,
    what: &str,
) -> Result<Option<T>, ServeError> {
    value
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| invalid_params(format!("{what} is malformed: {error}")))
}

fn spawned_surfaces(effects: &[Effect]) -> Vec<SurfaceId> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Spawn { surface } => Some(*surface),
            _ => None,
        })
        .collect()
}

pub fn published_events(
    events: &[Event],
    scopes: &[EventScope],
    revision: u64,
) -> Vec<PublishedEvent> {
    events
        .iter()
        .zip(scopes)
        .map(|(event, scope)| {
            let mut data = Map::new();
            if matches!(event, Event::LayoutChanged { .. }) {
                data.insert("revision".to_string(), json!(revision));
            }
            PublishedEvent {
                kind: event.kind().to_string(),
                ids: EventIds {
                    workspace_id: scope.workspace.map(|id| id.get()),
                    tab_id: scope.tab.map(|id| id.get()),
                    pane_id: scope.pane.map(|id| id.get()),
                    surface_id: scope.surface.map(|id| id.get()),
                    session: scope.session.as_ref().map(ToString::to_string),
                },
                data,
            }
        })
        .collect()
}

pub fn snapshot_value(snapshot: &Snapshot) -> LayoutSnapshot {
    match serde_json::to_value(snapshot) {
        Ok(Value::Object(map)) => LayoutSnapshot(map),
        _ => LayoutSnapshot::default(),
    }
}

pub fn snapshot_from(value: LayoutSnapshot) -> Result<Snapshot, ServeError> {
    serde_json::from_value(Value::Object(value.0))
        .map_err(|error| invalid_params(format!("the layout view is not a layout model: {error}")))
}

pub fn read_session_file(path: &Path) -> Result<Option<SessionState>, String> {
    let pending = paneflow_config::session_file::pending_session_path(path);
    if let Ok(Some(state)) = read_one(&pending) {
        return Ok(Some(state));
    }
    read_one(path)
}

fn read_one(path: &Path) -> Result<Option<SessionState>, String> {
    use std::io::Read;
    let file = match paneflow_home::open_for_reading(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("io:{:?}", error.kind())),
    };
    let metadata = file
        .metadata()
        .map_err(|error| format!("io:{:?}", error.kind()))?;
    if !metadata.is_file() {
        return Err("non_regular".to_string());
    }
    let cap = paneflow_config::session_file::MAX_SESSION_FILE_BYTES;
    if metadata.len() > cap {
        return Err("oversize".to_string());
    }
    let mut bytes = Vec::new();
    file.take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("io:{:?}", error.kind()))?;
    let mut state: SessionState =
        serde_json::from_slice(&bytes).map_err(|error| format!("parse:{error}"))?;
    match state.version {
        paneflow_config::schema::SESSION_SCHEMA_VERSION => {
            paneflow_config::schema::repair_durable_identities(&mut state);
        }
        paneflow_config::schema::SESSION_SCHEMA_VERSION_V2 => {
            paneflow_config::schema::migrate_session_v2(&mut state);
        }
        paneflow_config::schema::SESSION_SCHEMA_VERSION_V1 => {
            paneflow_config::schema::migrate_session_v1(&mut state);
        }
        other => return Err(format!("unsupported_version:{other}")),
    }
    Ok(Some(state))
}

type FileStamp = (u64, std::time::SystemTime);

fn file_stamp(path: &Path) -> Option<FileStamp> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.len(), metadata.modified().ok()?))
}

fn model_from_file(path: &Path) -> (Model, Loaded) {
    match read_session_file(path) {
        Ok(Some(session)) => {
            let loaded = paneflow_layout::load(&session);
            let model = loaded.model;
            let floor = model.next_id();
            model
                .ids()
                .claim(floor.saturating_add(LEASE_BLOCK).saturating_sub(1));
            (model, Loaded::Read)
        }
        Ok(None) => (Model::new(), Loaded::Missing),
        Err(cause) => {
            log::warn!(
                "paneflow-serve: {} is unreadable ({cause}); the layout starts empty and the file is kept aside before the first write",
                path.display()
            );
            (Model::new(), Loaded::Unreadable(cause))
        }
    }
}

fn session_sessions(model: &Model) -> HashSet<SessionId> {
    let state = model.state();
    let mut sessions: HashSet<SessionId> = state
        .workspaces
        .iter()
        .flat_map(|ws| ws.tabs.iter())
        .filter_map(|tab| tab.root.as_ref())
        .flat_map(paneflow_layout::Node::panes)
        .flat_map(|pane| pane.surfaces.iter())
        .filter_map(|surface| surface.session().cloned())
        .collect();
    sessions.extend(model.held_sessions());
    sessions
}

pub fn export_for_file(model: &Model) -> SessionState {
    let mut session = paneflow_layout::export(model, &[]);
    for ws in &mut session.workspaces {
        for tab in &mut ws.tabs {
            tab.dock = None;
        }
    }
    session
}

impl LayoutAuthority {
    pub fn open(home: &Path, core_endpoint: PathBuf, epoch: String) -> Self {
        let starter = crate::launcher::HostStarter::shared(core_endpoint.clone());
        Self::open_with(
            home,
            core_endpoint,
            epoch,
            LaunchSupport::default(),
            starter,
        )
    }

    pub fn open_with(
        home: &Path,
        core_endpoint: PathBuf,
        epoch: String,
        support: LaunchSupport,
        starter: Arc<dyn SessionStarter>,
    ) -> Self {
        let path = paneflow_home::session_path_in(home);
        let (model, loaded) = model_from_file(&path);
        let aside_pending = matches!(loaded, Loaded::Unreadable(_));
        let live_sweep_due = !aside_pending;
        let stamp = file_stamp(&path);
        Self {
            home: home.to_path_buf(),
            path,
            core_endpoint,
            epoch,
            core: Mutex::new(Core {
                model,
                loaded,
                aside_pending,
                dirty_since: None,
                written_revision: None,
                clients: BTreeMap::new(),
                attached: BTreeMap::new(),
                leases: HashMap::new(),
                reload_before_next: false,
                live_sweep_due,
                file_stamp: stamp,
                writing: false,
                write_failure: None,
                logged_causes: HashSet::new(),
                host_work: VecDeque::new(),
                launching: HashMap::new(),
                writes: 0,
                closed: false,
            }),
            wake: Condvar::new(),
            support,
            starter,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Core> {
        self.core.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    pub fn revision(&self) -> u64 {
        self.lock().model.revision()
    }

    pub fn snapshot(&self) -> Snapshot {
        self.lock().model.snapshot()
    }

    pub fn exported(&self) -> SessionState {
        export_for_file(&self.lock().model)
    }

    pub fn has_attached_window(&self) -> bool {
        !self.lock().attached.is_empty()
    }

    pub fn view(&self, unless_revision: Option<u64>) -> View {
        let core = self.lock();
        let revision = core.model.revision();
        View {
            epoch: self.epoch.clone(),
            revision,
            attached: !core.attached.is_empty(),
            view: (unless_revision != Some(revision))
                .then(|| snapshot_value(&core.model.snapshot())),
        }
    }

    pub fn attach(&self, connection: ConnectionId, client: Option<String>) -> Attached {
        let mut core = self.lock();
        self.reload_if_rewritten(&mut core);
        core.reload_before_next = false;
        let name = client
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| format!("connection-{}", connection.0));
        core.clients.insert(connection, name.clone());
        core.attached.insert(connection, name);
        Attached {
            epoch: self.epoch.clone(),
            revision: core.model.revision(),
            view: snapshot_value(&core.model.snapshot()),
        }
    }

    pub fn disconnect(&self, connection: ConnectionId) {
        let mut core = self.lock();
        core.attached.remove(&connection);
        if let Some(name) = core.clients.remove(&connection)
            && !core.clients.values().any(|held| *held == name)
            && name.starts_with("connection-")
        {
            core.leases.remove(&name);
        }
    }

    fn client_key(core: &mut Core, connection: ConnectionId) -> String {
        core.clients
            .entry(connection)
            .or_insert_with(|| format!("connection-{}", connection.0))
            .clone()
    }

    pub fn lease(&self, connection: ConnectionId) -> Result<Lease, ServeError> {
        if connection == ConnectionId::INTERNAL {
            return Err(invalid_params(
                "layout.lease_ids needs a connection; the worker allocates its own ids",
            ));
        }
        if !self.has_attached_window() && self.window_listening() {
            self.lock().reload_before_next = true;
            return Err(refused(
                WINDOW_OWNS_LAYOUT,
                "a Paneflow window of an earlier build owns the layout; ids are leased once that window restarts",
            ));
        }
        let mut core = self.lock();
        self.reload_if_rewritten_unattached(&mut core);
        let ids = core.model.ids();
        let start = ids.next_value();
        let end = start.saturating_add(LEASE_BLOCK);
        ids.claim(end.saturating_sub(1));
        let key = Self::client_key(&mut core, connection);
        core.leases
            .entry(key)
            .or_default()
            .push(IdBlock { start, end });
        Self::mark_dirty(&mut core);
        drop(core);
        self.flush();
        Ok(Lease { start, end })
    }

    fn mark_dirty(core: &mut Core) {
        if core.dirty_since.is_none() {
            core.dirty_since = Some(Instant::now());
        }
    }

    fn window_listening(&self) -> bool {
        let endpoint = paneflow_home::ipc_endpoint_path(&self.home);
        paneflow_ipc_client::line_wire::Wire::connect_with_timeout(&endpoint, 1024, WINDOW_PROBE)
            .is_ok()
    }

    fn reload_if_needed(&self, core: &mut Core) {
        if !core.reload_before_next {
            return;
        }
        core.reload_before_next = false;
        let (mut model, loaded) = model_from_file(&self.path);
        let floor = core.model.next_id();
        model.ids().claim(floor.saturating_sub(1));
        let revision = core.model.revision().max(model.revision()) + 1;
        model.set_revision(revision);
        core.aside_pending = matches!(loaded, Loaded::Unreadable(_));
        core.live_sweep_due = !core.aside_pending;
        core.loaded = loaded;
        core.model = model;
        core.file_stamp = file_stamp(&self.path);
        log::info!("paneflow-serve: reloaded the layout an earlier window wrote");
    }

    fn reload_if_rewritten_unattached(&self, core: &mut Core) {
        if core.attached.is_empty() && !core.writing && file_stamp(&self.path) != core.file_stamp {
            core.reload_before_next = true;
        }
        self.reload_if_needed(core);
    }

    fn reload_if_rewritten(&self, core: &mut Core) {
        if core.writing {
            return;
        }
        let stamp = file_stamp(&self.path);
        if stamp == core.file_stamp {
            return;
        }
        core.file_stamp = stamp;
        let (model, loaded) = model_from_file(&self.path);
        let floor = core.model.next_id();
        model.ids().claim(floor.saturating_sub(1));
        core.aside_pending = matches!(loaded, Loaded::Unreadable(_));
        core.live_sweep_due = !core.aside_pending;
        core.loaded = loaded;
        core.model = model;
        core.dirty_since = None;
        log::info!(
            "paneflow-serve: {} changed outside the worker; the layout was read from it again",
            self.path.display()
        );
    }

    pub fn current_state(&self) -> paneflow_layout::State {
        let mut core = self.lock();
        self.reload_if_rewritten_unattached(&mut core);
        core.model.state().clone()
    }

    pub fn reserve_id(&self) -> u64 {
        self.lock().model.reserve()
    }

    fn is_attached(&self, connection: ConnectionId) -> bool {
        self.lock().attached.contains_key(&connection)
    }

    pub fn apply(
        &self,
        connection: ConnectionId,
        op: Value,
        ids_from: Option<u64>,
        now_ms: u64,
        may_spawn: bool,
    ) -> Result<ApplyOutcome, ServeError> {
        let kind = op_kind(&op);
        let op: Op = serde_json::from_value(op)
            .map_err(|error| invalid_params(format!("the op is not a layout op: {error}")))?;
        let attached = self.has_attached_window();
        if !attached && connection != ConnectionId::INTERNAL && self.window_listening() {
            self.lock().reload_before_next = true;
            return Err(refused(
                WINDOW_OWNS_LAYOUT,
                "a Paneflow window of an earlier build owns the layout; it is applied there until that window restarts",
            ));
        }
        let launches_here = connection == ConnectionId::INTERNAL || !self.is_attached(connection);
        let mut link: Option<Box<dyn SessionLink>> = None;
        let mut ahead: Option<StartedAhead> = None;
        let committed: Result<Committed, ServeError> = loop {
            let mut core = self.lock();
            let tried = self.try_op(
                &mut core,
                Attempt {
                    connection,
                    kind: &kind,
                    op: &op,
                    ids_from,
                    now_ms,
                    launches_here,
                },
            );
            let (trial, applied, spawns) = match tried {
                Ok(tried) => tried,
                Err(error) => break Err(error),
            };
            if !spawns.is_empty() {
                let gated = !may_spawn
                    && spawns.iter().any(|surface| {
                        trial.state().surface(*surface).is_some_and(|known| {
                            crate::launcher::surface_needs_orchestration(&known.definition)
                        })
                    });
                if gated {
                    break Err(orchestration_refused("layout.apply"));
                }
                let Some(open) = link.as_mut() else {
                    drop(core);
                    match self.starter.connect() {
                        Ok(connected) => link = Some(connected),
                        Err(error) => break Err(launch_refused(&error)),
                    }
                    continue;
                };
                if ahead.is_none() {
                    let state = trial.state().clone();
                    drop(core);
                    ahead = Some(self.start_ahead(open.as_mut(), &state, &spawns));
                    continue;
                }
            }
            core.model = trial;
            for effect in &applied.effects {
                Self::queue_effect(&mut core, effect);
            }
            let mut events = published_events(&applied.events, &applied.scopes, applied.revision);
            let mut revision = applied.revision;
            let mut late = Vec::new();
            let mut launch_error = None;
            if let Some(ahead) = ahead.as_mut() {
                launch_error = ahead.error.take();
                for surface in &spawns {
                    match ahead.take(*surface) {
                        Some(started) => {
                            core.launching.remove(&started.session);
                            if !started.binds {
                                continue;
                            }
                            let bind = Op::BindSession {
                                surface: *surface,
                                session: started.session.clone(),
                            };
                            match core.model.apply(bind.clone(), now_ms) {
                                Ok(bound) => {
                                    for effect in &bound.effects {
                                        Self::queue_effect(&mut core, effect);
                                    }
                                    revision = bound.revision;
                                    events.extend(published_events(
                                        &bound.events,
                                        &bound.scopes,
                                        bound.revision,
                                    ));
                                }
                                Err(error) => {
                                    let cause = refusal_cause(core.model.state(), &bind, error);
                                    core.host_work.push_back(HostWork::Stop(started.session));
                                    launch_error = Some(op_refused("bind_session", error, cause));
                                }
                            }
                        }
                        None if ahead.failed.contains(surface) => {}
                        None => late.push(*surface),
                    }
                }
                for unused in ahead.started.drain(..) {
                    core.launching.remove(&unused.session);
                    if unused.binds {
                        core.host_work.push_back(HostWork::Stop(unused.session));
                    }
                }
            }
            Self::mark_dirty(&mut core);
            let state = (!late.is_empty()).then(|| core.model.state().clone());
            break Ok(Committed {
                events,
                revision,
                late,
                state,
                launch_error,
            });
        };
        let committed = match committed {
            Ok(committed) => committed,
            Err(error) => {
                if let Some(ahead) = ahead {
                    self.abandon(ahead);
                }
                return Err(error);
            }
        };
        self.wake.notify_all();
        let Committed {
            mut events,
            mut revision,
            late,
            state,
            mut launch_error,
        } = committed;
        if let (Some(state), Some(link)) = (state, link.as_mut()) {
            for surface in late {
                let Some(inputs) = crate::launcher::inputs_from_model(&state, surface) else {
                    continue;
                };
                let named = state
                    .surface(surface)
                    .and_then(|known| known.session().cloned());
                let binds = named.is_none();
                match self.start_session(
                    link.as_mut(),
                    &state,
                    surface,
                    named.unwrap_or_else(SessionId::new),
                    inputs,
                    binds,
                ) {
                    Ok(started) => {
                        revision = started.revision;
                        events.extend(started.events);
                    }
                    Err(error) => launch_error = Some(error),
                }
            }
        }
        Ok(ApplyOutcome {
            applied: AppliedOp {
                revision,
                events: events.clone(),
            },
            events,
            launch_error,
        })
    }

    fn try_op(
        &self,
        core: &mut Core,
        attempt: Attempt<'_>,
    ) -> Result<(Model, paneflow_layout::Applied, Vec<SurfaceId>), ServeError> {
        let Attempt {
            connection,
            kind,
            op,
            ids_from,
            now_ms,
            launches_here,
        } = attempt;
        self.reload_if_rewritten_unattached(core);
        let key = Self::client_key(core, connection);
        let blocks = core.leases.get(&key).cloned().unwrap_or_default();
        let mut trial = core.model.clone();
        for id in op.new_ids() {
            let leased = blocks.iter().any(|block| block.contains(id));
            if connection != ConnectionId::INTERNAL && !leased {
                return Err(op_refused(
                    kind,
                    OpError::Invalid,
                    Some(format!("id {id} is outside the ids leased to {key}")),
                ));
            }
            if connection != ConnectionId::INTERNAL && !trial.reserve_exact(id) {
                return Err(op_refused(
                    kind,
                    OpError::Invalid,
                    Some(format!("id {id} is already in use")),
                ));
            }
        }
        let ids = match ids_from {
            Some(at) if connection != ConnectionId::INTERNAL => {
                let mirror = IdSource::leased(&blocks, at);
                if mirror.is_overdrawn() {
                    return Err(op_refused(
                        kind,
                        OpError::Invalid,
                        Some(format!("ids_from {at} is past the ids leased to {key}")),
                    ));
                }
                mirror
            }
            _ => trial.ids(),
        };
        let applied = trial
            .apply_with_ids(op.clone(), now_ms, &ids)
            .map_err(|error| {
                op_refused(kind, error, refusal_cause(core.model.state(), op, error))
            })?;
        if ids.is_overdrawn() {
            return Err(op_refused(
                kind,
                OpError::Invalid,
                Some(format!(
                    "the op needs more ids than the lease of {key} holds"
                )),
            ));
        }
        let spawns = if launches_here {
            spawned_surfaces(&applied.effects)
        } else {
            Vec::new()
        };
        Ok((trial, applied, spawns))
    }

    fn start_ahead(
        &self,
        link: &mut dyn SessionLink,
        state: &paneflow_layout::State,
        spawns: &[SurfaceId],
    ) -> StartedAhead {
        let mut ahead = StartedAhead::default();
        for surface in spawns {
            let Some(inputs) = crate::launcher::inputs_from_model(state, *surface) else {
                continue;
            };
            let named = state
                .surface(*surface)
                .and_then(|known| known.session().cloned());
            let binds = named.is_none();
            let session = named.unwrap_or_default();
            match self.create_session(link, state, *surface, session.clone(), inputs) {
                Ok(_) => ahead.started.push(AheadSession {
                    surface: *surface,
                    session,
                    binds,
                }),
                Err(error) => {
                    ahead.failed.push(*surface);
                    ahead.error = Some(error);
                }
            }
        }
        ahead
    }

    fn abandon(&self, ahead: StartedAhead) {
        let mut core = self.lock();
        for unused in ahead.started {
            core.launching.remove(&unused.session);
            if unused.binds {
                core.host_work.push_back(HostWork::Stop(unused.session));
            }
        }
        drop(core);
        self.wake.notify_all();
    }

    fn create_session(
        &self,
        link: &mut dyn SessionLink,
        state: &paneflow_layout::State,
        surface: SurfaceId,
        session: SessionId,
        inputs: LaunchInputs,
    ) -> Result<Vec<String>, ServeError> {
        let prepared = crate::launcher::prepare(
            &self.home,
            &self.support,
            state,
            surface,
            session.clone(),
            inputs,
        )
        .ok_or_else(|| {
            op_refused(
                "launch",
                OpError::NotFound,
                Some(format!("surface {} is not a terminal", surface.get())),
            )
        })?;
        self.lock()
            .launching
            .insert(session.clone(), Instant::now());
        let created = link
            .exists(&session)
            .and_then(|known| {
                if known {
                    Ok(())
                } else {
                    link.create(&prepared.request)
                }
            })
            .map_err(|error| launch_refused(&error));
        if let Err(error) = created {
            self.lock().launching.remove(&session);
            return Err(error);
        }
        Ok(prepared.notices)
    }

    fn start_session(
        &self,
        link: &mut dyn SessionLink,
        state: &paneflow_layout::State,
        surface: SurfaceId,
        session: SessionId,
        inputs: LaunchInputs,
        bind: bool,
    ) -> Result<Launched, ServeError> {
        let notices = self.create_session(link, state, surface, session.clone(), inputs)?;
        if !bind {
            return Ok(Launched {
                revision: self.lock().model.revision(),
                events: Vec::new(),
                notices,
            });
        }
        let bound = self.apply_internal(Op::BindSession {
            surface,
            session: session.clone(),
        });
        self.lock().launching.remove(&session);
        let (applied, events) = bound?;
        Ok(Launched {
            revision: applied.revision,
            events,
            notices,
        })
    }

    fn apply_internal(&self, op: Op) -> Result<(AppliedOp, Vec<PublishedEvent>), ServeError> {
        let mut core = self.lock();
        let kind = encoded_kind(&op);
        let applied = match core.model.apply(op.clone(), crate::state::now_ms()) {
            Ok(applied) => applied,
            Err(error) => {
                let cause = refusal_cause(core.model.state(), &op, error);
                return Err(op_refused(&kind, error, cause));
            }
        };
        for effect in &applied.effects {
            Self::queue_effect(&mut core, effect);
        }
        Self::mark_dirty(&mut core);
        drop(core);
        self.wake.notify_all();
        let events = published_events(&applied.events, &applied.scopes, applied.revision);
        Ok((
            AppliedOp {
                revision: applied.revision,
                events: events.clone(),
            },
            events,
        ))
    }

    pub fn launch(
        &self,
        connection: ConnectionId,
        params: LaunchParams,
        may_spawn: bool,
    ) -> Result<Launched, ServeError> {
        if connection == ConnectionId::INTERNAL || !self.is_attached(connection) {
            return Err(refused(
                NOT_ATTACHED,
                "layout.launch needs a connection attached with layout.attach",
            ));
        }
        let carries_env = params.env.as_ref().is_some_and(|env| !env.is_empty());
        if carries_env && !may_spawn {
            return Err(orchestration_refused("layout.launch"));
        }
        let session: SessionId = params
            .session
            .parse()
            .map_err(|_| invalid_params("session is not a session id"))?;
        let surface = SurfaceId(params.surface_id);
        let state = self.lock().model.state().clone();
        let startable = state.surface(surface).is_some_and(|known| {
            known.kind() == SurfaceKind::Terminal
                && known
                    .definition
                    .session
                    .as_ref()
                    .is_none_or(|bound| *bound == session)
        });
        if !startable {
            let cause = match state.surface(surface).and_then(|known| known.session()) {
                Some(bound) => format!(
                    "surface {} already shows session {bound}",
                    params.surface_id
                ),
                None => format!(
                    "surface {} is not a terminal of the layout",
                    params.surface_id
                ),
            };
            return Err(op_refused("launch", OpError::NotFound, Some(cause)));
        }
        let inputs = LaunchInputs {
            cwd: params.cwd.map(PathBuf::from),
            confine_to: params.confine_to.map(PathBuf::from),
            fallback: params.fallback.map(PathBuf::from),
            env: params.env.map(|env| env.into_iter().collect()),
            size: params
                .cols
                .zip(params.rows)
                .map(|(cols, rows)| (usize::from(cols), usize::from(rows))),
            appearance: decode_param(params.appearance, "appearance")?,
            cell: decode_param(params.cell, "cell")?,
        };
        let mut link = self
            .starter
            .connect()
            .map_err(|error| launch_refused(&error))?;
        self.start_session(link.as_mut(), &state, surface, session, inputs, false)
    }

    pub fn sweep(
        &self,
        connection: ConnectionId,
        params: SweepParams,
    ) -> Result<Swept, ServeError> {
        if connection == ConnectionId::INTERNAL || !self.is_attached(connection) {
            return Err(refused(
                NOT_ATTACHED,
                "layout.sweep needs a connection attached with layout.attach",
            ));
        }
        let hello = paneflow_host::protocol::ClientHello::control(HOST_CLIENT);
        let mut client =
            paneflow_host::HostClient::connect(&self.core_endpoint, &hello).map_err(|error| {
                refused(
                    HOST_UNREACHABLE,
                    format!("the terminal host is unreachable: {error}"),
                )
            })?;
        let rows = client.list(None).map_err(|error| {
            refused(
                HOST_UNREACHABLE,
                format!("the terminal host did not list its sessions: {error}"),
            )
        })?;
        let listed: Vec<ListedRecord> = rows
            .into_iter()
            .map(|row| ListedRecord {
                session: row.session,
                generation: row.generation,
                owned: row.owned,
                live: row.live,
                lifecycle: row.lifecycle,
            })
            .collect();
        let (held, sweep_live) = {
            let mut core = self.lock();
            let mut held = layout_sessions(core.model.state());
            core.launching.retain(|session, started| {
                !held.contains(session) && started.elapsed() < LAUNCH_BIND_GRACE
            });
            held.extend(core.launching.keys().cloned());
            held.extend(core.model.undo_history().held_sessions());
            let sweep_live =
                params.live && core.live_sweep_due && !matches!(core.loaded, Loaded::Unreadable(_));
            if sweep_live {
                core.live_sweep_due = false;
            }
            (held, sweep_live)
        };
        let mut held = held;
        held.extend(
            params
                .keep
                .iter()
                .filter_map(|raw| raw.parse::<SessionId>().ok()),
        );
        let sweep = orphan_sweep(&listed, &held, &HashSet::new(), sweep_live);
        let mut swept = Swept::default();
        for session in sweep.forget {
            match client.remove(&session) {
                Ok(()) => {
                    log::info!(
                        "paneflow-serve: ended session {session} had no surface left; record removed"
                    );
                    swept.forgotten.push(session.to_string());
                }
                Err(error) => {
                    log::debug!("paneflow-serve: ended session {session} record kept: {error}")
                }
            }
        }
        if !sweep.stop.is_empty() {
            log::info!(
                "paneflow-serve: stopping {} live session(s) no surface holds after the restore",
                sweep.stop.len()
            );
        }
        for orphan in sweep.stop {
            match client.stop(&orphan.session, Some(orphan.generation)) {
                Ok(_) => {
                    if let Err(error) = client.remove(&orphan.session) {
                        log::debug!(
                            "paneflow-serve: session {} stopped, its record stays: {error}",
                            orphan.session
                        );
                    }
                    swept.stopped.push(orphan.session.to_string());
                }
                Err(error) if error.is_session_gone() => {}
                Err(error) => log::warn!(
                    "paneflow-serve: cannot stop orphan session {}: {error}",
                    orphan.session
                ),
            }
        }
        Ok(swept)
    }

    fn queue_effect(core: &mut Core, effect: &Effect) {
        match effect {
            Effect::Stop { session } => core.host_work.push_back(HostWork::Stop(session.clone())),
            Effect::Rehome { session, workspace } => {
                if let Some(durable) = core
                    .model
                    .state()
                    .workspace(*workspace)
                    .and_then(|ws| ws.extras.durable_id.clone())
                {
                    core.host_work.push_back(HostWork::Rehome {
                        session: session.clone(),
                        workspace: durable,
                    });
                }
            }
            Effect::Spawn { .. } | Effect::Reattach { .. } | Effect::Hold { .. } => {}
        }
    }

    pub fn adopt(
        &self,
        connection: ConnectionId,
        view: LayoutSnapshot,
        acknowledged: u64,
    ) -> Result<(Adopted, Vec<PublishedEvent>), ServeError> {
        let snapshot = snapshot_from(view)?;
        let mut core = self.lock();
        if connection != ConnectionId::INTERNAL && !core.attached.contains_key(&connection) {
            return Err(refused(
                NOT_ATTACHED,
                "layout.adopt needs a connection attached with layout.attach",
            ));
        }
        self.reload_if_rewritten(&mut core);
        if core.model.revision() > acknowledged {
            return Ok((
                Adopted {
                    accepted: false,
                    revision: core.model.revision(),
                    view: Some(snapshot_value(&core.model.snapshot())),
                },
                Vec::new(),
            ));
        }
        core.reload_before_next = false;
        let before = core.model.state().clone();
        let previous = session_sessions(&core.model);
        let revision = (core.model.revision() + 1).max(snapshot.revision);
        let events = core.model.restore(Snapshot {
            revision,
            ..snapshot
        });
        core.live_sweep_due = true;
        let kept = session_sessions(&core.model);
        let mut gone: Vec<SessionId> = previous.difference(&kept).cloned().collect();
        gone.sort_by_key(ToString::to_string);
        for session in gone {
            core.host_work.push_back(HostWork::Stop(session));
        }
        let scopes: Vec<EventScope> = events
            .iter()
            .map(|event| event.scope(&before, core.model.state()))
            .collect();
        let published = published_events(&events, &scopes, revision);
        Self::mark_dirty(&mut core);
        drop(core);
        self.wake.notify_all();
        Ok((
            Adopted {
                accepted: true,
                revision,
                view: None,
            },
            published,
        ))
    }

    pub fn status(&self) -> Value {
        let core = self.lock();
        json!({
            "revision": core.model.revision(),
            "epoch": self.epoch,
            "attached": core.attached.values().collect::<Vec<_>>(),
            "loaded": match &core.loaded {
                Loaded::Missing => "missing".to_string(),
                Loaded::Read => "read".to_string(),
                Loaded::Unreadable(cause) => format!("unreadable: {cause}"),
            },
            "pending_write": core.dirty_since.is_some(),
            "written_revision": core.written_revision,
            "writes": core.writes,
            "write_error": core.write_failure,
        })
    }

    fn take_write(&self, core: &mut Core) -> Option<(SessionState, u64, bool)> {
        core.dirty_since?;
        core.dirty_since = None;
        core.writing = true;
        let aside = std::mem::take(&mut core.aside_pending);
        Some((export_for_file(&core.model), core.model.revision(), aside))
    }

    fn set_aside(&self) {
        let stamp = crate::state::now_ms();
        let aside = self
            .path
            .with_extension(format!("json.{UNREADABLE_SUFFIX}-{stamp}"));
        match std::fs::rename(&self.path, &aside) {
            Ok(()) => log::warn!(
                "paneflow-serve: kept the unreadable session file at {}",
                aside.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                log::warn!("paneflow-serve: cannot keep the unreadable session file aside: {error}")
            }
        }
    }

    fn write(&self, session: &SessionState, revision: u64, aside: bool) {
        if aside {
            self.set_aside();
        }
        let outcome = paneflow_config::session_file::write_session_file(&self.path, session);
        let mut core = self.lock();
        core.writing = false;
        match outcome {
            Ok(()) => {
                core.file_stamp = file_stamp(&self.path);
                core.written_revision = Some(revision);
                core.writes += 1;
                core.write_failure = None;
            }
            Err(error) => {
                let cause = error.cause();
                if core.logged_causes.insert(cause.clone()) {
                    log::warn!(
                        "paneflow-serve: cannot write {}: {error}; the layout stays in memory",
                        self.path.display()
                    );
                }
                core.write_failure = Some(format!("{error}"));
                if core.dirty_since.is_none() {
                    core.dirty_since = Some(Instant::now());
                }
            }
        }
    }

    pub fn flush(&self) {
        let taken = self.take_write(&mut self.lock());
        if let Some((session, revision, aside)) = taken {
            self.write(&session, revision, aside);
        }
    }

    pub fn close(&self) {
        self.flush();
        self.lock().closed = true;
        self.wake.notify_all();
    }

    fn run_host_work(&self, work: Vec<HostWork>) {
        if work.is_empty() {
            return;
        }
        let hello = paneflow_host::protocol::ClientHello::control(HOST_CLIENT);
        let mut client = match paneflow_host::HostClient::connect(&self.core_endpoint, &hello) {
            Ok(client) => client,
            Err(error) => {
                log::warn!(
                    "paneflow-serve: {} layout effects wait for the host: {error}",
                    work.len()
                );
                return;
            }
        };
        for item in work {
            match item {
                HostWork::Stop(session) => match client.stop(&session, None) {
                    Ok(_) => {
                        if let Err(error) = client.remove(&session) {
                            log::debug!(
                                "paneflow-serve: session {session} stopped, its record stays: {error}"
                            );
                        }
                    }
                    Err(error) if error.is_session_gone() => {}
                    Err(error) => {
                        log::warn!("paneflow-serve: cannot stop session {session}: {error}")
                    }
                },
                HostWork::Rehome { session, workspace } => {
                    if let Err(error) = client.set_workspace(&session, &workspace) {
                        log::warn!(
                            "paneflow-serve: cannot move session {session} to its new workspace: {error}"
                        );
                    }
                }
            }
        }
    }

    fn next_deadline(core: &Core) -> Duration {
        let write = core
            .dirty_since
            .map(|since| (since + SAVE_DEBOUNCE).saturating_duration_since(Instant::now()));
        let now = crate::state::now_ms();
        let hold = core
            .model
            .undo_history()
            .held()
            .iter()
            .map(|held| Duration::from_millis(held.until_ms.saturating_sub(now)))
            .min();
        [write, hold]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(IDLE_WAKE)
    }

    fn run(&self) {
        loop {
            let (write, work) = {
                let mut core = self.lock();
                loop {
                    if core.closed {
                        return;
                    }
                    let due = core
                        .dirty_since
                        .is_some_and(|since| since.elapsed() >= SAVE_DEBOUNCE);
                    let expired = core.model.expire_holds(crate::state::now_ms());
                    for effect in &expired {
                        Self::queue_effect(&mut core, effect);
                    }
                    if !expired.is_empty() {
                        Self::mark_dirty(&mut core);
                    }
                    if due || !core.host_work.is_empty() {
                        break;
                    }
                    let wait = Self::next_deadline(&core).max(Duration::from_millis(1));
                    core = self
                        .wake
                        .wait_timeout(core, wait)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
                let due = core
                    .dirty_since
                    .is_some_and(|since| since.elapsed() >= SAVE_DEBOUNCE);
                let write = if due {
                    self.take_write(&mut core)
                } else {
                    None
                };
                let work: Vec<HostWork> = core.host_work.drain(..).collect();
                (write, work)
            };
            if let Some((session, revision, aside)) = write {
                self.write(&session, revision, aside);
            }
            self.run_host_work(work);
        }
    }

    pub fn start(self: &Arc<Self>) {
        let authority = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("paneflow-serve-layout".into())
            .spawn(move || authority.run());
        if let Err(error) = spawned {
            log::warn!("paneflow-serve: cannot start the layout writer: {error}");
        }
    }
}

pub fn owner_lock_held(home: &Path) -> bool {
    let path = paneflow_home::serve_owner_lock_path_in(home);
    let Ok(file) = std::fs::OpenOptions::new().read(true).open(&path) else {
        return false;
    };
    match file.try_lock_shared() {
        Ok(()) => {
            let _ = file.unlock();
            false
        }
        Err(std::fs::TryLockError::WouldBlock) => true,
        Err(std::fs::TryLockError::Error(_)) => false,
    }
}
