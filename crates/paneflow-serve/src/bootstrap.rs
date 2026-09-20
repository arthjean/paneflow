use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use paneflow_host::bootstrap::{BootstrapError, release_child, spawn_detached};
use paneflow_ipc_client::host_control::HostControl;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::protocol::{METHOD_WORKER_HELLO, METHOD_WORKER_SHUTDOWN, WorkerIdentity};

pub const STARTUP_WAIT: Duration = Duration::from_secs(10);
pub const DRAIN_WAIT: Duration = Duration::from_secs(5);
const STARTUP_POLL: Duration = Duration::from_millis(50);
const OWNER_LOCK_WAIT: Duration = Duration::from_millis(500);
const LOCK_RETRY: Duration = Duration::from_millis(25);
const MAX_INSTANCE_RECORD_BYTES: u64 = 64 * 1024;
const CLIENT_NAME: &str = "paneflow-serve-bootstrap";

#[derive(Debug, thiserror::Error)]
pub enum OwnerLockError {
    #[error("another paneflow worker already owns this state home (lock {0})")]
    Held(PathBuf),
    #[error("cannot take the worker owner lock: {0}")]
    Io(#[from] io::Error),
}

pub struct OwnerLock {
    _file: File,
    path: PathBuf,
}

impl OwnerLock {
    pub fn acquire(home: &Path) -> Result<Self, OwnerLockError> {
        let path = paneflow_home::serve_owner_lock_path_in(home);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        let deadline = Instant::now() + OWNER_LOCK_WAIT;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { _file: file, path }),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(LOCK_RETRY);
                }
                Err(TryLockError::WouldBlock) => return Err(OwnerLockError::Held(path)),
                Err(TryLockError::Error(error)) => return Err(OwnerLockError::Io(error)),
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Debug)]
pub enum Probe {
    Running(Box<WorkerIdentity>),
    Unreachable(String),
    Faulted(String),
}

pub fn probe(home: &Path, endpoint: &Path) -> Probe {
    let mut control = match HostControl::connect(endpoint, CLIENT_NAME) {
        Ok(control) => control,
        Err(error) => return Probe::Unreachable(error),
    };
    let answered = match control.request(METHOD_WORKER_HELLO, json!({"client": CLIENT_NAME})) {
        Ok(value) => value,
        Err(error) => return Probe::Faulted(error),
    };
    let identity: WorkerIdentity = match serde_json::from_value(answered) {
        Ok(identity) => identity,
        Err(error) => {
            return Probe::Faulted(format!(
                "the endpoint {} did not answer with a worker identity: {error}",
                endpoint.display()
            ));
        }
    };
    if paneflow_home::home_fingerprint(Path::new(&identity.home))
        != paneflow_home::home_fingerprint(home)
    {
        return Probe::Faulted(format!(
            "the worker on {} serves {} instead of {}",
            endpoint.display(),
            identity.home,
            home.display()
        ));
    }
    Probe::Running(Box::new(identity))
}

