use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use paneflow_browser_protocol::{
    Availability, BrowserError, CONTRACT_VERSION, Command as BrowserCommand, Envelope, Event,
    FRAME_CHANNEL_ENV, FrameAck, FrameChannel, FrameMessage, OperationId, Owner, Reply, read_value,
    write_message,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub use super::{HOST_ENV, RUNTIME_ENV};
pub const OWNER_ENV: &str = "PANEFLOW_BROWSER_OWNER";
pub const RENDER_NODE_ENV: &str = "PANEFLOW_BROWSER_RENDER_NODE";
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const HOST_STDERR_LIMIT: u64 = 4 * 1024 * 1024;
const EMBEDDED_MANIFEST: &str = include_str!("../../../native/browser/manifest.toml");
const EVENT_QUEUE_CAPACITY: usize = 64;
const REQUIRED_FILES: [&str; 3] = [
    "Release/libcef.so",
    "Resources/icudtl.dat",
    "Resources/locales/en-US.pak",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ozone {
    Wayland,
    X11,
}

impl Ozone {
    fn switch(self) -> &'static str {
        match self {
            Self::Wayland => "wayland",
            Self::X11 => "x11",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeCheck {
    Manifest,
    #[cfg(test)]
    StampOnly,
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
    pub ozone: Ozone,
    pub gpu: Option<(u32, u32)>,
    pub render_node: Option<PathBuf>,
    pub frames: bool,
    pub check: RuntimeCheck,
}

impl HostConfig {
    pub fn profile_dir(&self) -> PathBuf {
        self.profile_dir.clone()
    }

    pub fn witness_profile_dir(stage_root: &Path, origin: &str) -> PathBuf {
        stage_root
            .join("profiles")
            .join(&hex_digest(origin.as_bytes())[..16])
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
    Frame(FrameMessage, Vec<OwnedFd>),
    Lost(String),
    Stopped,
}

enum Outbound {
    Control(Envelope),
    Ack(FrameAck),
    Close,
}

struct Running {
    pid: u32,
    pgid: i32,
    outbound: SyncSender<Outbound>,
    frame_connection: Arc<()>,
    exited: Arc<(Mutex<bool>, Condvar)>,
    stop_requested: Arc<AtomicBool>,
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

struct Inner {
    state: Mutex<HostState>,
    startup_finished: Mutex<bool>,
    startup_changed: Condvar,
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

fn manifest_files() -> Result<BTreeMap<PathBuf, String>, String> {
    let manifest: toml::Value =
        toml::from_str(EMBEDDED_MANIFEST).map_err(|error| format!("embedded manifest: {error}"))?;
    let target = format!("{}-unknown-linux-gnu", std::env::consts::ARCH);
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
    for name in REQUIRED_FILES {
        if !root.join(name).is_file() {
            return Err(format!("runtime is missing {name}"));
        }
    }
    if matches!(check, RuntimeCheck::Manifest) {
        verify_files(root, &manifest_files()?)?;
    }
    Ok(())
}

fn verify_host_binary(path: &Path) -> Result<String, String> {
    let metadata = fs::metadata(path).map_err(|error| format!("host binary: {error}"))?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err("host binary is not an executable file".to_string());
    }
    file_digest(path).map_err(|error| format!("host binary digest: {error}"))
}

fn link_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            link_tree(&source, &target)?;
        } else if fs::hard_link(&source, &target).is_err() {
            std::os::unix::fs::symlink(&source, &target)?;
        }
    }
    Ok(())
}

fn staged_files(config: &HostConfig) -> Result<BTreeMap<PathBuf, String>, String> {
    let files = match config.check {
        RuntimeCheck::Manifest => manifest_files()?,
        #[cfg(test)]
        RuntimeCheck::StampOnly => REQUIRED_FILES
            .iter()
            .map(|name| {
                file_digest(&config.runtime_root.join(name))
                    .map(|digest| (PathBuf::from(name), digest))
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<_, _>>()?,
    };
    let mut staged = BTreeMap::new();
    for (name, digest) in files {
        let relative = name
            .strip_prefix("Release")
            .or_else(|_| name.strip_prefix("Resources"));
        if let Ok(relative) = relative
            && staged.insert(relative.to_path_buf(), digest).is_some()
        {
            return Err(format!(
                "runtime staging filename collision: {}",
                relative.display()
            ));
        }
    }
    Ok(staged)
}

fn reject_unlisted_files(
    root: &Path,
    directory: &Path,
    files: &BTreeMap<PathBuf, String>,
) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let relative = path.strip_prefix(root).map_err(|error| error.to_string())?;
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            if !files.keys().any(|name| name.starts_with(relative)) {
                return Err(format!(
                    "unlisted runtime staging directory: {}",
                    relative.display()
                ));
            }
            reject_unlisted_files(root, &path, files)?;
        } else if !files.contains_key(relative) {
            return Err(format!(
                "unlisted runtime staging file: {}",
                relative.display()
            ));
        }
    }
    Ok(())
}

fn verify_staged_runtime(
    directory: &Path,
    host_digest: &str,
    files: &BTreeMap<PathBuf, String>,
) -> Result<(), String> {
    let stamp = fs::read_to_string(directory.join(".complete"))
        .map_err(|error| format!("staging stamp: {error}"))?;
    if stamp != host_digest {
        return Err("staging stamp differs from the host binary digest".to_string());
    }
    let bin = directory.join("bin");
    let mut expected = files.clone();
    if expected
        .insert(
            PathBuf::from("paneflow-browser-host"),
            host_digest.to_string(),
        )
        .is_some()
    {
        return Err("runtime staging collides with the host executable".to_string());
    }
    reject_unlisted_files(&bin, &bin, &expected)?;
    verify_files(&bin, &expected)?;
    verify_host_binary(&bin.join("paneflow-browser-host"))?;
    Ok(())
}

fn stage(config: &HostConfig, host_digest: &str) -> Result<PathBuf, String> {
    let files = staged_files(config)?;
    let key = hex_digest(
        format!(
            "{}\n{}\n{}",
            config.runtime_root.display(),
            host_digest,
            manifest_digest()
        )
        .as_bytes(),
    );
    let directory = config.stage_root.join(format!("host-{}", &key[..16]));
    if directory.join(".complete").is_file() {
        verify_staged_runtime(&directory, host_digest, &files)?;
        return Ok(directory);
    }
    let scratch = config
        .stage_root
        .join(format!(".host-{}-{}", &key[..16], std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    let result = (|| {
        fs::create_dir_all(&scratch)?;
        fs::set_permissions(&scratch, fs::Permissions::from_mode(0o700))?;
        let bin = scratch.join("bin");
        link_tree(&config.runtime_root.join("Release"), &bin)?;
        link_tree(&config.runtime_root.join("Resources"), &bin)?;
        fs::copy(&config.host_binary, bin.join("paneflow-browser-host"))?;
        fs::write(scratch.join(".complete"), host_digest)?;
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
            verify_staged_runtime(&directory, host_digest, &files)?;
            Ok(directory)
        }
        Err(error) => {
            let _ = fs::remove_dir_all(&scratch);
            Err(format!("staging the browser host: {error}"))
        }
    }
}

fn group_alive(pgid: i32) -> bool {
    unsafe { libc::kill(-pgid, 0) == 0 }
}

fn signal_group(pgid: i32, signal: libc::c_int) {
    unsafe {
        libc::kill(-pgid, signal);
    }
}

fn shutdown_schedule(start: Instant, grace: Duration) -> (Instant, Instant, Instant) {
    (start + grace / 2, start + grace * 3 / 4, start + grace)
}

fn reap_group(pgid: i32, kill_at: Instant, deadline: Instant) -> bool {
    while group_alive(pgid) {
        if Instant::now() >= kill_at {
            signal_group(pgid, libc::SIGKILL);
            while group_alive(pgid) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            return !group_alive(pgid);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    true
}

fn reap_group_within(pgid: i32, budget: Duration) -> bool {
    let start = Instant::now();
    let (_, kill_at, deadline) = shutdown_schedule(start, budget);
    reap_group(pgid, kill_at, deadline)
}

impl Inner {
    fn finish_startup(&self) {
        if let Ok(mut finished) = self.startup_finished.lock() {
            *finished = true;
        }
        self.startup_changed.notify_all();
    }

    fn wait_for_startup(&self) {
        if let Ok(finished) = self.startup_finished.lock() {
            drop(
                self.startup_changed
                    .wait_while(finished, |finished| !*finished),
            );
        }
    }

    fn set_state(&self, state: HostState) {
        if let Ok(mut current) = self.state.lock() {
            *current = state;
        }
    }

    fn state(&self) -> HostState {
        self.state
            .lock()
            .map(|state| state.clone())
            .unwrap_or_else(|_| HostState::Failed("supervisor state poisoned".to_string()))
    }

    fn record_failure(&self, reason: String) {
        if let Ok(mut failure) = self.failure.lock() {
            failure.get_or_insert(reason);
        }
    }

    fn transport_failure_state(&self) -> HostState {
        let reason = self
            .failure
            .lock()
            .ok()
            .and_then(|failure| failure.clone())
            .unwrap_or_else(|| {
                "browser event transport failed; browser runtime must be recreated".to_string()
            });
        HostState::Failed(reason)
    }

    fn finish_shutdown(&self) {
        let failed = self
            .event_delivery_failed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.set_state(if *failed {
            self.transport_failure_state()
        } else {
            HostState::Inactive
        });
    }

    fn abort(&self, reason: String) {
        self.record_failure(reason);
        let pgid = self
            .running
            .lock()
            .ok()
            .and_then(|running| running.as_ref().map(|running| running.pgid));
        if let Some(pgid) = pgid {
            signal_group(pgid, libc::SIGKILL);
        }
    }

    fn emit(&self, event: HostEvent) -> bool {
        let mut failed = self
            .event_delivery_failed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *failed {
            return false;
        }
        let reason = if self.events.len() >= EVENT_QUEUE_CAPACITY {
            "host event queue saturated; browser runtime must be recreated".to_string()
        } else {
            match self.events.try_send(event) {
                Ok(()) => return true,
                Err(error) => format!("host event delivery failed: {error}"),
            }
        };
        *failed = true;
        self.set_state(HostState::Failed(reason.clone()));
        self.abort(reason.clone());
        if let Err(error) = self.events.try_send(HostEvent::Lost(reason)) {
            self.set_state(HostState::Failed(format!(
                "host failure notification unavailable: {error}"
            )));
        }
        self.events.close();
        false
    }
}

impl HostSupervisor {
    pub fn channel() -> (Self, smol::channel::Receiver<HostEvent>) {
        let (events, receiver) = smol::channel::bounded(EVENT_QUEUE_CAPACITY + 1);
        (
            Self {
                inner: Arc::new(Inner {
                    state: Mutex::new(HostState::Inactive),
                    startup_finished: Mutex::new(false),
                    startup_changed: Condvar::new(),
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

    pub fn pid(&self) -> Option<u32> {
        self.inner
            .running
            .lock()
            .ok()
            .and_then(|running| running.as_ref().map(|running| running.pid))
    }

    pub fn activate(&self, config: HostConfig) -> HostState {
        {
            let failed = self
                .inner
                .event_delivery_failed
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if *failed {
                let state = self.inner.transport_failure_state();
                self.inner.set_state(state.clone());
                return state;
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
        if let Ok(mut finished) = self.inner.startup_finished.lock() {
            *finished = false;
        }
        let inner = self.inner.clone();
        let spawned = std::thread::Builder::new()
            .name("browser-host-start".into())
            .spawn(move || {
                match start(&inner, &config) {
                    Ok(info) => {
                        let ready = inner
                            .state
                            .lock()
                            .map(|mut state| {
                                if *state == HostState::Starting {
                                    *state = HostState::Ready;
                                    true
                                } else {
                                    false
                                }
                            })
                            .unwrap_or(false);
                        if ready {
                            inner.emit(HostEvent::Ready(info));
                        }
                    }
                    Err(reason) => {
                        let pgid = inner
                            .running
                            .lock()
                            .ok()
                            .and_then(|mut running| running.take().map(|running| running.pgid));
                        if let Some(pgid) = pgid {
                            signal_group(pgid, libc::SIGKILL);
                            reap_group_within(pgid, Duration::from_secs(2));
                        }
                        inner.set_state(HostState::Failed(reason.clone()));
                        inner.emit(HostEvent::Lost(reason));
                    }
                }
                inner.finish_startup();
            });
        if let Err(error) = spawned {
            let reason = format!("host start thread: {error}");
            self.inner.set_state(HostState::Failed(reason.clone()));
            self.inner.emit(HostEvent::Lost(reason));
        }
        HostState::Starting
    }

    pub fn retry(&self, config: HostConfig) -> HostState {
        {
            let failed = self
                .inner
                .event_delivery_failed
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if *failed {
                return self.state();
            }
        }
        if let Ok(mut state) = self.inner.state.lock()
            && matches!(*state, HostState::Failed(_))
        {
            *state = HostState::Inactive;
        }
        self.activate(config)
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
        let Some(pid) = self.pid() else {
            return false;
        };
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) == 0 }
    }

    pub fn shutdown(&self, grace: Duration) {
        let running = {
            let failed = self
                .inner
                .event_delivery_failed
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Ok(mut state) = self.inner.state.lock() else {
                return;
            };
            let Ok(mut running) = self.inner.running.lock() else {
                return;
            };
            let Some(running) = running.take() else {
                if *failed {
                    *state = self.inner.transport_failure_state();
                } else if !matches!(*state, HostState::Failed(_)) {
                    *state = HostState::Inactive;
                }
                return;
            };
            running.stop_requested.store(true, Ordering::Release);
            *state = if *failed {
                self.inner.transport_failure_state()
            } else {
                HostState::Stopping
            };
            running
        };
        let _ = running.outbound.try_send(Outbound::Close);
        let (lock, condvar) = &*running.exited;
        let (closed_by, kill_at, deadline) = shutdown_schedule(Instant::now(), grace);
        if let Ok(mut exited) = lock.lock() {
            while !*exited {
                let now = Instant::now();
                if now >= closed_by {
                    break;
                }
                match condvar.wait_timeout(exited, closed_by - now) {
                    Ok((guard, _)) => exited = guard,
                    Err(_) => break,
                }
            }
        }
        signal_group(running.pgid, libc::SIGTERM);
        reap_group(running.pgid, kill_at, deadline);
        self.inner.finish_shutdown();
    }
}

fn start(inner: &Arc<Inner>, config: &HostConfig) -> Result<HostInfo, String> {
    verify_runtime(&config.runtime_root, config.check)?;
    let host_digest = verify_host_binary(&config.host_binary)?;
    let staged = stage(config, &host_digest)?;
    let bin = staged.join("bin");
    let profile = config.profile_dir();
    super::profile::prepare_profile_dir(&profile).map_err(|error| error.message())?;
    let (parent_channel, child_channel) = if config.frames {
        let (parent, child) =
            FrameChannel::pair().map_err(|error| format!("frame channel: {error}"))?;
        (Some(parent), Some(child))
    } else {
        (None, None)
    };
    let stderr = File::create(profile.join("host.stderr"))
        .map_err(|error| format!("host stderr log: {error}"))?;
    let mut command = Command::new(bin.join("paneflow-browser-host"));
    command
        .env(RUNTIME_ENV, &config.runtime_root)
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
        .env("PANEFLOW_BROWSER_OZONE", config.ozone.switch())
        .env(
            "PANEFLOW_BROWSER_TRACE",
            if config.tracing { "1" } else { "0" },
        )
        .env("LD_LIBRARY_PATH", &bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr))
        .process_group(0);
    if config.ozone == Ozone::Wayland {
        command.env_remove("DISPLAY");
    }
    if let Some((vendor, device)) = config.gpu {
        command.env(
            "PANEFLOW_BROWSER_GPU",
            format!("{vendor:#06x}:{device:#06x}"),
        );
    }
    if let Some(render_node) = &config.render_node {
        command.env(RENDER_NODE_ENV, render_node);
    }
    if let Some(child_channel) = &child_channel {
        let raw = child_channel.as_raw_fd();
        command.env(FRAME_CHANNEL_ENV, "3");
        unsafe {
            command.pre_exec(move || {
                if libc::dup2(raw, 3) < 0 {
                    return Err(io::Error::last_os_error());
                }
                let flags = libc::fcntl(3, libc::F_GETFD);
                if flags < 0 || libc::fcntl(3, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("spawning the browser host: {error}"))?;
    drop(child_channel);
    let pid = child.id();
    let pgid = pid as i32;
    let stdin = child.stdin.take().ok_or("host stdin unavailable")?;
    let stdout = child.stdout.take().ok_or("host stdout unavailable")?;
    let (outbound, outbound_receiver) = mpsc::sync_channel::<Outbound>(256);
    let exited = Arc::new((Mutex::new(false), Condvar::new()));
    let stop_requested = Arc::new(AtomicBool::new(false));
    if let Ok(mut running) = inner.running.lock() {
        *running = Some(Running {
            pid,
            pgid,
            outbound: outbound.clone(),
            frame_connection: Arc::new(()),
            exited: exited.clone(),
            stop_requested: stop_requested.clone(),
        });
    }
    let writer_inner = inner.clone();
    let writer_frames = parent_channel
        .as_ref()
        .map(FrameChannel::try_clone)
        .transpose()
        .map_err(|error| format!("frame channel: {error}"))?;
    std::thread::Builder::new()
        .name("browser-host-writer".into())
        .spawn(move || {
            let mut stdin = stdin;
            while let Ok(message) = outbound_receiver.recv() {
                let result = match message {
                    Outbound::Control(envelope) => write_message(&mut stdin, &envelope),
                    Outbound::Ack(ack) => match &writer_frames {
                        Some(frames) => frames.send(&ack, &[]),
                        None => Ok(()),
                    },
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
            let mut stdout = BufReader::new(stdout);
            let mut handshake = Some(handshake);
            loop {
                let header = stdout.fill_buf().ok().and_then(|bytes| {
                    bytes.get(..4).and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
                });
                match read_value(&mut stdout) {
                    Ok(Some(value)) => {
                        if let Some(sender) = &handshake {
                            let initialized =
                                value.get("native") == Some(&Value::from("initialized"));
                            if sender.send(value).is_err() {
                                reader_inner.abort("host handshake listener vanished".to_string());
                                return;
                            }
                            if initialized {
                                handshake = None;
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
                            "host event stream violates the contract: {error:?} (header {header:02x?})"
                        ));
                        return;
                    }
                }
            }
        })
        .map_err(|error| format!("host reader thread: {error}"))?;
    if let Some(channel) = parent_channel {
        let frames_inner = inner.clone();
        std::thread::Builder::new()
            .name("browser-host-frames".into())
            .spawn(move || {
                loop {
                    match channel.recv::<FrameMessage>() {
                        Ok(Some((message, fds))) => {
                            if !message.is_valid() || fds.len() != message.expected_fds() {
                                frames_inner
                                    .abort("host frame message violates the contract".to_string());
                                return;
                            }
                            if !frames_inner.emit(HostEvent::Frame(message, fds)) {
                                return;
                            }
                        }
                        Ok(None) => return,
                        Err(error) => {
                            frames_inner.abort(format!(
                                "host frame channel violates the contract: {error}"
                            ));
                            return;
                        }
                    }
                }
            })
            .map_err(|error| format!("host frame thread: {error}"))?;
    }
    let watcher_inner = inner.clone();
    std::thread::Builder::new()
        .name("browser-host-watch".into())
        .spawn(move || watch(watcher_inner, child, pgid, exited, stop_requested))
        .map_err(|error| format!("host watch thread: {error}"))?;
    let hello = Envelope {
        version: CONTRACT_VERSION,
        operation: OperationId::try_from("hello".to_string()).map_err(|error| error.to_string())?,
        command: BrowserCommand::Capabilities,
    };
    outbound
        .try_send(Outbound::Control(hello))
        .map_err(|_| "host control queue is full before the handshake".to_string())?;
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let mut presentation_ready = false;
    let host_stderr_hint = || {
        fs::metadata(profile.join("host.stderr"))
            .map(|metadata| metadata.len().min(HOST_STDERR_LIMIT))
            .unwrap_or(0)
    };
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err("host handshake timed out".to_string());
        }
        let value = match handshake_receiver.recv_timeout(deadline - now) {
            Ok(value) => value,
            Err(RecvTimeoutError::Timeout) => return Err("host handshake timed out".to_string()),
            Err(RecvTimeoutError::Disconnected) => {
                let recorded = inner
                    .failure
                    .lock()
                    .ok()
                    .and_then(|failure| failure.clone());
                return Err(recorded.unwrap_or_else(|| {
                    format!(
                        "host ended before the handshake (stderr {} bytes in {})",
                        host_stderr_hint(),
                        profile.display()
                    )
                }));
            }
        };
        match value.get("native").and_then(Value::as_str) {
            Some("presentation_ready") => presentation_ready = true,
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
                let presentation = value
                    .get("presentation")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if config.frames && !(presentation && presentation_ready) {
                    return Err("host did not initialize GPU presentation".to_string());
                }
                let reported = value.get("pid").and_then(Value::as_u64).unwrap_or(0);
                if reported != u64::from(pid) {
                    return Err("host reported a foreign process id".to_string());
                }
                return Ok(HostInfo {
                    pid,
                    availability,
                    contract_version,
                    presentation,
                    initialized: value,
                });
            }
            Some("frame_failed") => {
                return Err(format!(
                    "host presentation failed: {}",
                    value.get("detail").and_then(Value::as_str).unwrap_or("")
                ));
            }
            _ => (),
        }
    }
}

fn watch(
    inner: Arc<Inner>,
    mut child: Child,
    pgid: i32,
    exited: Arc<(Mutex<bool>, Condvar)>,
    stop_requested: Arc<AtomicBool>,
) {
    let status = child.wait();

    let (lock, condvar) = &*exited;
    if let Ok(mut flag) = lock.lock() {
        *flag = true;
    }
    condvar.notify_all();
    signal_group(pgid, libc::SIGTERM);
    let clean = reap_group_within(pgid, Duration::from_secs(2));
    if let Ok(mut running) = inner.running.lock()
        && running.as_ref().is_some_and(|running| running.pgid == pgid)
    {
        *running = None;
    }
    inner.wait_for_startup();
    if matches!(inner.state(), HostState::Failed(_)) {
        return;
    }
    if stop_requested.load(Ordering::Acquire) {
        inner.finish_shutdown();
        inner.emit(HostEvent::Stopped);
        return;
    }
    let recorded = inner
        .failure
        .lock()
        .ok()
        .and_then(|failure| failure.clone());
    let mut reason = recorded.unwrap_or_else(|| match status {
        Ok(status) => format!("host exited: {status}"),
        Err(error) => format!("host wait failed: {error}"),
    });
    if !clean {
        reason.push_str("; host descendants survived the group cleanup");
    }
    inner.set_state(HostState::Failed(reason.clone()));
    inner.emit(HostEvent::Lost(reason));
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use paneflow_browser_protocol::{SessionId, WorkspaceId};

    use super::*;

    #[test]
    fn gpu_acknowledger_cannot_follow_a_replaced_host_connection() {
        let (supervisor, _events) = HostSupervisor::channel();
        let install = |outbound| {
            *supervisor.inner.running.lock().unwrap() = Some(Running {
                pid: 10,
                pgid: 10,
                outbound,
                frame_connection: Arc::new(()),
                exited: Arc::new((Mutex::new(false), Condvar::new())),
                stop_requested: Arc::new(AtomicBool::new(false)),
            });
        };
        let (old_sender, old_receiver) = mpsc::sync_channel(2);
        install(old_sender);
        let acknowledger = supervisor.frame_acknowledger().unwrap();
        assert!(supervisor.has_frame_connection(&acknowledger));
        let (new_sender, new_receiver) = mpsc::sync_channel(2);
        install(new_sender);
        assert!(!supervisor.has_frame_connection(&acknowledger));
        let ack = serde_json::from_value::<FrameAck>(serde_json::json!({
            "type": "release", "document": {
                "owner": { "workspace": "w", "session": "s" },
                "browser": "b", "generation": 1
            }, "pool_generation": 1, "buffer": 0, "sequence": 1
        }))
        .unwrap();
        acknowledger.ack(ack.clone()).unwrap();
        assert!(matches!(old_receiver.try_recv(), Ok(Outbound::Ack(_))));
        assert!(matches!(
            new_receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        drop(old_receiver);
        assert_eq!(acknowledger.ack(ack), Err(BrowserError::Unavailable));
    }

    const FAKE_HOST: &str = r#"
import json, os, struct, subprocess, sys, time
mode = os.environ.get("FAKE_HOST_MODE", "normal")
counter = os.environ.get("FAKE_HOST_COUNTER")
if counter:
    with open(counter, "a") as handle:
        handle.write(f"{os.getpid()}\n")
out = sys.stdout.buffer
inp = sys.stdin.buffer
def emit(value):
    body = json.dumps(value).encode()
    out.write(struct.pack(">I", len(body)) + body)
    out.flush()
def read():
    header = inp.read(4)
    if len(header) < 4:
        return None
    length = struct.unpack(">I", header)[0]
    return json.loads(inp.read(length))
if mode == "garbage":
    out.write(b"this is not a length-prefixed protocol message " * 8)
    out.flush()
    time.sleep(30)
    sys.exit(1)
first = read()
if first is None or first["command"]["type"] != "capabilities":
    sys.exit(3)
child = subprocess.Popen(["sleep", "300"]) if mode == "spawn-child" else None
presentation = "PANEFLOW_BROWSER_FRAME_FD" in os.environ and mode != "no-presentation"
if presentation:
    emit({"native": "presentation_ready", "device": "fake"})
hello = {"version": 3, "operation": first["operation"], "result": {"Ok": {"type": "capabilities", "target": "fake", "availability": "development", "contract_version": 3, "terminal_available": True}}}
emit({"native": "initialized", "pid": os.getpid(), "sandbox_requested": True, "contract_version": 3, "presentation": presentation, "protocol": hello, "child_pid": child.pid if child else None})
if mode == "flood":
    for index in range(10000):
        emit({"native": "console_message", "message": str(index)})
if mode == "exit-after-ready":
    sys.exit(7)
while True:
    message = read()
    if message is None:
        break
    emit({"protocol": {"version": 3, "operation": message["operation"], "result": {"Err": "unavailable"}}})
"#;

    struct Fixture {
        directory: PathBuf,
        config: HostConfig,
        counter: PathBuf,
    }

    impl Fixture {
        fn new(name: &str, mode: &str) -> Self {
            let directory = std::env::temp_dir().join(format!(
                "paneflow-browser-supervisor-{name}-{}-{}",
                std::process::id(),
                now_ns()
            ));
            fs::create_dir_all(directory.join("runtime/Release")).unwrap();
            fs::create_dir_all(directory.join("runtime/Resources/locales")).unwrap();
            for name in REQUIRED_FILES {
                fs::write(directory.join("runtime").join(name), b"stub").unwrap();
            }
            fs::write(
                directory.join("runtime/verified-manifest.sha256"),
                format!("{}\n", manifest_digest()),
            )
            .unwrap();
            let host = directory.join("fake-host");
            fs::write(&host, format!("#!/bin/sh\nexec env FAKE_HOST_MODE={mode} FAKE_HOST_COUNTER={} python3 -c \"$(cat {})\"\n", directory.join("spawns").display(), directory.join("fake-host.py").display())).unwrap();
            fs::write(directory.join("fake-host.py"), FAKE_HOST).unwrap();
            fs::set_permissions(&host, fs::Permissions::from_mode(0o755)).unwrap();
            let config = HostConfig {
                tracing: false,
                host_binary: host,
                runtime_root: directory.join("runtime"),
                stage_root: directory.join("stage"),
                profile_dir: directory.join("stage/profiles/test"),
                origin: "http://127.0.0.1:1".to_string(),
                owner: Owner {
                    workspace: WorkspaceId::try_from("test".to_string()).unwrap(),
                    session: SessionId::try_from("browser".to_string()).unwrap(),
                },
                ozone: Ozone::Wayland,
                gpu: None,
                render_node: None,
                frames: true,
                check: RuntimeCheck::StampOnly,
            };
            Self {
                directory: directory.clone(),
                config,
                counter: directory.join("spawns"),
            }
        }

        fn spawns(&self) -> usize {
            fs::read_to_string(&self.counter)
                .map(|text| text.lines().count())
                .unwrap_or(0)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    fn now_ns() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    }

    fn next_event(receiver: &smol::channel::Receiver<HostEvent>, timeout: Duration) -> HostEvent {
        let deadline = Instant::now() + timeout;
        loop {
            match receiver.try_recv() {
                Ok(event) => return event,
                Err(smol::channel::TryRecvError::Empty) => {
                    assert!(
                        Instant::now() < deadline,
                        "no host event within {timeout:?}"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(smol::channel::TryRecvError::Closed) => panic!("event channel closed"),
            }
        }
    }

    fn process_alive(pid: i32) -> bool {
        unsafe { libc::kill(pid, 0) == 0 }
    }

    #[test]
    fn nothing_runs_and_no_libcef_is_mapped_before_activation() {
        let fixture = Fixture::new("inactive", "normal");
        let (supervisor, _receiver) = HostSupervisor::channel();
        assert_eq!(supervisor.state(), HostState::Inactive);
        assert_eq!(fixture.spawns(), 0);
        assert!(supervisor.pid().is_none());
        assert_eq!(
            supervisor.send(BrowserCommand::Capabilities),
            Err(BrowserError::Unavailable)
        );
        let maps = fs::read_to_string("/proc/self/maps").unwrap();
        assert!(
            !maps.contains("libcef"),
            "libcef must not be loaded in the app process"
        );
    }

    #[test]
    fn concurrent_activation_starts_exactly_one_host() {
        let fixture = Fixture::new("concurrent", "normal");
        let (supervisor, receiver) = HostSupervisor::channel();
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let supervisor = supervisor.clone();
                let config = fixture.config.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    supervisor.activate(config)
                })
            })
            .collect();
        barrier.wait();
        let states: Vec<HostState> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert!(states.iter().all(|state| *state == HostState::Starting));
        let event = next_event(&receiver, Duration::from_secs(20));
        let HostEvent::Ready(info) = event else {
            panic!("expected Ready, got {event:?}");
        };
        assert!(info.presentation);
        assert_eq!(info.contract_version, CONTRACT_VERSION);
        assert_eq!(supervisor.state(), HostState::Ready);
        assert_eq!(
            supervisor.activate(fixture.config.clone()),
            HostState::Ready
        );
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(fixture.spawns(), 1);
        assert!(supervisor.send(BrowserCommand::Capabilities).is_ok());
        let reply = next_event(&receiver, Duration::from_secs(5));
        assert!(
            matches!(reply, HostEvent::Reply(_)),
            "expected a reply, got {reply:?}"
        );
        supervisor.shutdown(Duration::from_secs(5));
        assert_eq!(supervisor.state(), HostState::Inactive);
        assert_eq!(fixture.spawns(), 1);
    }

    #[test]
    fn a_garbage_handshake_fails_once_and_never_restarts_on_its_own() {
        let fixture = Fixture::new("garbage", "garbage");
        let (supervisor, receiver) = HostSupervisor::channel();
        assert_eq!(
            supervisor.activate(fixture.config.clone()),
            HostState::Starting
        );
        let event = next_event(&receiver, Duration::from_secs(20));
        let HostEvent::Lost(reason) = event else {
            panic!("expected Lost, got {event:?}");
        };
        assert!(reason.contains("contract"), "{reason}");
        assert!(matches!(supervisor.state(), HostState::Failed(_)));
        assert!(matches!(
            supervisor.activate(fixture.config.clone()),
            HostState::Failed(_)
        ));
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            fixture.spawns(),
            1,
            "a failed host must not be relaunched implicitly"
        );
        assert!(
            receiver.try_recv().is_err(),
            "no second event after the failure"
        );
        assert_eq!(
            supervisor.retry(fixture.config.clone()),
            HostState::Starting
        );
        let event = next_event(&receiver, Duration::from_secs(20));
        assert!(matches!(event, HostEvent::Lost(_)));
        assert_eq!(fixture.spawns(), 2);
        supervisor.shutdown(Duration::from_secs(2));
    }

    #[test]
    fn a_dead_host_is_reported_without_relaunch() {
        let fixture = Fixture::new("dead", "exit-after-ready");
        let (supervisor, receiver) = HostSupervisor::channel();
        supervisor.activate(fixture.config.clone());
        let event = next_event(&receiver, Duration::from_secs(20));
        let event = match event {
            HostEvent::Ready(_) => next_event(&receiver, Duration::from_secs(10)),
            event => event,
        };
        let HostEvent::Lost(reason) = event else {
            panic!("expected Lost, got {event:?}");
        };
        assert!(reason.contains("exit"), "{reason}");
        assert!(matches!(supervisor.state(), HostState::Failed(_)));
        assert_eq!(
            supervisor.send(BrowserCommand::Capabilities),
            Err(BrowserError::Unavailable)
        );
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(fixture.spawns(), 1);
    }

    #[test]
    fn the_shutdown_schedule_never_exceeds_the_stop_budget() {
        let start = Instant::now();
        let (closed_by, kill_at, deadline) = shutdown_schedule(start, SHUTDOWN_GRACE);
        assert!(start < closed_by && closed_by < kill_at && kill_at < deadline);
        assert_eq!(deadline - start, SHUTDOWN_GRACE);
    }

    #[test]
    fn shutdown_reaps_every_descendant_of_the_host() {
        let fixture = Fixture::new("descendants", "spawn-child");
        let (supervisor, receiver) = HostSupervisor::channel();
        supervisor.activate(fixture.config.clone());
        let event = next_event(&receiver, Duration::from_secs(20));
        let HostEvent::Ready(info) = event else {
            panic!("expected Ready, got {event:?}");
        };
        let child = info.initialized["child_pid"].as_i64().unwrap() as i32;
        assert!(process_alive(child));
        let started = Instant::now();
        supervisor.shutdown(SHUTDOWN_GRACE);
        assert!(started.elapsed() <= SHUTDOWN_GRACE);
        assert!(
            !process_alive(child),
            "the host's descendant survived shutdown"
        );
        assert_eq!(supervisor.state(), HostState::Inactive);
        let event = next_event(&receiver, Duration::from_secs(5));
        assert!(matches!(event, HostEvent::Stopped), "{event:?}");
    }

    #[test]
    fn runtime_verification_rejects_a_foreign_or_altered_runtime() {
        let fixture = Fixture::new("verify", "normal");
        assert!(verify_runtime(&fixture.config.runtime_root, RuntimeCheck::StampOnly).is_ok());
        let altered = verify_runtime(&fixture.config.runtime_root, RuntimeCheck::Manifest);
        assert!(
            altered.is_err(),
            "stub files must fail complete manifest verification"
        );
        fs::write(
            fixture.config.runtime_root.join("verified-manifest.sha256"),
            "0000\n",
        )
        .unwrap();
        let foreign = verify_runtime(&fixture.config.runtime_root, RuntimeCheck::StampOnly);
        assert!(foreign.unwrap_err().contains("different browser manifest"));
        let (supervisor, receiver) = HostSupervisor::channel();
        supervisor.activate(fixture.config.clone());
        let event = next_event(&receiver, Duration::from_secs(10));
        assert!(matches!(event, HostEvent::Lost(_)), "{event:?}");
        assert_eq!(
            fixture.spawns(),
            0,
            "no host may start on an unverified runtime"
        );
    }

    #[test]
    fn a_missing_presentation_is_a_handshake_failure() {
        let fixture = Fixture::new("nopres", "no-presentation");
        let (supervisor, receiver) = HostSupervisor::channel();
        supervisor.activate(fixture.config.clone());
        let event = next_event(&receiver, Duration::from_secs(20));
        let HostEvent::Lost(reason) = event else {
            panic!("expected Lost, got {event:?}");
        };
        assert!(reason.contains("presentation"), "{reason}");
        assert!(supervisor.pid().is_none());
    }

    #[test]
    fn saturated_events_reserve_one_failure_for_diagnostics_controls_and_frames() {
        let fixture = Fixture::new("queue", "normal");
        let document = paneflow_browser_protocol::Document {
            owner: fixture.config.owner.clone(),
            browser: paneflow_browser_protocol::BrowserId::try_from("test".to_string()).unwrap(),
            generation: 1,
        };
        let events = [
            HostEvent::Native(serde_json::json!({"native":"console_message"})),
            HostEvent::Reply(Reply {
                version: CONTRACT_VERSION,
                operation: OperationId::try_from("test".to_string()).unwrap(),
                result: Err(BrowserError::Unavailable),
            }),
            HostEvent::Frame(
                FrameMessage::PoolRetired {
                    document,
                    pool_generation: 1,
                },
                Vec::new(),
            ),
        ];
        for event in events {
            let (supervisor, receiver) = HostSupervisor::channel();
            supervisor.inner.set_state(HostState::Ready);
            for index in 0..EVENT_QUEUE_CAPACITY {
                assert!(
                    supervisor
                        .inner
                        .emit(HostEvent::Native(serde_json::json!({"index":index})))
                );
            }
            assert!(!supervisor.inner.emit(event));
            assert!(
                matches!(supervisor.state(), HostState::Failed(reason) if reason.contains("saturated"))
            );
            assert_eq!(receiver.len(), EVENT_QUEUE_CAPACITY + 1);
            assert!(receiver.is_closed());
            assert!(!supervisor.inner.emit(HostEvent::Stopped));
            for _ in 0..EVENT_QUEUE_CAPACITY {
                assert!(matches!(receiver.try_recv().unwrap(), HostEvent::Native(_)));
            }
            assert!(
                matches!(receiver.try_recv().unwrap(), HostEvent::Lost(reason) if reason.contains("saturated"))
            );
            assert!(receiver.try_recv().is_err());
        }
    }

    #[test]
    fn concurrent_event_producers_keep_the_failure_reserve_bounded() {
        let (supervisor, receiver) = HostSupervisor::channel();
        let producers: Vec<_> = (0..8)
            .map(|_| {
                let inner = supervisor.inner.clone();
                std::thread::spawn(move || {
                    for _ in 0..EVENT_QUEUE_CAPACITY {
                        if !inner.emit(HostEvent::Native(Value::Null)) {
                            break;
                        }
                    }
                })
            })
            .collect();
        for producer in producers {
            producer.join().unwrap();
        }
        assert_eq!(receiver.len(), EVENT_QUEUE_CAPACITY + 1);
        let mut failures = 0;
        while let Ok(event) = receiver.try_recv() {
            failures += usize::from(matches!(event, HostEvent::Lost(_)));
        }
        assert_eq!(failures, 1);
    }

    #[test]
    fn vanished_event_consumer_fails_the_transport_immediately() {
        let (supervisor, receiver) = HostSupervisor::channel();
        drop(receiver);
        assert!(!supervisor.inner.emit(HostEvent::Native(Value::Null)));
        assert!(
            matches!(supervisor.state(), HostState::Failed(reason) if reason.contains("notification unavailable"))
        );
        assert_eq!(
            supervisor.send(BrowserCommand::Capabilities),
            Err(BrowserError::Unavailable)
        );
    }

    #[test]
    fn a_page_diagnostic_flood_stops_the_host_without_unbounded_buffering() {
        let fixture = Fixture::new("flood", "flood");
        let (supervisor, receiver) = HostSupervisor::channel();
        supervisor.activate(fixture.config.clone());
        let deadline = Instant::now() + Duration::from_secs(20);
        while !matches!(supervisor.state(), HostState::Failed(_)) || supervisor.pid().is_some() {
            assert!(Instant::now() < deadline, "flooding host was not stopped");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            matches!(supervisor.state(), HostState::Failed(reason) if reason.contains("saturated"))
        );
        assert_eq!(receiver.len(), EVENT_QUEUE_CAPACITY + 1);
        assert!(matches!(
            supervisor.retry(fixture.config.clone()),
            HostState::Failed(_)
        ));
        assert_eq!(fixture.spawns(), 1);
        let mut failures = 0;
        while let Ok(event) = receiver.try_recv() {
            failures += usize::from(matches!(event, HostEvent::Lost(_)));
        }
        assert_eq!(failures, 1);
        assert!(matches!(
            supervisor.retry(fixture.config.clone()),
            HostState::Failed(_)
        ));
        assert_eq!(fixture.spawns(), 1);
    }

    #[test]
    fn activate_rejects_a_poisoned_transport_even_after_an_inactive_state_reset() {
        let fixture = Fixture::new("closed-activate", "normal");
        let (supervisor, _receiver) = HostSupervisor::channel();
        for _ in 0..=EVENT_QUEUE_CAPACITY {
            supervisor.inner.emit(HostEvent::Native(Value::Null));
        }
        supervisor.inner.set_state(HostState::Inactive);
        assert!(
            matches!(supervisor.activate(fixture.config.clone()), HostState::Failed(reason) if reason.contains("saturated"))
        );
        assert!(matches!(supervisor.state(), HostState::Failed(_)));
        assert!(supervisor.pid().is_none());
        assert_eq!(fixture.spawns(), 0);
    }

    #[test]
    fn shutdown_preserves_a_poisoned_transport_and_forbids_reactivation() {
        let fixture = Fixture::new("closed-shutdown", "normal");
        let (supervisor, receiver) = HostSupervisor::channel();
        supervisor.activate(fixture.config.clone());
        assert!(matches!(
            next_event(&receiver, Duration::from_secs(20)),
            HostEvent::Ready(_)
        ));
        for _ in 0..=EVENT_QUEUE_CAPACITY {
            supervisor.inner.emit(HostEvent::Native(Value::Null));
        }
        supervisor.shutdown(Duration::from_secs(2));
        assert!(
            matches!(supervisor.state(), HostState::Failed(reason) if reason.contains("saturated"))
        );
        while receiver.try_recv().is_ok() {}
        assert!(receiver.is_closed());
        assert!(matches!(
            supervisor.activate(fixture.config.clone()),
            HostState::Failed(_)
        ));
        assert!(matches!(
            supervisor.retry(fixture.config.clone()),
            HostState::Failed(_)
        ));
        assert!(supervisor.pid().is_none());
        assert_eq!(fixture.spawns(), 1);
        supervisor.shutdown(Duration::ZERO);
        assert!(matches!(supervisor.state(), HostState::Failed(_)));
    }

    #[test]
    fn every_loaded_cef_library_is_covered_and_alterations_are_rejected() {
        let fixture = Fixture::new("all-digests", "normal");
        let libraries = [
            "Release/libcef.so",
            "Release/libEGL.so",
            "Release/libGLESv2.so",
            "Release/libvulkan.so.1",
            "Release/libvk_swiftshader.so",
        ];
        let manifest = manifest_files().unwrap();
        let expected: BTreeMap<_, _> = libraries
            .iter()
            .map(|name| {
                assert!(manifest.contains_key(Path::new(name)));
                fs::write(fixture.config.runtime_root.join(name), b"verified fixture").unwrap();
                (PathBuf::from(name), hex_digest(b"verified fixture"))
            })
            .collect();
        assert!(verify_files(&fixture.config.runtime_root, &expected).is_ok());
        for name in libraries {
            fs::write(fixture.config.runtime_root.join(name), b"altered fixture").unwrap();
            let error = verify_files(&fixture.config.runtime_root, &expected).unwrap_err();
            assert!(error.contains(name), "{error}");
            fs::write(fixture.config.runtime_root.join(name), b"verified fixture").unwrap();
        }
    }

    #[test]
    fn reused_staging_rejects_replaced_libraries_resources_and_host_binary() {
        let fixture = Fixture::new("stage-altered", "normal");
        let host_digest = file_digest(&fixture.config.host_binary).unwrap();
        for name in [
            "libcef.so",
            "icudtl.dat",
            "locales/en-US.pak",
            "paneflow-browser-host",
        ] {
            let staged = stage(&fixture.config, &host_digest).unwrap();
            let path = staged.join("bin").join(name);
            fs::remove_file(&path).unwrap();
            fs::write(&path, b"replaced staging file").unwrap();
            let error = stage(&fixture.config, &host_digest).unwrap_err();
            assert!(error.contains(name), "{error}");
            fs::remove_dir_all(staged).unwrap();
        }
    }

    #[test]
    fn reused_staging_rejects_extra_libraries_and_accepts_verified_symlinks() {
        let fixture = Fixture::new("stage-extra", "normal");
        let host_digest = file_digest(&fixture.config.host_binary).unwrap();
        let staged = stage(&fixture.config, &host_digest).unwrap();
        let library = staged.join("bin/libcef.so");
        fs::remove_file(&library).unwrap();
        std::os::unix::fs::symlink(
            fixture.config.runtime_root.join("Release/libcef.so"),
            &library,
        )
        .unwrap();
        assert_eq!(stage(&fixture.config, &host_digest).unwrap(), staged);
        fs::write(staged.join("bin/libz.so.1"), b"unlisted injected library").unwrap();
        assert!(
            stage(&fixture.config, &host_digest)
                .unwrap_err()
                .contains("unlisted")
        );
    }
}
