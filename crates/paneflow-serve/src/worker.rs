use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use paneflow_host::bootstrap::{OwnerLock, OwnerLockError};
use serde_json::{Value, json};

use crate::core_link::{CoreFrame, CoreLink, CoreWaker};
use crate::protocol::{
    METHOD_AGENT_SNAPSHOT, REQUIRED_CORE_PROTOCOL, WORKER_PROTOCOL_VERSION, WorkerIdentity,
    advertised_capabilities,
};
use crate::server::{ServerHandle, Worker};
use crate::state::{WorkerState, now_ms};

const HEALTH_REFRESH: Duration = Duration::from_secs(30);
const SWEEP_INTERVAL: Duration = Duration::from_secs(2);
const POLL_FALLBACK: Duration = Duration::from_secs(2);
const DRAIN_PER_TICK: usize = 256;

#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error("another paneflow worker already owns this state home: {0}")]
    AlreadyRunning(String),
    #[error("cannot prepare the worker directory: {0}")]
    Storage(String),
    #[error("cannot serve {endpoint}: {source}")]
    Endpoint {
        endpoint: PathBuf,
        source: std::io::Error,
    },
}

pub struct RunningWorker {
    worker: Arc<Worker>,
    server: Option<ServerHandle>,
    pump: Option<std::thread::JoinHandle<()>>,
    pump_waker: Option<CoreWaker>,
    _owner: OwnerLock,
}

impl RunningWorker {
    pub fn worker(&self) -> &Arc<Worker> {
        &self.worker
    }

    pub fn spawn_core_pump(&mut self) {
        if self.pump.is_some() {
            return;
        }
        let link = match CoreLink::follow(&self.worker.home) {
            Ok(link) => link,
            Err(error) => {
                log::warn!("paneflow-serve: cannot follow the core: {error}");
                return;
            }
        };
        let waker = link.waker();
        let worker = Arc::clone(&self.worker);
        let spawned = std::thread::Builder::new()
            .name("paneflow-serve-pump".into())
            .spawn(move || pump(&worker, &link));
        match spawned {
            Ok(handle) => {
                self.pump = Some(handle);
                self.pump_waker = Some(waker);
            }
            Err(error) => log::warn!("paneflow-serve: cannot start the core pump: {error}"),
        }
    }

    pub fn wait_for_shutdown(&self) {
        self.worker.wait_for_shutdown();
    }

    pub fn stop(mut self) {
        self.worker.request_shutdown();
        self.worker.pane_endpoints.close_all();
        self.worker.layout.close();
        self.worker.config.stop(crate::bootstrap::DRAIN_WAIT);
        if let Some(waker) = self.pump_waker.take() {
            waker.wake();
        }
        if let Some(pump) = self.pump.take() {
            let _ = pump.join();
        }
        self.worker.absorb_turn_signals();
        self.worker
            .lock_usage()
            .flush(std::time::Instant::now(), true);
        if let Some(server) = self.server.take() {
            let _ = server.stop();
        }
        let _ = std::fs::remove_file(paneflow_home::serve_instance_record_path_in(
            &self.worker.home,
        ));
    }
}

fn open_with_build_id(
    home: &Path,
    build_id: String,
    launching: Launching,
) -> Result<RunningWorker, WorkerError> {
    std::fs::create_dir_all(paneflow_home::serve_dir_in(home))
        .map_err(|error| WorkerError::Storage(error.to_string()))?;
    let owner =
        OwnerLock::acquire_at(&paneflow_home::serve_owner_lock_path_in(home)).map_err(|error| {
            match error {
                OwnerLockError::Held(path) => {
                    WorkerError::AlreadyRunning(path.display().to_string())
                }
                OwnerLockError::Io(io) => WorkerError::Storage(io.to_string()),
            }
        })?;
    let endpoint = paneflow_home::serve_endpoint_path(home);
    let identity = WorkerIdentity {
        name: "paneflow-serve".to_string(),
        version: crate::protocol::LOCAL_BUILD_VERSION.to_string(),
        build_id,
        protocol: WORKER_PROTOCOL_VERSION,
        required_core_protocol: REQUIRED_CORE_PROTOCOL,
        pid: std::process::id(),
        home: home.display().to_string(),
        endpoint: endpoint.display().to_string(),
        started_at_ms: now_ms(),
        capabilities: advertised_capabilities(),
    };
    let mut state = WorkerState::new(home);
    let rebuilt = state.rebuild_from_home(home);
    log::info!("paneflow-serve: rebuilt {rebuilt} sessions from manifests and seeds");
    let epoch = crate::event_log::fresh_epoch(identity.pid, identity.started_at_ms);
    let starter = launching.starter.unwrap_or_else(|| {
        crate::launcher::HostStarter::shared(paneflow_home::host_endpoint_path(home))
    });
    let layout = Arc::new(crate::layout_authority::LayoutAuthority::open_with(
        home,
        paneflow_home::host_endpoint_path(home),
        epoch.clone(),
        launching.support,
        starter,
    ));
    layout.start();
    let worker = Arc::new(Worker {
        identity,
        home: home.to_path_buf(),
        core_endpoint: paneflow_home::host_endpoint_path(home),
        state: Mutex::new(state),
        bus: Default::default(),
        sweeps: Default::default(),
        shutdown: Arc::new(AtomicBool::new(false)),
        shutdown_gate: Mutex::new(()),
        shutdown_requested: Default::default(),
        core_connected: AtomicBool::new(false),
        broadcast_snapshot: Mutex::new(None),
        usage: Mutex::new(crate::usage::Usage::open(home, now_ms())),
        probe_slots: crate::usage::probe::ProbeSlots::default(),
        events: crate::event_log::EventLog::new(epoch),
        config: Arc::new(crate::config_runtime::ConfigRuntime::new(home)),
        prompts: crate::prompts::PromptBroker::default(),
        agent_limit: Default::default(),
        action_runs: Default::default(),
        integration_refreshes: Default::default(),
        layout,
        classifier: paneflow_host::peer::PeerClassifier::new(
            Some(paneflow_home::host_endpoint_path(home)),
            Some(home.to_path_buf()),
            "paneflow-serve-classify",
        ),
        principals: Mutex::new(Default::default()),
        outside_callers: Mutex::new(Default::default()),
        genuine_callers: paneflow_host::genuine::GenuineCallers::of_this_installation(),
        agent_callers: Mutex::new(Default::default()),
        policy: paneflow_host::policy::PolicySource::for_home(home),
        acting: Default::default(),
        pane_grants: crate::pane_grants::PaneGrantStore::open(home, now_ms()),
        grant_profiles: crate::grant_methods::ProfileSource::new(paneflow_home::config_path_in(
            home,
        )),
        pane_endpoints: Default::default(),
    });
    let server = ServerHandle::spawn(Arc::clone(&worker), endpoint.clone()).map_err(|source| {
        WorkerError::Endpoint {
            endpoint: endpoint.clone(),
            source,
        }
    })?;
    write_instance_record(&worker);
    let mut running = RunningWorker {
        worker,
        server: Some(server),
        pump: None,
        pump_waker: None,
        _owner: owner,
    };
    running.spawn_core_pump();
    let config = Arc::clone(&running.worker.config);
    config.start_watching(&running.worker);
    Ok(running)
}

