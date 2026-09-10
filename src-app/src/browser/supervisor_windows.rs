use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::os::windows::io::AsRawHandle;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use interprocess::TryClone;
use interprocess::local_socket::{
    GenericFilePath, Listener, ListenerNonblockingMode, ListenerOptions, prelude::*,
};
use interprocess::os::windows::{
    local_socket::ListenerOptionsExt, security_descriptor::SecurityDescriptor,
};
use paneflow_browser_protocol::{
    Availability, BrowserError, CONTRACT_VERSION, Command as BrowserCommand, Envelope, Event,
    FrameAck, FrameMessage, OperationId, Owner, Reply, read_value, write_message,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use win32job::{ExtendedLimitInfo, Job};

pub use super::RUNTIME_ENV;
use super::windows::D3dImporter;
pub const OWNER_ENV: &str = "PANEFLOW_BROWSER_OWNER";
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const EVENT_QUEUE_CAPACITY: usize = 64;
const EMBEDDED_MANIFEST: &str = include_str!("../../../native/browser/manifest.toml");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeCheck {
    Manifest,
    Qualification([u8; 32]),
}

impl RuntimeCheck {
    pub fn configured() -> Result<Self, String> {
        let Some(value) = std::env::var_os("PANEFLOW_BROWSER_QUALIFICATION_SHA256") else {
            return Ok(Self::Manifest);
        };
        let value = value.to_str().ok_or("qualification digest is not UTF-8")?;
        Self::qualification(value)
    }

    pub fn dock_session() -> Result<Self, String> {
        if paneflow_config::loader::qualification_root().is_none() {
            return Ok(Self::Manifest);
        }
        Self::configured()
    }

    fn qualification(value: &str) -> Result<Self, String> {
        if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("qualification runtime requires an exact SHA-256 digest".into());
        }
        let mut digest = [0; 32];
        for (index, byte) in digest.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
                .map_err(|error| error.to_string())?;
        }
        Ok(Self::Qualification(digest))
    }

    fn runtime_digest(self) -> Result<Option<String>, String> {
        match self {
            Self::Manifest => manifest_runtime_digest().map(Some),
            Self::Qualification(digest) => Ok(Some(
                digest.iter().map(|byte| format!("{byte:02x}")).collect(),
            )),
        }
    }
}

#[derive(Clone, Debug)]
pub struct HostConfig {
    pub tracing: bool,
    pub host_binary: PathBuf,
    pub runtime_root: PathBuf,
    pub stage_root: PathBuf,
    pub profile_dir: PathBuf,
    pub origin: String,
    pub owner: Owner,
    pub frames: bool,
    pub check: RuntimeCheck,
}

impl HostConfig {
    pub fn profile_dir(&self) -> PathBuf {
        self.profile_dir.clone()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostState {
    Inactive,
    Starting,
    Ready,
    Stopping,
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct HostInfo {
    pub pid: u32,
    pub availability: Availability,
    pub contract_version: u32,
    pub presentation: bool,
    pub initialized: Value,
}

#[derive(Debug)]
pub enum HostEvent {
    Ready(HostInfo),
    Reply(Reply),
    Native(Value),
    Frame(FrameMessage, Vec<u64>),
    Lost(String),
    Stopped,
}

enum Outbound {
    Control(Envelope),
    Ack(FrameAck),
    Close,
}

#[derive(Clone)]
pub struct FrameAcknowledger {
    outbound: SyncSender<Outbound>,
    connection: Arc<()>,
}

impl FrameAcknowledger {
    pub fn ack(&self, ack: FrameAck) -> Result<(), BrowserError> {
        self.outbound
            .try_send(Outbound::Ack(ack))
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => BrowserError::Busy,
                mpsc::TrySendError::Disconnected(_) => BrowserError::Unavailable,
            })
    }
}

struct Running {
    pid: u32,
    outbound: SyncSender<Outbound>,
    frame_connection: Arc<()>,
    exited: Arc<(Mutex<bool>, Condvar)>,
    stop_requested: Arc<AtomicBool>,
    job: Arc<Mutex<Option<Job>>>,
}

struct Inner {
    state: Mutex<HostState>,
    running: Mutex<Option<Running>>,
    failure: Mutex<Option<String>>,
    events: smol::channel::Sender<HostEvent>,
    event_delivery_failed: Mutex<bool>,
    operations: AtomicU64,
}

#[derive(Clone)]
pub struct HostSupervisor {
    inner: Arc<Inner>,
}

pub fn manifest_digest() -> String {
    hex_digest(EMBEDDED_MANIFEST.as_bytes())
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn file_digest(path: &Path) -> io::Result<String> {
    if !fs::metadata(path)?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a regular file",
        ));
    }
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 16];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn runtime_digest(root: &Path) -> io::Result<String> {
    runtime_digest_excluding(root, &[])
}

