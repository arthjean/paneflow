use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use crate::bootstrap::{OwnerLock, OwnerLockError};
use crate::core_link::{CoreFrame, CoreLink};
use crate::protocol::{
    METHOD_AGENT_SNAPSHOT, REQUIRED_CORE_PROTOCOL, WORKER_PROTOCOL_VERSION, WorkerIdentity,
    advertised_capabilities,
};
use crate::server::{ServerHandle, Worker};
use crate::state::{WorkerState, now_ms};

const HEALTH_REFRESH: Duration = Duration::from_secs(2);
const FRAME_WAIT: Duration = Duration::from_millis(100);
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
    let owner = OwnerLock::acquire(home).map_err(|error| match error {
        OwnerLockError::Held(path) => WorkerError::AlreadyRunning(path.display().to_string()),
        OwnerLockError::Io(io) => WorkerError::Storage(io.to_string()),
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
    let mut state = WorkerState::new();
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
    let executable =
        std::env::current_exe().map_err(|error| WorkerError::Storage(error.to_string()))?;
    let build_id = crate::protocol::executable_build_id(&executable)
        .map_err(|error| WorkerError::Storage(error.to_string()))?;
    open_with_build_id(home, build_id)
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

pub fn refresh_integrations() {
    let Some(binaries) = crate::integrations::resolve_binaries() else {
        log::info!("paneflow-serve: no helper binaries staged; integrations are left untouched");
        return;
    };
    for (runtime, result) in paneflow_mcp_install::adopt_and_refresh_installed(&binaries) {
        match result {
            Ok(()) => log::info!("paneflow-serve: refreshed the {runtime} integration"),
            Err(error) => {
                log::warn!("paneflow-serve: {runtime} integration refresh failed: {error}")
            }
        }
    }
}

pub fn run(home: &Path) -> Result<(), WorkerError> {
    let running = open(home)?;
    if let Err(error) = std::thread::Builder::new()
        .name("paneflow-serve-integrations".into())
        .spawn(refresh_integrations)
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
    while !worker.shutdown.load(Ordering::Acquire) {
        if let Some(frame) = link.wait(FRAME_WAIT) {
            apply(worker, frame);
            for frame in link.drain(DRAIN_PER_TICK) {
                apply(worker, frame);
            }
        }
        if last_health.elapsed() >= HEALTH_REFRESH {
            last_health = std::time::Instant::now();
            refresh_core_snapshot(worker);
            let sessions = {
                let mut state = worker.lock_state();
                state.refresh_health();
                state.snapshot()
            };
            worker
                .bus
                .broadcast(&json!({"type": "snapshot", "sessions": sessions}));
        }
    }
}

fn refresh_core_snapshot(worker: &Arc<Worker>) {
    match crate::core_link::call_core(&worker.core_endpoint, METHOD_AGENT_SNAPSHOT, &json!({})) {
        Ok(snapshot) => {
            let entries = snapshot["sessions"].as_array().cloned().unwrap_or_default();
            worker.lock_state().apply_core_snapshot(&entries);
            worker.core_connected.store(true, Ordering::Release);
        }
        Err(error) => {
            worker.core_connected.store(false, Ordering::Release);
            log::debug!("paneflow-serve: cannot refresh the core snapshot: {error}");
        }
    }
}

fn apply(worker: &Arc<Worker>, frame: CoreFrame) {
    match frame {
        CoreFrame::Snapshot(entries) => {
            {
                let mut state = worker.lock_state();
                state.apply_core_snapshot(&entries);
                state.refresh_health();
            }
            worker.core_connected.store(true, Ordering::Release);
            write_instance_record(worker);
            let snapshot = worker.snapshot_frame();
            worker.bus.broadcast(&with_type(snapshot, "snapshot"));
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
            if let Some(session) = projected {
                worker.bus.broadcast(&json!({
                    "type": "event",
                    "session": session["session"],
                    "kind": value["kind"],
                    "tool": value["tool"],
                    "pid": value["pid"],
                    "tool_name": value["tool_name"],
                    "exit_code": value["exit_code"],
                    "emitted_at_ms": value["emitted_at_ms"],
                    "event_source": value["event_source"],
                    "hook_payload": value["hook_payload"],
                    "agent": session["activity"],
                    "activity_source": session["activity_source"],
                    "status": session["status"],
                }));
            }
        }
        CoreFrame::Disconnected(reason) => {
            worker.core_connected.store(false, Ordering::Release);
            log::warn!("paneflow-serve: the core link dropped: {reason}");
            worker
                .bus
                .broadcast(&json!({"type": "core_disconnected", "reason": reason}));
        }
    }
}

fn with_type(mut frame: Value, kind: &str) -> Value {
    if let Some(map) = frame.as_object_mut() {
        map.insert("type".to_string(), Value::from(kind));
    }
    frame
}