#[derive(Default)]
pub struct Launching {
    pub support: crate::launcher::LaunchSupport,
    pub starter: Option<Arc<dyn crate::launcher::SessionStarter>>,
}

pub fn open(home: &Path) -> Result<RunningWorker, WorkerError> {
    open_with(home, Launching::default())
}

pub fn open_with(home: &Path, launching: Launching) -> Result<RunningWorker, WorkerError> {
    static BUILD_ID: OnceLock<String> = OnceLock::new();
    if let Some(build_id) = BUILD_ID.get() {
        return open_with_build_id(home, build_id.clone(), launching);
    }
    let build_id = std::env::current_exe()
        .and_then(|executable| crate::protocol::executable_build_id(&executable))
        .map_err(|error| WorkerError::Storage(error.to_string()))?;
    open_with_build_id(home, BUILD_ID.get_or_init(|| build_id).clone(), launching)
}

fn write_instance_record(worker: &Worker) {
    let path = paneflow_home::serve_instance_record_path_in(&worker.home);
    let Ok(bytes) =
        paneflow_host::bootstrap::instance_record_json(&worker.identity, worker.identity.pid)
    else {
        return;
    };
    if let Err(error) = paneflow_host::manifest::write_atomically(&path, &bytes) {
        log::warn!("paneflow-serve: cannot write the instance record: {error}");
    }
}

impl Worker {
    pub fn agent_prompts_changed(&self) {
        self.integration_refreshes
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let home = self.home.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("paneflow-serve-integrations".into())
            .spawn(move || refresh_integrations(&home))
        {
            log::warn!("paneflow-serve: cannot rewrite the agent hooks for agent_prompts: {error}");
        }
    }
}

pub fn refresh_integrations(home: &Path) {
    refresh_integrations_as(home, cfg!(debug_assertions));
}

fn refresh_integrations_as(home: &Path, debug_build: bool) -> bool {
    remove_legacy_project_hooks(home);
    if debug_build {
        log::info!(
            "paneflow-serve: a debug build leaves the agent integrations untouched; `paneflow integrations install <runtime> --force` points them at it"
        );
        return false;
    }
    let Some(binaries) = crate::integrations::resolve_binaries(home) else {
        log::info!("paneflow-serve: no helper binaries staged; integrations are left untouched");
        return false;
    };
    for (runtime, result) in paneflow_mcp_install::adopt_and_refresh_installed(home, &binaries) {
        match result {
            Ok(()) => log::info!("paneflow-serve: refreshed the {runtime} integration"),
            Err(error) => {
                log::warn!("paneflow-serve: {runtime} integration refresh failed: {error}")
            }
        }
    }
    true
}