fn runtime_digest_excluding(root: &Path, excluded: &[&str]) -> io::Result<String> {
    fn collect(root: &Path, current: &Path, paths: &mut Vec<PathBuf>) -> io::Result<()> {
        for entry in fs::read_dir(current)? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                collect(root, &path, paths)?;
            } else if file_type.is_file()
                && (path
                    .strip_prefix(root)
                    .map(|relative| relative.components().count() != 1)
                    .unwrap_or(true)
                    || !matches!(
                        entry.file_name().to_str(),
                        Some("verified-manifest.sha256" | "elf-audit.json" | "windows-audit.json")
                    ))
            {
                paths.push(path.strip_prefix(root).unwrap_or(&path).to_path_buf());
            }
        }
        Ok(())
    }

    let mut paths = Vec::new();
    collect(root, root, &mut paths)?;
    paths.retain(|path| {
        let relative = path.to_string_lossy().replace('\\', "/");
        !excluded.iter().any(|candidate| relative == *candidate)
    });
    paths.sort_by_key(|path| path.to_string_lossy().replace('\\', "/"));
    let mut hasher = Sha256::new();
    for relative in paths {
        let path = root.join(&relative);
        hasher.update(relative.to_string_lossy().replace('\\', "/"));
        hasher.update([0]);
        hasher.update(path.metadata()?.len().to_string());
        hasher.update([0]);
        hasher.update(file_digest(&path)?);
        hasher.update(*b"\n");
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn manifest_runtime_digest() -> Result<String, String> {
    let manifest: toml::Value =
        toml::from_str(EMBEDDED_MANIFEST).map_err(|error| format!("embedded manifest: {error}"))?;
    let target = format!("{}-pc-windows-msvc", std::env::consts::ARCH);
    manifest
        .get("targets")
        .and_then(|targets| targets.get(&target))
        .and_then(|target| target.get("runtime_sha256"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("embedded manifest has no runtime digest for {target}"))
}

fn manifest_abi_hash() -> Result<String, String> {
    let manifest: toml::Value =
        toml::from_str(EMBEDDED_MANIFEST).map_err(|error| format!("embedded manifest: {error}"))?;
    let target = format!("{}-pc-windows-msvc", std::env::consts::ARCH);
    manifest
        .get("targets")
        .and_then(|targets| targets.get(&target))
        .and_then(|target| target.get("abi_hash"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("embedded manifest has no ABI hash for {target}"))
}

fn manifest_files() -> Result<BTreeMap<PathBuf, String>, String> {
    let manifest: toml::Value =
        toml::from_str(EMBEDDED_MANIFEST).map_err(|error| format!("embedded manifest: {error}"))?;
    let target = format!("{}-pc-windows-msvc", std::env::consts::ARCH);
    let files = manifest
        .get("targets")
        .and_then(|targets| targets.get(&target))
        .and_then(|target| target.get("files"))
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("embedded manifest has no file table for {target}"))?;
    files
        .iter()
        .map(|(name, value)| {
            let path = PathBuf::from(name);
            let digest = value
                .as_str()
                .ok_or_else(|| format!("invalid digest for {name}"))?;
            if path.as_os_str().is_empty()
                || !path
                    .components()
                    .all(|part| matches!(part, Component::Normal(_)))
                || digest.len() != 64
                || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(format!("invalid embedded manifest file {name}"));
            }
            Ok((path, digest.to_string()))
        })
        .collect()
}

fn verify_files(root: &Path, files: &BTreeMap<PathBuf, String>) -> Result<(), String> {
    for (name, expected) in files {
        let actual = file_digest(&root.join(name))
            .map_err(|error| format!("runtime file {}: {error}", name.display()))?;
        if &actual != expected {
            return Err(format!(
                "runtime file {} does not match its manifest digest",
                name.display()
            ));
        }
    }
    Ok(())
}

pub fn verify_runtime(root: &Path, check: RuntimeCheck) -> Result<(), String> {
    let stamp = fs::read_to_string(root.join("verified-manifest.sha256"))
        .map_err(|error| format!("runtime verification stamp: {error}"))?;
    if stamp.trim() != manifest_digest() {
        return Err("runtime was verified against a different browser manifest".to_string());
    }
    for name in [
        "Release/libcef.dll",
        "Release/chrome_elf.dll",
        "Release/libEGL.dll",
        "Release/libGLESv2.dll",
    ] {
        if !root.join(name).is_file() {
            return Err(format!("runtime is missing {name}"));
        }
    }
    if matches!(check, RuntimeCheck::Manifest) {
        verify_files(root, &manifest_files()?)?;
    }
    let metadata: Value = serde_json::from_str(
        &fs::read_to_string(root.join("cef_version.json"))
            .map_err(|error| format!("runtime version metadata: {error}"))?,
    )
    .map_err(|error| format!("runtime version metadata: {error}"))?;
    if metadata.get("abi_hash").and_then(Value::as_str) != Some(manifest_abi_hash()?.as_str()) {
        return Err("runtime ABI hash does not match the Windows browser manifest".to_string());
    }
    if let Some(expected) = check.runtime_digest()? {
        let actual = runtime_digest(root).map_err(|error| format!("runtime tree: {error}"))?;
        if actual != expected {
            return Err("runtime tree does not match the Windows browser manifest".to_string());
        }
    }
    Ok(())
}

fn verify_host_binary(path: &Path) -> Result<String, String> {
    if !path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("exe"))
    {
        return Err("browser host must be a bootstrap .exe".to_string());
    }
    let metadata = fs::metadata(path).map_err(|error| format!("host binary: {error}"))?;
    if !metadata.is_file() {
        return Err("host binary is not a regular file".to_string());
    }
    super::install::verify_windows_pe(path, "host binary", false)?;
    file_digest(path).map_err(|error| format!("host binary digest: {error}"))
}