pub fn read_instance_record(home: &Path) -> Option<WorkerIdentity> {
    use std::io::Read;
    let file = File::open(paneflow_home::serve_instance_record_path_in(home)).ok()?;
    if file.metadata().ok()?.len() > MAX_INSTANCE_RECORD_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_INSTANCE_RECORD_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerAdoption {
    pub identity: WorkerIdentity,
    pub started: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkerBootstrapError {
    #[error("the worker endpoint {endpoint} cannot be served: {reason}")]
    EndpointFaulted { endpoint: PathBuf, reason: String },
    #[error("cannot start the worker: {0}")]
    Spawn(#[from] BootstrapError),
    #[error("cannot prepare worker executable {path}: {source}")]
    Executable { path: PathBuf, source: io::Error },
    #[error("the worker did not serve {endpoint} within {waited:?}; see {log}")]
    StartupTimeout {
        endpoint: PathBuf,
        waited: Duration,
        log: PathBuf,
    },
    #[error("the worker exited before serving {endpoint}; see {log}")]
    ExitedEarly { endpoint: PathBuf, log: PathBuf },
}

pub fn needs_replacement(
    version: &str,
    protocol: u32,
    build_id: &str,
    expected_build_id: &str,
) -> bool {
    version != crate::protocol::LOCAL_BUILD_VERSION
        || protocol != crate::protocol::WORKER_PROTOCOL_VERSION
        || build_id != expected_build_id
}

#[cfg(windows)]
fn prepare_worker_executable(
    home: &Path,
    controller_exe: &Path,
    build_id: &str,
) -> Result<PathBuf, WorkerBootstrapError> {
    let runtime_root = paneflow_home::serve_runtime_dir_in(home);
    let build_dir = runtime_root.join(build_id);
    std::fs::create_dir_all(&build_dir).map_err(|source| WorkerBootstrapError::Executable {
        path: build_dir.clone(),
        source,
    })?;
    let file_name = controller_exe
        .file_name()
        .ok_or_else(|| WorkerBootstrapError::Executable {
            path: controller_exe.to_path_buf(),
            source: io::Error::new(io::ErrorKind::InvalidInput, "executable has no file name"),
        })?;
    let target = build_dir.join(file_name);
    if target.is_file()
        && crate::protocol::executable_build_id(&target).is_ok_and(|held| held == build_id)
    {
        return Ok(target);
    }
    if target.exists() {
        std::fs::remove_file(&target).map_err(|source| WorkerBootstrapError::Executable {
            path: target.clone(),
            source,
        })?;
    }
    let temporary = build_dir.join(format!(
        ".{}.{}.tmp",
        file_name.to_string_lossy(),
        std::process::id()
    ));
    let _ = std::fs::remove_file(&temporary);
    std::fs::copy(controller_exe, &temporary).map_err(|source| {
        WorkerBootstrapError::Executable {
            path: temporary.clone(),
            source,
        }
    })?;
    match std::fs::rename(&temporary, &target) {
        Ok(()) => {}
        Err(error)
            if target.is_file()
                && crate::protocol::executable_build_id(&target)
                    .is_ok_and(|held| held == build_id) =>
        {
            let _ = std::fs::remove_file(&temporary);
            let _ = error;
        }
        Err(source) => {
            let _ = std::fs::remove_file(&temporary);
            return Err(WorkerBootstrapError::Executable {
                path: target,
                source,
            });
        }
    }
    for entry in std::fs::read_dir(&runtime_root)
        .into_iter()
        .flatten()
        .flatten()
    {
        if entry.file_name() != build_id {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
    Ok(target)
}

#[cfg(not(windows))]
fn prepare_worker_executable(
    _home: &Path,
    controller_exe: &Path,
    _build_id: &str,
) -> Result<PathBuf, WorkerBootstrapError> {
    Ok(controller_exe.to_path_buf())
}

pub fn ensure_worker_running(
    home: &Path,
    controller_exe: &Path,
) -> Result<WorkerAdoption, WorkerBootstrapError> {
    let endpoint = paneflow_home::serve_endpoint_path(home);
    let build_id = crate::protocol::executable_build_id(controller_exe).map_err(|source| {
        WorkerBootstrapError::Executable {
            path: controller_exe.to_path_buf(),
            source,
        }
    })?;
    let mut replaced = None;
    match probe(home, &endpoint) {
        Probe::Running(identity) => {
            if !needs_replacement(
                &identity.version,
                identity.protocol,
                &identity.build_id,
                &build_id,
            ) {
                return Ok(WorkerAdoption {
                    identity: *identity,
                    started: false,
                    replaced: None,
                });
            }
            log::info!(
                "paneflow-serve: replacing the worker {} ({}, protocol {}) with this build {} ({}, protocol {})",
                identity.version,
                identity.build_id,
                identity.protocol,
                crate::protocol::LOCAL_BUILD_VERSION,
                build_id,
                crate::protocol::WORKER_PROTOCOL_VERSION
            );
            let stopped = stop_worker(home, DRAIN_WAIT);
            if !stopped {
                return Err(WorkerBootstrapError::EndpointFaulted {
                    endpoint,
                    reason: format!(
                        "the worker {} still answers after a {DRAIN_WAIT:?} drain",
                        identity.version
                    ),
                });
            }
            replaced = Some(identity.version.clone());
        }
        Probe::Faulted(reason) => {
            return Err(WorkerBootstrapError::EndpointFaulted { endpoint, reason });
        }
        Probe::Unreachable(_) => {}
    }

    let log = paneflow_home::serve_log_path_in(home);
    if let Some(parent) = log.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let worker_executable = prepare_worker_executable(home, controller_exe, &build_id)?;
    let mut child = spawn_detached(
        &worker_executable,
        &[
            std::ffi::OsStr::new("serve"),
            std::ffi::OsStr::new("run"),
            std::ffi::OsStr::new("--home"),
            home.as_os_str(),
        ],
        &log,
    )?;
    let started = Instant::now();
    loop {
        match probe(home, &endpoint) {
            Probe::Running(identity) => {
                release_child(child);
                return Ok(WorkerAdoption {
                    identity: *identity,
                    started: true,
                    replaced,
                });
            }
            Probe::Faulted(reason) => {
                release_child(child);
                return Err(WorkerBootstrapError::EndpointFaulted { endpoint, reason });
            }
            Probe::Unreachable(_) => {}
        }
        if matches!(child.try_wait(), Ok(Some(_))) {
            return Err(WorkerBootstrapError::ExitedEarly { endpoint, log });
        }
        if started.elapsed() >= STARTUP_WAIT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(WorkerBootstrapError::StartupTimeout {
                endpoint,
                waited: STARTUP_WAIT,
                log,
            });
        }
        std::thread::sleep(STARTUP_POLL);
    }
}

pub fn stop_worker(home: &Path, drain: Duration) -> bool {
    let endpoint = paneflow_home::serve_endpoint_path(home);
    if let Ok(mut control) = HostControl::connect(&endpoint, CLIENT_NAME) {
        let _ = control.request(METHOD_WORKER_HELLO, json!({"client": CLIENT_NAME}));
        let _ = control.request(
            METHOD_WORKER_SHUTDOWN,
            json!({"drain_ms": drain.as_millis() as u64}),
        );
    }
    let deadline = Instant::now() + drain;
    loop {
        if matches!(probe(home, &endpoint), Probe::Unreachable(_)) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(STARTUP_POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_owner_lock_on_the_same_home_is_refused_while_the_first_lives() {
        let home = tempfile::tempdir().unwrap();
        let first = OwnerLock::acquire(home.path()).unwrap();
        assert!(first.path().ends_with("owner.lock"));
        assert!(
            matches!(OwnerLock::acquire(home.path()), Err(OwnerLockError::Held(path)) if path == first.path())
        );
        drop(first);
        OwnerLock::acquire(home.path()).unwrap();
    }

    #[test]
    fn only_a_different_build_or_protocol_replaces_the_worker_that_already_serves_the_home() {
        assert!(!needs_replacement(
            crate::protocol::LOCAL_BUILD_VERSION,
            crate::protocol::WORKER_PROTOCOL_VERSION,
            "build-a",
            "build-a"
        ));
        assert!(needs_replacement(
            "0.0.1",
            crate::protocol::WORKER_PROTOCOL_VERSION,
            "build-a",
            "build-a"
        ));
        assert!(needs_replacement(
            crate::protocol::LOCAL_BUILD_VERSION,
            crate::protocol::WORKER_PROTOCOL_VERSION + 1,
            "build-a",
            "build-a"
        ));
        assert!(needs_replacement(
            crate::protocol::LOCAL_BUILD_VERSION,
            crate::protocol::WORKER_PROTOCOL_VERSION,
            "build-a",
            "build-b"
        ));
        assert_eq!(
            DRAIN_WAIT,
            Duration::from_secs(5),
            "a replacement drains for five seconds before the new worker starts"
        );
    }

    #[test]
    fn an_unserved_endpoint_probes_as_unreachable_and_a_missing_record_reads_as_none() {
        let home = tempfile::tempdir().unwrap();
        let endpoint = paneflow_home::serve_endpoint_path(home.path());
        assert!(matches!(
            probe(home.path(), &endpoint),
            Probe::Unreachable(_)
        ));
        assert!(read_instance_record(home.path()).is_none());
    }
}