fn session_project_dirs(session: &[u8]) -> Vec<PathBuf> {
    let Ok(state) = serde_json::from_slice::<paneflow_config::schema::SessionState>(session) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = Vec::new();
    for workspace in &state.workspaces {
        let candidates = std::iter::once(workspace.cwd.as_str()).chain(
            workspace
                .tabs
                .iter()
                .filter_map(|tab| tab.worktree.as_deref()),
        );
        for candidate in candidates {
            let dir = PathBuf::from(candidate);
            if dir.is_absolute() && !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
    }
    dirs
}

fn remove_legacy_project_hooks(home: &Path) {
    let Ok(session) = std::fs::read(home.join("session.json")) else {
        return;
    };
    for (path, result) in
        paneflow_mcp_install::remove_legacy_project_hooks(&session_project_dirs(&session))
    {
        match result {
            Ok(()) => log::info!(
                "paneflow-serve: removed pre-0.17 Paneflow hooks from {}",
                path.display()
            ),
            Err(error) => log::warn!(
                "paneflow-serve: cannot remove pre-0.17 Paneflow hooks from {}: {error}",
                path.display()
            ),
        }
    }
}

pub fn run(home: &Path) -> Result<(), WorkerError> {
    run_with(home, crate::launcher::LaunchSupport::default())
}

pub fn run_with(home: &Path, support: crate::launcher::LaunchSupport) -> Result<(), WorkerError> {
    let running = open_with(
        home,
        Launching {
            support,
            starter: None,
        },
    )?;
    let accounts =
        crate::usage::configured_accounts(home, paneflow_agent_config::home_dir().as_deref());
    running.worker().start_import(accounts, false);
    let integrations_home = home.to_path_buf();
    if let Err(error) = std::thread::Builder::new()
        .name("paneflow-serve-integrations".into())
        .spawn(move || refresh_integrations(&integrations_home))
    {
        log::warn!("paneflow-serve: cannot start the integration refresh: {error}");
    }
    running.wait_for_shutdown();
    running.stop();
    Ok(())
}

fn pump(worker: &Arc<Worker>, link: &CoreLink) {
    let mut last_health = std::time::Instant::now();
    let mut last_sweep = std::time::Instant::now();
    let mut following = false;
    while !worker.shutdown.load(Ordering::Acquire) {
        let until_health = core_poll_interval(following).saturating_sub(last_health.elapsed());
        let until_sweep = SWEEP_INTERVAL.saturating_sub(last_sweep.elapsed());
        if let Some(frame) = link.wait(until_health.min(until_sweep)) {
            following = apply(worker, frame, following);
            for frame in link.drain(DRAIN_PER_TICK) {
                following = apply(worker, frame, following);
            }
        }
        if last_health.elapsed() >= core_poll_interval(following) {
            last_health = std::time::Instant::now();
            refresh_core_snapshot(worker);
        }
        if last_sweep.elapsed() >= SWEEP_INTERVAL {
            last_sweep = std::time::Instant::now();
            sweep(worker);
        }
    }
}

fn sweep(worker: &Arc<Worker>) {
    worker.sweeps.increment();
    let (settled, sessions) = {
        let mut state = worker.lock_state();
        state.refresh_health();
        let settled = state.sweep();
        let sessions = state.snapshot();
        (settled, sessions)
    };
    for projection in settled {
        broadcast_projection(worker, &projection, &json!({}));
    }
    worker.absorb_state_events();
    worker.broadcast_snapshot_if_changed(sessions);
    worker.absorb_turn_signals();
    worker.sweep_usage();
    worker.reconcile_pane_grants();
    worker.reconcile_pane_endpoints();
}

fn core_poll_interval(following: bool) -> Duration {
    if following {
        HEALTH_REFRESH
    } else {
        POLL_FALLBACK
    }
}

fn refresh_core_snapshot(worker: &Arc<Worker>) {
    match crate::core_link::call_core(&worker.core_endpoint, METHOD_AGENT_SNAPSHOT, &json!({})) {
        Ok(snapshot) => {
            let entries = snapshot["sessions"].as_array().cloned().unwrap_or_default();
            let projections = worker.lock_state().apply_core_snapshot(&entries);
            worker.core_connected.store(true, Ordering::Release);
            for projection in projections {
                broadcast_projection(worker, &projection, &json!({}));
            }
            worker.absorb_state_events();
        }
        Err(error) => {
            worker.core_connected.store(false, Ordering::Release);
            log::debug!("paneflow-serve: cannot refresh the core snapshot: {error}");
        }
    }
}

pub(crate) fn apply(worker: &Arc<Worker>, frame: CoreFrame, following: bool) -> bool {
    let connected = matches!(frame, CoreFrame::Snapshot(_));
    let following = apply_frame(worker, frame, following);
    worker.absorb_state_events();
    worker.absorb_turn_signals();
    worker.reconcile_pane_grants();
    worker.reconcile_pane_endpoints();
    if connected {
        worker.push_grants_to_host(&worker.pane_grants.set());
    }
    following
}

fn apply_frame(worker: &Arc<Worker>, frame: CoreFrame, following: bool) -> bool {
    match frame {
        CoreFrame::PlanLimits(value) => {
            worker.core_connected.store(true, Ordering::Release);
            worker.absorb_plan_limits(&value);
        }
        CoreFrame::Snapshot(entries) => {
            let projections = {
                let mut state = worker.lock_state();
                let projections = state.apply_core_snapshot(&entries);
                state.refresh_health();
                projections
            };
            worker.core_connected.store(true, Ordering::Release);
            for projection in projections {
                broadcast_projection(worker, &projection, &json!({}));
            }
            write_instance_record(worker);
            let sessions = worker.lock_state().snapshot();
            worker.broadcast_snapshot_if_changed(sessions);
            return true;
        }
        CoreFrame::Session(entry) => {
            let projections = worker.lock_state().apply_core_session(&entry);
            worker.core_connected.store(true, Ordering::Release);
            for projection in projections {
                broadcast_projection(worker, &projection, &json!({}));
            }
        }
        CoreFrame::SessionRemoved(session) => {
            worker.prompts.cancel_asked_by(&session);
            let removed = paneflow_config::schema::SessionId::parse(&session)
                .is_ok_and(|session| worker.lock_state().forget_core_session(&session));
            if removed {
                let sessions = worker.lock_state().snapshot();
                worker.broadcast_snapshot_if_changed(sessions);
            }
        }
        CoreFrame::Event(value) => {
            worker.core_connected.store(true, Ordering::Release);
            let missing = value["session"]
                .as_str()
                .and_then(|raw| paneflow_config::schema::SessionId::parse(raw).ok())
                .is_some_and(|session| !worker.lock_state().contains(&session));
            if missing {
                refresh_core_snapshot(worker);
            }
            let projected = worker.lock_state().apply_core_event(&value);
            if let Some(projection) = projected {
                broadcast_projection(worker, &projection, &value);
            }
        }
        CoreFrame::Disconnected(reason) => {
            worker.core_connected.store(false, Ordering::Release);
            log::warn!("paneflow-serve: the core link dropped: {reason}");
            return false;
        }
        CoreFrame::Refused(reason) => {
            log::warn!(
                "paneflow-serve: the core refused the agent subscription ({reason}); polling its snapshot every {}s until the next attempt in {}s",
                POLL_FALLBACK.as_secs(),
                crate::core_link::REFUSED_RETRY.as_secs()
            );
            return false;
        }
        CoreFrame::Wake => {}
    }
    following
}

fn broadcast_projection(
    worker: &Arc<Worker>,
    projection: &crate::state::Projection,
    source: &Value,
) {
    worker.publish(projection, source);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_sweep_counts_one_sweep_and_one_snapshot_broadcast_in_worker_status() {
        let home = tempfile::tempdir().expect("a temporary home");
        let running = open_with_config(home.path(), "{}");
        let worker = running.worker();
        let names = paneflow_host::work_counters::WORKER_COUNTERS;
        let before = paneflow_host::work_counters::sample("worker", &worker.status_frame(), names);
        sweep(worker);
        let after = paneflow_host::work_counters::sample("worker", &worker.status_frame(), names);
        let paneflow_host::work_counters::Window::Deltas(deltas) =
            paneflow_host::work_counters::window(&before, &after)
        else {
            panic!("the worker did not restart");
        };
        let measured = paneflow_host::work_counters::Reading::Measured;
        assert_eq!(deltas["sweeps"], measured(1));
        assert_eq!(deltas["snapshot_broadcasts"], measured(1));
        assert_eq!(deltas["projection_broadcasts"], measured(0));
        running.stop();
    }

    #[test]
    fn open_turns_outlive_sweeps_until_the_core_reports_its_sessions() {
        use crate::usage::ledger::{LedgerAccount, TurnInput, TurnSignal};

        let home = tempfile::tempdir().expect("a temporary home");
        let running = open_with_config(home.path(), "{}");
        let worker = running.worker();
        worker.lock_usage().apply_turn_signals(&[TurnSignal {
            session: "restored".to_string(),
            account: LedgerAccount {
                agent: "claude-code".to_string(),
                config_dir: home.path().join(".claude").display().to_string(),
            },
            input: TurnInput::Prompt,
            at_ms: now_ms(),
            order: 1,
            transcript: None,
        }]);
        assert!(!worker.core_connected.load(Ordering::Acquire));
        worker.sweep_usage();
        assert_eq!(
            worker.lock_usage().ledger().book().open_turns(),
            1,
            "a sweep before the first core snapshot cannot tell a live session from a gone one"
        );
        running.stop();
    }

    #[test]
    fn a_shutdown_request_wakes_the_thread_waiting_for_it() {
        let home = tempfile::tempdir().expect("a temporary home");
        let running = open_with_config(home.path(), "{}");
        let worker = Arc::clone(running.worker());
        let waiter = std::thread::spawn(move || worker.wait_for_shutdown());
        std::thread::sleep(Duration::from_millis(50));
        assert!(!waiter.is_finished(), "nothing requested a shutdown yet");
        running.worker().request_shutdown();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !waiter.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(waiter.is_finished(), "the request wakes the waiting thread");
        waiter.join().expect("the waiter returns");
        running.stop();
    }

    fn snapshot_broadcasts(worker: &Worker) -> u64 {
        worker.bus.broadcasts().snapshot.get()
    }

    fn running_row(session: &paneflow_config::schema::SessionId, title: &str) -> Value {
        json!({
            "session": session,
            "generation": paneflow_config::schema::SessionGeneration::FIRST,
            "live": true,
            "lifecycle": paneflow_host::manifest::SessionLifecycle::Running,
            "title": title,
            "host_protocol_version": paneflow_host::HOST_PROTOCOL_VERSION,
            "host_build_id": "test-build",
        })
    }

    #[test]
    fn a_sweep_broadcasts_the_snapshot_only_when_its_content_changed() {
        let home = tempfile::tempdir().expect("a temporary home");
        let running = open_with_config(home.path(), "{}");
        let worker = running.worker();
        let session = paneflow_config::schema::SessionId::new();
        worker
            .lock_state()
            .apply_core_session(&running_row(&session, "agent"));
        sweep(worker);
        let initial = snapshot_broadcasts(worker);
        assert_eq!(initial, 1, "a fresh worker broadcasts its first snapshot");

        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(2));
            worker
                .lock_state()
                .apply_core_session(&running_row(&session, "agent"));
            sweep(worker);
        }
        assert_eq!(
            snapshot_broadcasts(worker),
            initial,
            "an unchanged state is not rebroadcast, whatever its update stamp"
        );

        worker
            .lock_state()
            .apply_core_session(&running_row(&session, "renamed"));
        sweep(worker);
        assert_eq!(snapshot_broadcasts(worker), initial + 1);
        sweep(worker);
        assert_eq!(snapshot_broadcasts(worker), initial + 1);

        assert!(apply(
            worker,
            CoreFrame::SessionRemoved(session.to_string()),
            true
        ));
        assert_eq!(snapshot_broadcasts(worker), initial + 2);
        running.stop();
    }

    #[test]
    fn a_restarted_worker_broadcasts_its_initial_snapshot_even_when_identical() {
        let home = tempfile::tempdir().expect("a temporary home");
        let first = open_with_config(home.path(), "{}");
        sweep(first.worker());
        assert_eq!(snapshot_broadcasts(first.worker()), 1);
        first.stop();

        let restarted = open_with_config(home.path(), "{}");
        let worker = restarted.worker();
        assert!(apply(worker, CoreFrame::Snapshot(Vec::new()), false));
        assert_eq!(snapshot_broadcasts(worker), 1);
        sweep(worker);
        assert_eq!(snapshot_broadcasts(worker), 1);
        let header = worker.snapshot_frame();
        assert!(
            header["sessions"].is_array(),
            "a reconnecting follower still receives the full snapshot header"
        );
        restarted.stop();
    }

    #[test]
    fn a_refused_subscription_falls_back_to_the_two_second_snapshot_poll() {
        let home = tempfile::tempdir().expect("a temporary home");
        let running = open_with_config(home.path(), "{}");
        let worker = running.worker();
        assert!(!apply(
            worker,
            CoreFrame::Refused("refused".to_string()),
            true
        ));
        assert_eq!(core_poll_interval(false), Duration::from_secs(2));
        assert!(apply(worker, CoreFrame::Snapshot(Vec::new()), false));
        assert_eq!(core_poll_interval(true), Duration::from_secs(30));
        assert!(!apply(
            worker,
            CoreFrame::Disconnected("gone".to_string()),
            true
        ));
        running.stop();
    }

    fn open_with_config(home: &Path, body: &str) -> RunningWorker {
        std::fs::write(home.join("paneflow.json"), body).expect("the config is written");
        open_with_build_id(home, "test-build".to_string(), Launching::default())
            .expect("the worker opens")
    }

    #[test]
    fn a_debug_worker_removes_pre_0_17_project_hooks_but_leaves_integrations_alone() {
        let home = tempfile::tempdir().expect("a temporary home");
        let project = tempfile::tempdir().expect("a project");
        let untracked = tempfile::tempdir().expect("a project absent from the session");
        let mut legacy = json!({ "permissions": { "allow": ["Bash(ls)"] }, "hooks": {} });
        for event in paneflow_agent_config::claude_hooks::CLAUDE_HOOK_EVENTS
            .iter()
            .chain(paneflow_agent_config::claude_hooks::CLAUDE_RETIRED_HOOK_EVENTS)
        {
            legacy["hooks"][*event] = json!([{
                "_paneflow_managed": true,
                "hooks": [{ "type": "command", "command": format!("/old/bin/paneflow-ai-hook {event}"), "timeout": 5 }],
            }]);
        }
        legacy["hooks"]["Stop"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "hooks": [{ "type": "command", "command": "my-hook" }] }));
        for dir in [project.path(), untracked.path()] {
            std::fs::create_dir_all(dir.join(".claude")).unwrap();
            std::fs::write(
                dir.join(".claude").join("settings.local.json"),
                serde_json::to_vec_pretty(&legacy).unwrap(),
            )
            .unwrap();
        }
        let session = json!({
            "version": 3,
            "active_workspace": 0,
            "workspaces": [{ "title": "p", "cwd": project.path(), "tabs": [] }],
        });
        std::fs::write(
            home.path().join("session.json"),
            serde_json::to_vec(&session).unwrap(),
        )
        .unwrap();

        assert!(
            !refresh_integrations_as(home.path(), true),
            "a debug worker never rewrites the user's agent integrations"
        );

        let cleaned: Value = serde_json::from_slice(
            &std::fs::read(project.path().join(".claude").join("settings.local.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(cleaned["permissions"]["allow"][0], "Bash(ls)");
        let text = cleaned.to_string();
        assert!(!text.contains("paneflow-ai-hook"), "{text}");
        assert_eq!(
            cleaned["hooks"]["Stop"][0]["hooks"][0]["command"],
            "my-hook"
        );
        let untouched =
            std::fs::read_to_string(untracked.path().join(".claude").join("settings.local.json"))
                .unwrap();
        assert!(untouched.contains("paneflow-ai-hook"));
    }

    #[test]
    fn a_second_worker_on_the_same_home_is_refused_while_the_first_holds_the_owner_lock() {
        let home = tempfile::tempdir().expect("a temporary home");
        let first = open_with_config(home.path(), "{}");
        let expected = paneflow_home::serve_owner_lock_path_in(home.path());
        assert!(expected.ends_with("owner.lock"));
        assert!(matches!(
            open_with_build_id(home.path(), "test-build".to_string(), Launching::default()),
            Err(WorkerError::AlreadyRunning(path)) if path == expected.display().to_string()
        ));
        first.stop();
    }

    fn agent_question(
        session: Option<&str>,
        kind: &str,
    ) -> paneflow_api::methods::prompt::RequestParams {
        let mut origin = json!({"kind": kind});
        if let Some(session) = session {
            origin["session"] = json!(session);
        }
        serde_json::from_value(json!({"kind": "input", "title": "Deploy?", "origin": origin}))
            .expect("a prompt request")
    }

    fn pending(outcome: paneflow_api::methods::prompt::PromptOutcome) -> u64 {
        assert_eq!(
            outcome.status,
            paneflow_api::methods::prompt::PromptStatus::Pending,
            "{outcome:?}"
        );
        outcome.id.expect("a pending prompt has an id")
    }

    fn exited_row(session: &paneflow_config::schema::SessionId) -> Value {
        let mut row = running_row(session, "agent");
        row["live"] = json!(false);
        row["lifecycle"] = json!(paneflow_host::manifest::SessionLifecycle::Exited {
            code: 0,
            signal: None,
        });
        row
    }

    #[test]
    fn an_agent_asking_past_its_burst_is_rate_limited_before_the_broker_sees_it() {
        use paneflow_api::Principal;
        use paneflow_api::methods::prompt::{PromptKind, PromptStatus};
        let home = tempfile::tempdir().expect("a temporary home");
        let running = open_with_config(home.path(), "{}");
        let worker = running.worker();
        let window = worker
            .prompts
            .follow(vec![PromptKind::Input], Principal::Window);
        for _ in 0..crate::agent_asks::AGENT_BURST {
            pending(
                worker
                    .request_prompt(
                        agent_question(Some("a1"), "agent"),
                        &Principal::External,
                        crate::layout_authority::ConnectionId::INTERNAL,
                    )
                    .unwrap(),
            );
        }
        let refused = worker
            .request_prompt(
                agent_question(Some("a1"), "agent"),
                &Principal::External,
                crate::layout_authority::ConnectionId::INTERNAL,
            )
            .unwrap();
        assert_eq!(refused.status, PromptStatus::RateLimited);
        assert_eq!(refused.id, None, "nothing was queued");
        assert_eq!(
            window.frames.try_iter().count(),
            3,
            "the window was shown only the admitted questions"
        );
        pending(
            worker
                .request_prompt(
                    agent_question(Some("a2"), "agent"),
                    &Principal::External,
                    crate::layout_authority::ConnectionId::INTERNAL,
                )
                .unwrap(),
        );
        let script = worker
            .request_prompt(
                agent_question(None, "script"),
                &Principal::External,
                crate::layout_authority::ConnectionId::INTERNAL,
            )
            .unwrap();
        assert_eq!(
            script.status,
            PromptStatus::Pending,
            "a script is never limited as an agent"
        );
        running.stop();
    }

    #[test]
    fn the_end_of_the_asking_agent_session_cancels_its_pending_question() {
        use paneflow_api::Principal;
        use paneflow_api::methods::prompt::{PromptKind, PromptStatus};
        let home = tempfile::tempdir().expect("a temporary home");
        let running = open_with_config(home.path(), "{}");
        let worker = running.worker();
        let _window = worker
            .prompts
            .follow(vec![PromptKind::Input], Principal::Window);
        let exiting = paneflow_config::schema::SessionId::new();
        let removed = paneflow_config::schema::SessionId::new();
        let staying = paneflow_config::schema::SessionId::new();
        for session in [&exiting, &removed, &staying] {
            apply(
                worker,
                CoreFrame::Session(Box::new(running_row(session, "agent"))),
                true,
            );
        }
        let ask = |session: &paneflow_config::schema::SessionId| {
            pending(
                worker
                    .request_prompt(
                        agent_question(Some(&session.to_string()), "agent"),
                        &Principal::External,
                        crate::layout_authority::ConnectionId::INTERNAL,
                    )
                    .unwrap(),
            )
        };
        let (on_exit, on_removal, kept) = (ask(&exiting), ask(&removed), ask(&staying));
        apply(
            worker,
            CoreFrame::Session(Box::new(exited_row(&exiting))),
            true,
        );
        apply(worker, CoreFrame::SessionRemoved(removed.to_string()), true);
        for id in [on_exit, on_removal] {
            assert_eq!(
                worker.prompts.wait(id, None).unwrap().status,
                PromptStatus::Cancelled
            );
        }
        assert_eq!(
            worker.prompts.wait(kept, None).unwrap().status,
            PromptStatus::Pending
        );
        running.stop();
    }

    #[test]
    fn a_pane_is_the_asker_of_its_question_and_no_other_pane_can_wait_on_it() {
        use paneflow_api::Principal;
        use paneflow_api::methods::prompt::{OriginKind, PromptKind};
        let home = tempfile::tempdir().expect("a temporary home");
        let running = open_with_config(home.path(), "{}");
        let worker = running.worker();
        let window = worker
            .prompts
            .follow(vec![PromptKind::Input], Principal::Window);
        let pane = |session: &str| Principal::Pane {
            session: session.to_string(),
            workspace: None,
        };
        let id = pending(
            worker
                .request_prompt(
                    agent_question(Some("forged"), "agent"),
                    &pane("real"),
                    crate::layout_authority::ConnectionId::INTERNAL,
                )
                .unwrap(),
        );
        let shown = window
            .frames
            .try_recv()
            .expect("the window is shown the question");
        assert_eq!(shown["origin"]["session"], "real");
        assert!(worker.wait_prompt(id, None, &pane("forged")).is_err());
        assert!(worker.wait_prompt(id, None, &pane("other")).is_err());
        assert_eq!(
            pending(worker.wait_prompt(id, None, &pane("real")).unwrap()),
            id
        );
        assert_eq!(
            pending(worker.wait_prompt(id, None, &Principal::Window).unwrap()),
            id,
            "a caller outside every pane is not a competing agent"
        );
        let agent = paneflow_config::schema::SessionId::new();
        apply(
            worker,
            CoreFrame::Session(Box::new(running_row(&agent, "agent"))),
            true,
        );
        let mut started = json!({
            "session": agent.to_string(),
            "kind": "ai.prompt_submit",
            "tool": "claude",
            "pid": std::process::id(),
            "hook_payload": {"hook_event_name": "UserPromptSubmit"},
        });
        started["emitted_at_ms"] = json!(crate::state::now_ms());
        apply(worker, CoreFrame::Event(Box::new(started)), true);
        assert!(worker.lock_state().get(&agent).unwrap().activity.is_some());
        let disguised = worker
            .attribute_prompt(
                agent_question(None, "action").origin,
                &pane(&agent.to_string()),
                crate::layout_authority::ConnectionId::INTERNAL,
            )
            .expect("an origin");
        assert_eq!(disguised.kind, OriginKind::Agent);
        assert_eq!(
            disguised.session.as_deref(),
            Some(agent.to_string().as_str())
        );
        assert!(
            disguised
                .agent
                .as_deref()
                .is_some_and(|label| !label.is_empty()),
            "{disguised:?}"
        );
        let plain = worker
            .attribute_prompt(
                agent_question(None, "script").origin,
                &pane("a-shell"),
                crate::layout_authority::ConnectionId::INTERNAL,
            )
            .expect("an origin");
        assert_eq!(
            plain.kind,
            OriginKind::Script,
            "a pane with no agent keeps the origin it claims"
        );
        let mut forged = agent_question(None, "agent").origin.expect("an origin");
        forged.agent = Some("Paneflow".into());
        let unplaced = worker
            .attribute_prompt(
                Some(forged),
                &Principal::External,
                crate::layout_authority::ConnectionId::INTERNAL,
            )
            .expect("an origin");
        assert_eq!(
            unplaced.agent, None,
            "a claim with no session never shows an unknown name"
        );
        running.stop();
    }

    #[test]
    fn an_action_run_from_an_agent_pane_runs_as_that_agent_in_its_own_pane() {
        use paneflow_api::Principal;
        use paneflow_api::methods::config::InvokeParams;
        let home = tempfile::tempdir().expect("a temporary home");
        std::fs::write(
            paneflow_home::init_lua_path_in(home.path()),
            "paneflow.action{ name = 'where', title = 'Where', agents = true, run = function(ctx)\n\
               return { origin = ctx.origin, surface = ctx.surface_id, workspace = ctx.workspace_id } end }\n\
             paneflow.action{ name = 'secret', title = 'Secret', run = function() return 'hidden' end }\n",
        )
        .expect("init.lua");
        let running = open_with_config(home.path(), "{}");
        let worker = running.worker();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while worker.config.actions().actions.len() < 2 {
            assert!(
                std::time::Instant::now() < deadline,
                "init.lua never loaded"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let agent = paneflow_config::schema::SessionId::new();
        apply(
            worker,
            CoreFrame::Session(Box::new(running_row(&agent, "agent"))),
            true,
        );
        let mut started = json!({
            "session": agent.to_string(),
            "kind": "ai.prompt_submit",
            "tool": "claude",
            "pid": std::process::id(),
            "hook_payload": {"hook_event_name": "UserPromptSubmit"},
        });
        started["emitted_at_ms"] = json!(crate::state::now_ms());
        apply(worker, CoreFrame::Event(Box::new(started)), true);
        let pane = Principal::Pane {
            session: agent.to_string(),
            workspace: None,
        };
        let invoke = |name: &str, origin: &str| {
            serde_json::from_value::<InvokeParams>(json!({"name": name, "ctx": {"origin": origin}}))
                .expect("params")
        };
        let ran = worker
            .invoke_action(
                invoke("where", "cli"),
                &pane,
                crate::layout_authority::ConnectionId::INTERNAL,
            )
            .expect("a marked action runs");
        assert_eq!(ran.result.as_ref().unwrap()["origin"], "agent");
        let refused = worker
            .invoke_action(
                invoke("secret", "cli"),
                &pane,
                crate::layout_authority::ConnectionId::INTERNAL,
            )
            .expect_err("an agent pane never reaches an unmarked action through the cli");
        assert!(
            refused.message.contains("no action named 'secret'"),
            "{}",
            refused.message
        );
        let shell = Principal::Pane {
            session: paneflow_config::schema::SessionId::new().to_string(),
            workspace: None,
        };
        assert_eq!(
            worker
                .invoke_action(
                    invoke("secret", "cli"),
                    &shell,
                    crate::layout_authority::ConnectionId::INTERNAL
                )
                .expect("a pane with no agent runs every action")
                .result,
            Some(json!("hidden"))
        );
        running.stop();
    }

    #[test]
    fn a_run_whose_runtime_stops_before_it_returns_ends_with_an_error_for_its_waiter() {
        use paneflow_api::Principal;
        use paneflow_api::methods::config::InvokeContext;
        let home = tempfile::tempdir().expect("a temporary home");
        let running = open_with_config(home.path(), "{}");
        let worker = running.worker();
        let mut context = InvokeContext::default();
        let token = worker
            .acting
            .start_detached(Principal::External, &mut context);
        let run_id = worker.action_runs.open(Some("a1".into())).unwrap();
        let dropped_uncalled = crate::agent_asks::RunEnd::of(worker, &token, &run_id);
        assert_eq!(worker.action_runs.running(), 1);
        drop(dropped_uncalled);
        assert_eq!(worker.action_runs.running(), 0);
        assert_eq!(
            worker
                .action_runs
                .wait(&run_id, None, Some("a1"))
                .expect_err("the run ended without a result")
                .message,
            crate::config_runtime::runtime_stopped().message
        );
        let (principal, _) = worker.acting.resolve(context.grant.as_deref());
        assert_ne!(
            principal,
            Principal::External,
            "the run's grant no longer acts as its caller"
        );
        running.stop();
    }

    #[test]
    fn an_agent_question_left_unanswered_ends_at_the_shared_deadline_and_declines() {
        use paneflow_api::Principal;
        use paneflow_api::methods::prompt::{PromptKind, ResolveParams, ResolveStatus};
        let home = tempfile::tempdir().expect("a temporary home");
        let running = open_with_config(home.path(), r#"{"agent_prompts": "paneflow"}"#);
        let worker = running.worker();
        let window = worker
            .prompts
            .follow(vec![PromptKind::Pick], Principal::Window);
        let params: ResolveParams = serde_json::from_value(json!({
            "event": "ask_user_question",
            "agent": "claude",
            "payload": {"tool_name": "AskUserQuestion", "tool_input": {"questions": [
                {"question": "First?", "options": [{"label": "a"}], "multiSelect": false},
                {"question": "Second?", "options": [{"label": "b"}], "multiSelect": false}
            ]}}
        }))
        .expect("params");
        let resolved = worker.resolve_agent_prompt_within(
            params,
            &Principal::External,
            crate::layout_authority::ConnectionId::INTERNAL,
            std::time::Duration::from_millis(300),
        );
        assert_eq!(resolved.status, ResolveStatus::Declined);
        assert_eq!(resolved.reason.as_deref(), Some("timeout"));
        let frames: Vec<Value> = window.frames.try_iter().collect();
        let requested: Vec<&Value> = frames
            .iter()
            .filter(|frame| frame["type"] == "requested")
            .collect();
        assert_eq!(
            requested.len(),
            1,
            "the second question is never asked: {frames:?}"
        );
        assert_eq!(requested[0]["spec"]["title"], "First?");
        assert!(
            frames.iter().any(|frame| frame["type"] == "ended"),
            "the open prompt ends with the deadline: {frames:?}"
        );
        running.stop();
    }

    #[test]
    fn a_change_of_agent_prompts_in_paneflow_json_rewrites_the_agent_hooks_once() {
        use std::sync::atomic::Ordering;
        let home = tempfile::tempdir().expect("a temporary home");
        let running = open_with_config(home.path(), "{}");
        let worker = running.worker();
        let settled = |count: u64| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while worker.integration_refreshes.load(Ordering::Relaxed) < count {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the hooks were never rewritten"
                );
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        };
        let config = home.path().join("paneflow.json");
        std::fs::write(&config, r#"{"agent_prompts": "paneflow"}"#).expect("opt in");
        settled(1);
        std::fs::write(
            &config,
            r#"{"agent_prompts": "paneflow", "theme": "One Dark"}"#,
        )
        .expect("an unrelated edit");
        std::fs::write(&config, r#"{"theme": "One Dark"}"#).expect("back to native");
        settled(2);
        std::thread::sleep(crate::config_runtime::RELOAD_SETTLE * 4);
        assert_eq!(
            worker.integration_refreshes.load(Ordering::Relaxed),
            2,
            "an edit that keeps agent_prompts rewrites nothing"
        );
        running.stop();
    }
}