fn verify_client_binary(path: &Path) -> Result<String, String> {
    if !path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("dll"))
    {
        return Err("browser client must be a .dll".to_string());
    }
    let metadata = fs::metadata(path).map_err(|error| format!("client binary: {error}"))?;
    if !metadata.is_file() {
        return Err("client binary is not a regular file".to_string());
    }
    super::install::verify_windows_pe(path, "client binary", true)?;
    file_digest(path).map_err(|error| format!("client binary digest: {error}"))
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_tree(&source, &target)?;
        } else if file_type.is_file() {
            fs::copy(&source, &target)?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported runtime entry {}", source.display()),
            ));
        }
    }
    Ok(())
}

fn verify_staged_runtime(
    directory: &Path,
    host_digest: &str,
    client_digest: &str,
    check: RuntimeCheck,
) -> Result<(), String> {
    let stamp = fs::read_to_string(directory.join(".complete"))
        .map_err(|error| format!("staging stamp: {error}"))?;
    if stamp != format!("{host_digest}\n{client_digest}") {
        return Err("staging stamp differs from the host binary digest".to_string());
    }
    let bin = directory.join("bin");
    if let Some(expected_runtime) = check.runtime_digest()? {
        if matches!(check, RuntimeCheck::Manifest) {
            verify_files(&bin, &manifest_files()?)?;
        }
        let actual_runtime = runtime_digest_excluding(
            &bin,
            &[
                "Release/paneflow-browser-host.exe",
                "Release/paneflow-browser-host.dll",
            ],
        )
        .map_err(|error| format!("staged runtime tree: {error}"))?;
        if actual_runtime != expected_runtime {
            return Err(
                "staged runtime tree differs from the Windows browser manifest".to_string(),
            );
        }
    } else {
        for name in [
            "Release/libcef.dll",
            "Release/chrome_elf.dll",
            "Release/libEGL.dll",
            "Release/libGLESv2.dll",
        ] {
            if !bin.join(name).is_file() {
                return Err(format!("staged runtime is missing {name}"));
            }
        }
    }
    let host = bin.join("Release/paneflow-browser-host.exe");
    let actual = verify_host_binary(&host)?;
    if actual != host_digest {
        return Err("staged bootstrap digest differs from the source".to_string());
    }
    let client = bin.join("Release/paneflow-browser-host.dll");
    let actual = verify_client_binary(&client)?;
    if actual != client_digest {
        return Err("staged client digest differs from the source".to_string());
    }
    Ok(())
}

