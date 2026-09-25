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

const HEALTH_REFRESH: Duration = Duration::from_secs(2);
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

fn apply(worker: &Arc<Worker>, frame: CoreFrame) {
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
        }
    }
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

    fn open_with_config(home: &Path, body: &str) -> RunningWorker {
        std::fs::write(home.join("paneflow.json"), body).expect("the config is written");
        open_with_build_id(home, "test-build".to_string()).expect("the worker opens")
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
