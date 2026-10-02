use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use paneflow_host::bootstrap::{OwnerLock, OwnerLockError};
use serde_json::{Value, json};

use crate::core_link::{CoreFrame, CoreLink};
use crate::protocol::{
    METHOD_AGENT_SNAPSHOT, REQUIRED_CORE_PROTOCOL, WORKER_PROTOCOL_VERSION, WorkerIdentity,
    advertised_capabilities,
};
use crate::server::{ServerHandle, Worker};
use crate::state::{WorkerState, now_ms};

const HEALTH_REFRESH: Duration = Duration::from_secs(30);
const SWEEP_INTERVAL: Duration = Duration::from_secs(2);
const POLL_FALLBACK: Duration = Duration::from_secs(2);
const FRAME_WAIT: Duration = Duration::from_millis(100);
const DRAIN_PER_TICK: usize = 256;
const MENU_EVIDENCE_DEADLINE: Duration = Duration::from_millis(100);

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
        let worker = Arc::clone(&self.worker);
        let home = self.worker.home.clone();
        let spawned = std::thread::Builder::new()
            .name("paneflow-serve-pump".into())
            .spawn(move || pump(&worker, &home));
        match spawned {
            Ok(handle) => self.pump = Some(handle),
            Err(error) => log::warn!("paneflow-serve: cannot start the core pump: {error}"),
        }
    }

    pub fn wait_for_shutdown(&self) {
        while !self.worker.shutdown.load(Ordering::Acquire) {
            std::thread::sleep(FRAME_WAIT);
        }
    }

    pub fn stop(mut self) {
        self.worker.shutdown.store(true, Ordering::Release);
        if let Some(pump) = self.pump.take() {
            let _ = pump.join();
        }
        if let Some(server) = self.server.take() {
            let _ = server.stop();
        }
        let _ = std::fs::remove_file(paneflow_home::serve_instance_record_path_in(
            &self.worker.home,
        ));
    }
}

fn open_with_build_id(home: &Path, build_id: String) -> Result<RunningWorker, WorkerError> {
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
    state.set_menu_attention_detection(menu_attention_detection(home));
    let rebuilt = state.rebuild_from_home(home);
    log::info!("paneflow-serve: rebuilt {rebuilt} sessions from manifests and seeds");
    let worker = Arc::new(Worker {
        identity,
        home: home.to_path_buf(),
        core_endpoint: paneflow_home::host_endpoint_path(home),
        state: Mutex::new(state),
        bus: Default::default(),
        shutdown: Arc::new(AtomicBool::new(false)),
        core_connected: AtomicBool::new(false),
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
        _owner: owner,
    };
    running.spawn_core_pump();
    Ok(running)
}

pub fn open(home: &Path) -> Result<RunningWorker, WorkerError> {
    static BUILD_ID: OnceLock<String> = OnceLock::new();
    if let Some(build_id) = BUILD_ID.get() {
        return open_with_build_id(home, build_id.clone());
    }
    let build_id = std::env::current_exe()
        .and_then(|executable| crate::protocol::executable_build_id(&executable))
        .map_err(|error| WorkerError::Storage(error.to_string()))?;
    open_with_build_id(home, BUILD_ID.get_or_init(|| build_id).clone())
}

fn menu_attention_detection(home: &Path) -> bool {
    paneflow_config::loader::load_config_from_path(&home.join("paneflow.json"))
        .menu_attention_detection_enabled()
}

fn write_instance_record(worker: &Worker) {
    let path = paneflow_home::serve_instance_record_path_in(&worker.home);
    let Ok(bytes) = serde_json::to_vec_pretty(&worker.identity) else {
        return;
    };
    if let Err(error) = paneflow_host::manifest::write_atomically(&path, &bytes) {
        log::warn!("paneflow-serve: cannot write the instance record: {error}");
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
    let running = open(home)?;
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

fn pump(worker: &Arc<Worker>, home: &Path) {
    let link = match CoreLink::follow(home) {
        Ok(link) => link,
        Err(error) => {
            log::warn!("paneflow-serve: cannot follow the core: {error}");
            return;
        }
    };
    let mut last_health = std::time::Instant::now();
    let mut last_sweep = std::time::Instant::now();
    let mut following = false;
    while !worker.shutdown.load(Ordering::Acquire) {
        if let Some(frame) = link.wait(FRAME_WAIT) {
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
            let core_endpoint = worker.core_endpoint.clone();
            let evidence = move |session: &paneflow_config::schema::SessionId| {
                crate::core_link::menu_prompt_active(
                    &core_endpoint,
                    session,
                    MENU_EVIDENCE_DEADLINE,
                )
            };
            let (settled, sessions) = {
                let mut state = worker.lock_state();
                state.refresh_health();
                let settled = state.sweep(std::time::SystemTime::now(), &evidence);
                let sessions = state.snapshot();
                (settled, sessions)
            };
            for projection in settled {
                broadcast_projection(worker, &projection, &json!({}));
            }
            worker
                .bus
                .broadcast(&json!({"type": "snapshot", "sessions": sessions}));
        }
    }
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
        }
        Err(error) => {
            worker.core_connected.store(false, Ordering::Release);
            log::debug!("paneflow-serve: cannot refresh the core snapshot: {error}");
        }
    }
}

fn apply(worker: &Arc<Worker>, frame: CoreFrame, following: bool) -> bool {
    match frame {
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
            worker
                .bus
                .broadcast(&json!({"type": "snapshot", "sessions": sessions}));
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
            let removed = paneflow_config::schema::SessionId::parse(&session)
                .is_ok_and(|session| worker.lock_state().forget_core_session(&session));
            if removed {
                let sessions = worker.lock_state().snapshot();
                worker
                    .bus
                    .broadcast(&json!({"type": "snapshot", "sessions": sessions}));
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
        CoreFrame::Cancellation(value) => {
            worker.core_connected.store(true, Ordering::Release);
            let projected = worker.lock_state().apply_cancellation(&value);
            if let Some(projection) = projected {
                broadcast_projection(worker, &projection, &json!({}));
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
        open_with_build_id(home, "test-build".to_string()).expect("the worker opens")
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
    fn menu_attention_detection_defaults_on_and_the_setting_turns_it_off() {
        let home = tempfile::tempdir().expect("a temporary home");
        let running = open_with_config(home.path(), "{}");
        assert!(running.worker().lock_state().menu_attention_detection());
        running.stop();

        let running = open_with_config(home.path(), r#"{"menu_attention_detection": false}"#);
        assert!(!running.worker().lock_state().menu_attention_detection());
        running.stop();
    }

    #[test]
    fn a_second_worker_on_the_same_home_is_refused_while_the_first_holds_the_owner_lock() {
        let home = tempfile::tempdir().expect("a temporary home");
        let first = open_with_config(home.path(), "{}");
        let expected = paneflow_home::serve_owner_lock_path_in(home.path());
        assert!(expected.ends_with("owner.lock"));
        assert!(matches!(
            open_with_build_id(home.path(), "test-build".to_string()),
            Err(WorkerError::AlreadyRunning(path)) if path == expected.display().to_string()
        ));
        first.stop();
    }
}