fn stage(config: &HostConfig, host_digest: &str, client_digest: &str) -> Result<PathBuf, String> {
    let key = hex_digest(
        format!(
            "{}\n{}\n{}\n{}\n{:?}",
            config.runtime_root.display(),
            host_digest,
            client_digest,
            manifest_digest(),
            config.check
        )
        .as_bytes(),
    );
    let directory = config.stage_root.join(format!("host-{}", &key[..16]));
    if directory.join(".complete").is_file() {
        verify_staged_runtime(&directory, host_digest, client_digest, config.check)?;
        return Ok(directory);
    }
    let scratch = config
        .stage_root
        .join(format!(".host-{}-{}", &key[..16], std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    let result = (|| {
        fs::create_dir_all(&scratch)?;
        let bin = scratch.join("bin");
        copy_tree(&config.runtime_root, &bin)?;
        let release = bin.join("Release");
        fs::copy(
            &config.host_binary,
            release.join("paneflow-browser-host.exe"),
        )?;
        fs::copy(
            super::install::client_binary(&config.host_binary),
            release.join("paneflow-browser-host.dll"),
        )?;
        fs::write(
            scratch.join(".complete"),
            format!("{host_digest}\n{client_digest}"),
        )?;
        match fs::rename(&scratch, &directory) {
            Ok(()) => Ok(()),
            Err(_) if directory.join(".complete").is_file() => {
                let _ = fs::remove_dir_all(&scratch);
                Ok(())
            }
            Err(error) => Err(error),
        }
    })();
    match result {
        Ok(()) => {
            verify_staged_runtime(&directory, host_digest, client_digest, config.check)?;
            Ok(directory)
        }
        Err(error) => {
            let _ = fs::remove_dir_all(&scratch);
            Err(format!("staging the browser host: {error}"))
        }
    }
}

fn security_descriptor() -> io::Result<SecurityDescriptor> {
    let sddl = widestring::U16CString::from_str("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;OW)")
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    SecurityDescriptor::deserialize(sddl.as_ucstr())
}

fn pipe_path() -> PathBuf {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    PathBuf::from(format!(
        r"\\.\pipe\paneflow-browser-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}

fn listener(path: &Path) -> Result<Listener, String> {
    let name = path
        .to_fs_name::<GenericFilePath>()
        .map_err(|error| format!("browser named-pipe name: {error}"))?;
    let descriptor =
        security_descriptor().map_err(|error| format!("browser named-pipe ACL: {error}"))?;
    ListenerOptions::new()
        .name(name)
        .security_descriptor(descriptor)
        .nonblocking(ListenerNonblockingMode::Accept)
        .create_sync()
        .map_err(|error| format!("browser named-pipe listener: {error}"))
}

fn accept_with_timeout(listener: &Listener) -> Result<interprocess::local_socket::Stream, String> {
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    loop {
        match listener.accept() {
            Ok(stream) => return Ok(stream),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err("browser bootstrap named-pipe handshake timed out".to_string());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(format!("browser bootstrap named-pipe handshake: {error}")),
        }
    }
}

impl Inner {
    fn state(&self) -> HostState {
        self.state
            .lock()
            .map(|state| state.clone())
            .unwrap_or_else(|_| HostState::Failed("supervisor state poisoned".to_string()))
    }

    fn set_state(&self, state: HostState) {
        if let Ok(mut current) = self.state.lock() {
            *current = state;
        }
    }

    fn emit(&self, event: HostEvent) -> bool {
        match self.events.try_send(event) {
            Ok(()) => return true,
            Err(error) => {
                if let HostEvent::Frame(_, handles) = error.into_inner() {
                    D3dImporter::close_handles(handles);
                }
            }
        }
        if let Ok(mut failed) = self.event_delivery_failed.lock() {
            *failed = true;
        }
        false
    }

    fn abort(&self, reason: String) {
        if let Ok(mut failure) = self.failure.lock() {
            *failure = Some(reason.clone());
        }
        self.set_state(HostState::Failed(reason.clone()));
        let _ = self.emit(HostEvent::Lost(reason));
        self.kill_running();
    }

    fn kill_running(&self) {
        let running = self.running.lock().ok().and_then(|mut value| value.take());
        if let Some(running) = running {
            running.stop_requested.store(true, Ordering::Release);
            if let Ok(mut job) = running.job.lock() {
                job.take();
            }
        }
    }
}

impl HostSupervisor {
    pub fn channel() -> (Self, smol::channel::Receiver<HostEvent>) {
        let (events, receiver) = smol::channel::bounded(EVENT_QUEUE_CAPACITY + 1);
        (
            Self {
                inner: Arc::new(Inner {
                    state: Mutex::new(HostState::Inactive),
                    running: Mutex::new(None),
                    failure: Mutex::new(None),
                    events,
                    event_delivery_failed: Mutex::new(false),
                    operations: AtomicU64::new(1),
                }),
            },
            receiver,
        )
    }

    pub fn state(&self) -> HostState {
        self.inner.state()
    }

    pub fn activate(&self, config: HostConfig) -> HostState {
        {
            let failed = self
                .inner
                .event_delivery_failed
                .lock()
                .map(|failed| *failed)
                .unwrap_or(true);
            if failed {
                return self.state();
            }
            let Ok(mut state) = self.inner.state.lock() else {
                return HostState::Failed("supervisor state poisoned".to_string());
            };
            if *state != HostState::Inactive {
                return state.clone();
            }
            *state = HostState::Starting;
        }
        if let Ok(mut failure) = self.inner.failure.lock() {
            *failure = None;
        }
        let inner = self.inner.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("browser-host-start".into())
            .spawn(move || match start(&inner, &config) {
                Ok(info) => {
                    if inner.state() == HostState::Starting {
                        inner.set_state(HostState::Ready);
                        let _ = inner.emit(HostEvent::Ready(info));
                    }
                }
                Err(reason) => {
                    inner.kill_running();
                    inner.set_state(HostState::Failed(reason.clone()));
                    let _ = inner.emit(HostEvent::Lost(reason));
                }
            })
        {
            let reason = format!("host start thread: {error}");
            self.inner.set_state(HostState::Failed(reason.clone()));
            let _ = self.inner.emit(HostEvent::Lost(reason));
        }
        HostState::Starting
    }

    pub fn send(&self, command: BrowserCommand) -> Result<OperationId, BrowserError> {
        if self.state() != HostState::Ready {
            return Err(BrowserError::Unavailable);
        }
        let running = self
            .inner
            .running
            .lock()
            .map_err(|_| BrowserError::Unavailable)?;
        let Some(running) = running.as_ref() else {
            return Err(BrowserError::Unavailable);
        };
        let operation = OperationId::try_from(format!(
            "op-{}",
            self.inner.operations.fetch_add(1, Ordering::Relaxed)
        ))
        .map_err(|_| BrowserError::LimitReached)?;
        running
            .outbound
            .try_send(Outbound::Control(Envelope {
                version: CONTRACT_VERSION,
                operation: operation.clone(),
                command,
            }))
            .map_err(|_| BrowserError::Busy)?;
        Ok(operation)
    }

    pub fn frame_acknowledger(&self) -> Result<FrameAcknowledger, BrowserError> {
        let running = self
            .inner
            .running
            .lock()
            .map_err(|_| BrowserError::Unavailable)?;
        let Some(running) = running.as_ref() else {
            return Err(BrowserError::Unavailable);
        };
        Ok(FrameAcknowledger {
            outbound: running.outbound.clone(),
            connection: running.frame_connection.clone(),
        })
    }

    pub fn has_frame_connection(&self, acknowledger: &FrameAcknowledger) -> bool {
        self.inner.running.lock().is_ok_and(|running| {
            running.as_ref().is_some_and(|running| {
                Arc::ptr_eq(&running.frame_connection, &acknowledger.connection)
            })
        })
    }

    pub fn terminate(&self) -> bool {
        let Some(running) = self
            .inner
            .running
            .lock()
            .ok()
            .and_then(|mut value| value.take())
        else {
            return false;
        };
        running.stop_requested.store(true, Ordering::Release);
        if let Ok(mut job) = running.job.lock() {
            job.take().is_some()
        } else {
            false
        }
    }

    pub fn shutdown(&self, grace: Duration) {
        let Some(running) = self
            .inner
            .running
            .lock()
            .ok()
            .and_then(|mut value| value.take())
        else {
            self.inner.set_state(HostState::Inactive);
            return;
        };
        running.stop_requested.store(true, Ordering::Release);
        self.inner.set_state(HostState::Stopping);
        let _ = running.outbound.try_send(Outbound::Close);
        let deadline = Instant::now() + grace;
        if let Ok(mut exited) = running.exited.0.lock() {
            while !*exited {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                match running.exited.1.wait_timeout(exited, deadline - now) {
                    Ok((guard, _)) => exited = guard,
                    Err(_) => break,
                }
            }
        }
        if let Ok(mut job) = running.job.lock() {
            job.take();
        }
        self.inner.set_state(HostState::Inactive);
    }
}

fn start(inner: &Arc<Inner>, config: &HostConfig) -> Result<HostInfo, String> {
    verify_runtime(&config.runtime_root, config.check)?;
    let host_digest = verify_host_binary(&config.host_binary)?;
    let client_digest = verify_client_binary(&super::install::client_binary(&config.host_binary))?;
    let staged = stage(config, &host_digest, &client_digest)?;
    let bin = staged.join("bin");
    let profile = config.profile_dir();
    super::profile::prepare_profile_dir(&profile).map_err(|error| error.message())?;
    let pipe = pipe_path();
    let listener = listener(&pipe)?;
    let stderr = File::create(profile.join("host.stderr"))
        .map_err(|error| format!("host stderr log: {error}"))?;
    let mut path = vec![bin.join("Release")];
    if let Some(current) = std::env::var_os("PATH") {
        path.extend(std::env::split_paths(&current));
    }
    let path = std::env::join_paths(path).map_err(|error| format!("browser DLL path: {error}"))?;
    let mut command = Command::new(bin.join("Release/paneflow-browser-host.exe"));
    command
        .current_dir(bin.join("Release"))
        .env("PATH", path)
        .env(RUNTIME_ENV, &bin)
        .env("PANEFLOW_CEF_PROFILE", &profile)
        .env("PANEFLOW_CEF_ORIGIN", &config.origin)
        .env(
            OWNER_ENV,
            format!(
                "{}/{}",
                serde_json::to_value(&config.owner.workspace)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_string))
                    .unwrap_or_default(),
                serde_json::to_value(&config.owner.session)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_string))
                    .unwrap_or_default()
            ),
        )
        .env("PANEFLOW_BROWSER_CONTROL_PIPE", &pipe)
        .env(
            "PANEFLOW_BROWSER_TRACE",
            if config.tracing { "1" } else { "0" },
        )
        .env("PANEFLOW_CEF_SANDBOX", "required")
        .env(
            "PANEFLOW_BROWSER_FRAMES",
            if config.frames { "1" } else { "0" },
        )
        .env(
            "PANEFLOW_BROWSER_CLIENT_PID",
            std::process::id().to_string(),
        )
        .env_remove("PANEFLOW_BROWSER_SUBPROCESS_PATH")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr));
    let mut child = command
        .spawn()
        .map_err(|error| format!("spawning the browser bootstrap: {error}"))?;
    let pid = child.id();
    let mut limits = ExtendedLimitInfo::default();
    limits.limit_kill_on_job_close();
    let job = match Job::create_with_limit_info(&limits) {
        Ok(job) => job,
        Err(error) => {
            let _ = child.kill();
            return Err(format!("creating browser Job Object: {error}"));
        }
    };
    if let Err(error) = job.assign_process(child.as_raw_handle() as isize) {
        let _ = child.kill();
        return Err(format!(
            "assigning browser bootstrap to Job Object: {error}"
        ));
    }
    let job = Arc::new(Mutex::new(Some(job)));
    let stream = accept_with_timeout(&listener)?;
    let writer_stream = stream
        .try_clone()
        .map_err(|error| format!("browser named-pipe clone: {error}"))?;
    let (outbound, outbound_receiver) = mpsc::sync_channel::<Outbound>(256);
    let frame_connection = Arc::new(());
    let exited = Arc::new((Mutex::new(false), Condvar::new()));
    let stop_requested = Arc::new(AtomicBool::new(false));
    if let Ok(mut running) = inner.running.lock() {
        *running = Some(Running {
            pid,
            outbound: outbound.clone(),
            frame_connection: frame_connection.clone(),
            exited: exited.clone(),
            stop_requested: stop_requested.clone(),
            job: job.clone(),
        });
    }
    let writer_inner = inner.clone();
    std::thread::Builder::new()
        .name("browser-host-writer".into())
        .spawn(move || {
            let mut stream = writer_stream;
            while let Ok(message) = outbound_receiver.recv() {
                let result = match message {
                    Outbound::Control(envelope) => write_message(&mut stream, &envelope),
                    Outbound::Ack(ack) => write_message(&mut stream, &json!({ "frame": ack })),
                    Outbound::Close => return,
                };
                if let Err(error) = result {
                    writer_inner.abort(format!("host control write failed: {error}"));
                    return;
                }
            }
        })
        .map_err(|error| format!("host writer thread: {error}"))?;
    let (handshake, handshake_receiver) = mpsc::sync_channel::<Value>(64);
    let reader_inner = inner.clone();
    std::thread::Builder::new()
        .name("browser-host-reader".into())
        .spawn(move || {
            let mut stream = BufReader::new(stream);
            let mut waiting = Some(handshake);
            loop {
                match read_value(&mut stream) {
                    Ok(Some(value)) => {
                        if let Some(sender) = &waiting {
                            let initialized =
                                value.get("native") == Some(&Value::from("initialized"));
                            if sender.send(value).is_err() {
                                reader_inner.abort("host handshake listener vanished".to_string());
                                return;
                            }
                            if initialized {
                                waiting = None;
                            }
                            continue;
                        }
                        if let Some(reply) = value.get("protocol") {
                            match serde_json::from_value::<Reply>(reply.clone()) {
                                Ok(reply) => {
                                    if !reader_inner.emit(HostEvent::Reply(reply)) {
                                        return;
                                    }
                                }
                                Err(error) => {
                                    reader_inner.abort(format!(
                                        "host reply violates the contract: {error}"
                                    ));
                                    return;
                                }
                            }
                        } else if let Some(frame) = value.get("frame") {
                            let message = serde_json::from_value::<FrameMessage>(frame.clone())
                                .map_err(|error| {
                                    format!("host frame violates the contract: {error}")
                                });
                            let Ok(message) = message else {
                                reader_inner.abort(message.unwrap_err());
                                return;
                            };
                            if !message.is_valid() {
                                reader_inner.abort(
                                    "host frame violates the contract: invalid payload".to_string(),
                                );
                                return;
                            }
                            let handles = match &message {
                                FrameMessage::PoolCreated { shared_handles, .. }
                                    if shared_handles.len() == 3 =>
                                {
                                    shared_handles.clone()
                                }
                                FrameMessage::PoolCreated { .. } => {
                                    reader_inner.abort(
                                        "Windows frame pool did not contain three shared handles"
                                            .to_string(),
                                    );
                                    return;
                                }
                                _ => Vec::new(),
                            };
                            if !reader_inner.emit(HostEvent::Frame(message, handles)) {
                                return;
                            }
                        } else if value.get("native").is_some() {
                            if !reader_inner.emit(HostEvent::Native(value)) {
                                return;
                            }
                        } else {
                            reader_inner
                                .abort("host event without protocol or native payload".to_string());
                            return;
                        }
                    }
                    Ok(None) => return,
                    Err(error) => {
                        reader_inner.abort(format!(
                            "host event stream violates the contract: {error:?}"
                        ));
                        return;
                    }
                }
            }
        })
        .map_err(|error| format!("host reader thread: {error}"))?;
    let watcher_inner = inner.clone();
    std::thread::Builder::new()
        .name("browser-host-watch".into())
        .spawn(move || watch(watcher_inner, child, exited, stop_requested, job))
        .map_err(|error| format!("host watch thread: {error}"))?;
    let hello = Envelope {
        version: CONTRACT_VERSION,
        operation: OperationId::try_from("hello".to_string()).map_err(|error| error.to_string())?,
        command: BrowserCommand::Capabilities,
    };
    outbound
        .try_send(Outbound::Control(hello))
        .map_err(|_| "host control queue is full before the handshake".to_string())?;
    let value =
        handshake_receiver
            .recv_timeout(HANDSHAKE_TIMEOUT)
            .map_err(|error| match error {
                RecvTimeoutError::Timeout => "host handshake timed out".to_string(),
                RecvTimeoutError::Disconnected => "host ended before the handshake".to_string(),
            })?;
    match value.get("native").and_then(Value::as_str) {
        Some("initialized") => {
            let reply: Reply =
                serde_json::from_value(value.get("protocol").cloned().unwrap_or(Value::Null))
                    .map_err(|error| format!("host capabilities reply: {error}"))?;
            let Ok(Event::Capabilities {
                availability,
                contract_version,
                ..
            }) = reply.result
            else {
                return Err("host refused the capability negotiation".to_string());
            };
            if contract_version != CONTRACT_VERSION {
                return Err(format!(
                    "host contract version {contract_version} differs from {CONTRACT_VERSION}"
                ));
            }
            if availability == Availability::Absent {
                return Err("host reports no browser availability".to_string());
            }
            let reported = value.get("pid").and_then(Value::as_u64).unwrap_or(0);
            if reported != u64::from(pid) {
                return Err("host reported a foreign process id".to_string());
            }
            Ok(HostInfo {
                pid: reported as u32,
                availability,
                contract_version,
                presentation: value
                    .get("presentation")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                initialized: value,
            })
        }
        _ => Err("host did not initialize the Windows bootstrap contract".to_string()),
    }
}

fn watch(
    inner: Arc<Inner>,
    mut child: Child,
    exited: Arc<(Mutex<bool>, Condvar)>,
    stop_requested: Arc<AtomicBool>,
    job: Arc<Mutex<Option<Job>>>,
) {
    let status = child.wait();
    if let Ok(mut flag) = exited.0.lock() {
        *flag = true;
    }
    exited.1.notify_all();
    if let Ok(mut running) = inner.running.lock()
        && running
            .as_ref()
            .is_some_and(|running| running.pid == child.id())
    {
        *running = None;
    }
    if let Ok(mut owned_job) = job.lock() {
        owned_job.take();
    }
    if matches!(inner.state(), HostState::Failed(_)) {
        return;
    }
    if stop_requested.load(Ordering::Acquire) {
        inner.set_state(HostState::Inactive);
        let _ = inner.emit(HostEvent::Stopped);
        return;
    }
    let recorded = inner
        .failure
        .lock()
        .ok()
        .and_then(|failure| failure.clone());
    let reason = recorded.unwrap_or_else(|| match status {
        Ok(status) => format!("host exited: {status}"),
        Err(error) => format!("host wait failed: {error}"),
    });
    inner.set_state(HostState::Failed(reason.clone()));
    let _ = inner.emit(HostEvent::Lost(reason));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualification_requires_an_exact_digest_and_preserves_it() {
        for value in ["", "true", "stamp-only", &"0".repeat(63), &"g".repeat(64)] {
            assert!(RuntimeCheck::qualification(value).is_err());
        }
        let digest = "ab".repeat(32);
        assert_eq!(
            RuntimeCheck::qualification(&digest)
                .unwrap()
                .runtime_digest()
                .unwrap(),
            Some(digest)
        );
    }

    #[test]
    fn a_dock_page_outside_an_isolated_qualification_keeps_the_manifest_check() {
        assert!(paneflow_config::loader::qualification_root().is_none());
        assert_eq!(RuntimeCheck::dock_session(), Ok(RuntimeCheck::Manifest));
    }

    #[test]
    fn manifest_digest_is_stable_for_the_embedded_contract() {
        assert_eq!(manifest_digest().len(), 64);
        assert!(
            manifest_digest()
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        );
    }

    #[test]
    fn browser_pipe_acl_is_owner_scoped() {
        assert!(security_descriptor().is_ok());
    }

    #[test]
    fn windows_runtime_contract_has_a_pinned_file_table() {
        let files = manifest_files().unwrap();
        assert!(files.contains_key(Path::new("Release/libcef.dll")));
        assert!(files.contains_key(Path::new("cef_version.json")));
    }

    #[test]
    fn runtime_tree_digest_excludes_verification_receipts_but_detects_payload_changes() {
        let root = std::env::temp_dir().join(format!(
            "paneflow-browser-runtime-digest-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|moment| moment.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("Release")).unwrap();
        fs::write(root.join("Release/libcef.dll"), b"payload").unwrap();
        let first = runtime_digest(&root).unwrap();
        fs::write(root.join("verified-manifest.sha256"), b"manifest").unwrap();
        fs::write(root.join("windows-audit.json"), b"audit").unwrap();
        assert_eq!(runtime_digest(&root).unwrap(), first);
        fs::write(root.join("Release/libcef.dll"), b"changed").unwrap();
        assert_ne!(runtime_digest(&root).unwrap(), first);
        let _ = fs::remove_dir_all(root);
    }
}
