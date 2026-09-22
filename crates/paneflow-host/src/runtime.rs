use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use paneflow_config::schema::SessionGeneration;
use paneflow_terminal_ghostty as ghostty;
use portable_pty::{CommandBuilder, PtySize};

use crate::process::ProcessIdentity;
use crate::protocol::{MAX_CHECKPOINT_BYTES, MAX_OUTPUT_TAIL_BYTES, REQUEST_DEADLINE};
use crate::tail::OutputTail;

const READ_CHUNK_BYTES: usize = 32 * 1024;
const OUTPUT_QUEUE_SLOTS: usize = 64;
const INPUT_QUEUE_SLOTS: usize = 64;
const QUEUE_RETRY: Duration = Duration::from_millis(5);
const RUNTIME_TICK: Duration = Duration::from_millis(20);
pub const STARTUP_DEADLINE: Duration = Duration::from_secs(10);
pub const STOP_BUDGET: Duration = Duration::from_secs(5);
const CELL_WIDTH_PX: u32 = 8;
const CELL_HEIGHT_PX: u32 = 16;
const TERMINFO_NAME: &str = "xterm-256color";
const SCROLLBACK_BYTES_PER_LINE: usize = 1024;
const MAX_SCROLLBACK_BYTES: usize = 128 * 1024 * 1024;
pub const CONTINUATION_MAX_BYTES: usize = 64 * 1024;
const NEWLINE: &str = "\n";
const NEWLINE_CHAR: char = '\n';
const RUNTIME_PANIC_REASON: &str =
    "the session runtime panicked; its child process is held for recovery";
const LATE_LAUNCH_CANCELLED: &str = "the launch was cancelled before its process was published";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnSpec {
    pub shell: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub cols: u16,
    pub rows: u16,
    pub scrollback_lines: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitOutcome {
    pub code: i64,
    pub signal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeNotice {
    Title(String),
    WorkingDirectory(String),
    Exited(ExitOutcome),
    Unverified(String),
}

pub type RuntimeObserver = Arc<dyn Fn(RuntimeNotice) + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub generation: SessionGeneration,
    pub offset: u64,
    pub cols: u16,
    pub rows: u16,
    pub snapshot: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewportScan {
    pub screen: String,
    pub foreground_process_group: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputSlice {
    pub offset: u64,
    pub data: Vec<u8>,
    pub end_offset: u64,
    pub live: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StopReport {
    pub exit: Option<ExitOutcome>,
    pub descendants_unresolved: usize,
    pub unverified: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeError {
    #[error("the session runtime is gone")]
    Gone,
    #[error("the session did not answer within {0:?}")]
    Deadline(Duration),
    #[error("the session is not running")]
    NotLive,
    #[error("the session process could not be verified: {0}")]
    Unverified(String),
    #[error("terminal state of {bytes} bytes exceeds the {limit}-byte attachment limit")]
    CheckpointTooLarge { bytes: usize, limit: usize },
    #[error("output offset {requested} was evicted; the tail now covers {tail_start}..{tail_end}")]
    OutputEvicted {
        requested: u64,
        tail_start: u64,
        tail_end: u64,
    },
    #[error("terminal engine error: {0}")]
    Engine(String),
    #[error("pty error: {0}")]
    Pty(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SpawnError(pub String);

enum Command {
    Checkpoint(SyncSender<Result<Checkpoint, RuntimeError>>),
    Text(SyncSender<Result<String, RuntimeError>>),
    Viewport(SyncSender<Result<ViewportScan, RuntimeError>>),
    BracketedPaste(SyncSender<Result<bool, RuntimeError>>),
    Output {
        from: u64,
        max: usize,
        reply: SyncSender<Result<OutputSlice, RuntimeError>>,
    },
    Input(Vec<u8>, SyncSender<Result<usize, RuntimeError>>),
    Resize {
        cols: u16,
        rows: u16,
        reply: SyncSender<Result<(), RuntimeError>>,
    },
    Stop {
        deadline: Instant,
        reply: SyncSender<StopReport>,
    },
    #[cfg(test)]
    InjectPanic,
}

enum Message {
    Output(Vec<u8>),
    Eof,
    Command(Command),
}

struct Shared {
    exit: Mutex<Option<ExitOutcome>>,
    unverified: Mutex<Option<String>>,
    descendants_unresolved: AtomicUsize,
    stop_requested: AtomicBool,
    output_changed_at_ms: AtomicU64,
}

impl Shared {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            exit: Mutex::new(None),
            unverified: Mutex::new(None),
            descendants_unresolved: AtomicUsize::new(0),
            stop_requested: AtomicBool::new(false),
            output_changed_at_ms: AtomicU64::new(0),
        })
    }

    fn exit(&self) -> Option<ExitOutcome> {
        self.exit
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn unverified(&self) -> Option<String> {
        self.unverified
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn set_unverified(&self, reason: Option<String>) {
        *self
            .unverified
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = reason;
    }
}

pub struct SessionRuntime {
    tx: SyncSender<Message>,
    shared: Arc<Shared>,
    generation: SessionGeneration,
    process: ProcessIdentity,
}

pub struct LaunchHandle {
    tx: SyncSender<Message>,
    shared: Arc<Shared>,
    generation: SessionGeneration,
    startup_rx: Receiver<Result<ProcessIdentity, String>>,
}

pub enum LaunchWait {
    Ready(SessionRuntime),
    Recovery(SessionRuntime),
    Failed(String),
    Pending(LaunchHandle),
}

#[derive(Clone)]
pub struct LaunchCancel(Arc<Shared>);

impl LaunchCancel {
    pub fn cancel(&self) {
        self.0.stop_requested.store(true, Ordering::Release);
    }
}

impl LaunchHandle {
    pub fn wait(self, budget: Duration) -> LaunchWait {
        match self.startup_rx.recv_timeout(budget) {
            Ok(Ok(process)) => {
                if self.shared.stop_requested.load(Ordering::Acquire) {
                    self.shared
                        .set_unverified(Some(LATE_LAUNCH_CANCELLED.into()));
                }
                let runtime = SessionRuntime {
                    tx: self.tx,
                    shared: self.shared,
                    generation: self.generation,
                    process,
                };
                if runtime.unverified().is_some() {
                    LaunchWait::Recovery(runtime)
                } else {
                    LaunchWait::Ready(runtime)
                }
            }
            Ok(Err(reason)) => LaunchWait::Failed(reason),
            Err(RecvTimeoutError::Timeout) => LaunchWait::Pending(self),
            Err(RecvTimeoutError::Disconnected) => {
                LaunchWait::Failed("the session thread ended before reporting its start".into())
            }
        }
    }

    pub fn cancel(&self) {
        self.shared.stop_requested.store(true, Ordering::Release);
    }

    pub fn canceller(&self) -> LaunchCancel {
        LaunchCancel(Arc::clone(&self.shared))
    }

    pub fn generation(&self) -> SessionGeneration {
        self.generation
    }
}

impl SessionRuntime {
    pub fn launch(
        spec: SpawnSpec,
        generation: SessionGeneration,
        observer: RuntimeObserver,
    ) -> Result<LaunchHandle, SpawnError> {
        let (tx, rx) = sync_channel::<Message>(OUTPUT_QUEUE_SLOTS);
        let (startup_tx, startup_rx) = sync_channel::<Result<ProcessIdentity, String>>(1);
        let shared = Shared::new();
        let thread_shared = Arc::clone(&shared);
        let thread_tx = tx.clone();
        std::thread::Builder::new()
            .name("paneflow-host-session".into())
            .spawn(move || {
                run(
                    spec,
                    generation,
                    thread_tx,
                    rx,
                    thread_shared,
                    observer,
                    startup_tx,
                )
            })
            .map_err(|e| SpawnError(format!("could not start the session thread: {e}")))?;
        Ok(LaunchHandle {
            tx,
            shared,
            generation,
            startup_rx,
        })
    }

    pub fn spawn(
        spec: SpawnSpec,
        generation: SessionGeneration,
        observer: RuntimeObserver,
    ) -> Result<Self, SpawnError> {
        match Self::launch(spec, generation, observer)?.wait(STARTUP_DEADLINE) {
            LaunchWait::Ready(runtime) => Ok(runtime),
            LaunchWait::Recovery(runtime) => {
                let reason = runtime
                    .unverified()
                    .unwrap_or_else(|| "startup needs recovery".into());
                let _ = runtime.stop();
                Err(SpawnError(reason))
            }
            LaunchWait::Failed(reason) => Err(SpawnError(reason)),
            LaunchWait::Pending(handle) => {
                handle.cancel();
                Err(SpawnError("the session did not start in time".to_string()))
            }
        }
    }

    pub fn generation(&self) -> SessionGeneration {
        self.generation
    }

    pub fn process(&self) -> ProcessIdentity {
        self.process
    }

    pub fn exit(&self) -> Option<ExitOutcome> {
        self.shared.exit()
    }

    pub fn unverified(&self) -> Option<String> {
        self.shared.unverified()
    }

    pub fn descendants_unresolved(&self) -> usize {
        self.shared.descendants_unresolved.load(Ordering::Acquire)
    }

    pub fn output_changed_at_ms(&self) -> Option<u64> {
        let changed_at = self.shared.output_changed_at_ms.load(Ordering::Acquire);
        (changed_at != 0).then_some(changed_at)
    }

    pub fn is_live(&self) -> bool {
        self.shared.exit().is_none() && self.shared.unverified().is_none()
    }

    pub fn owns_process(&self) -> bool {
        self.shared.exit().is_none() || self.descendants_unresolved() > 0
    }

    fn send_bounded(&self, message: Message, budget: Duration) -> Result<(), RuntimeError> {
        use std::sync::mpsc::TrySendError;
        let deadline = Instant::now() + budget;
        let mut message = message;
        loop {
            match self.tx.try_send(message) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Disconnected(_)) => return Err(RuntimeError::Gone),
                Err(TrySendError::Full(returned)) => {
                    if Instant::now() >= deadline {
                        return Err(RuntimeError::Deadline(budget));
                    }
                    message = returned;
                    std::thread::sleep(QUEUE_RETRY);
                }
            }
        }
    }

    fn ask<T>(
        &self,
        build: impl FnOnce(SyncSender<Result<T, RuntimeError>>) -> Command,
    ) -> Result<T, RuntimeError> {
        self.ask_within(REQUEST_DEADLINE, build)
    }

    fn ask_within<T>(
        &self,
        budget: Duration,
        build: impl FnOnce(SyncSender<Result<T, RuntimeError>>) -> Command,
    ) -> Result<T, RuntimeError> {
        let (reply_tx, reply_rx) = sync_channel(1);
        self.send_bounded(Message::Command(build(reply_tx)), budget)?;
        match reply_rx.recv_timeout(budget) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => Err(RuntimeError::Deadline(budget)),
            Err(RecvTimeoutError::Disconnected) => Err(RuntimeError::Gone),
        }
    }

    pub fn checkpoint(&self) -> Result<Checkpoint, RuntimeError> {
        self.ask(Command::Checkpoint)
    }

    pub fn text(&self) -> Result<String, RuntimeError> {
        self.ask(Command::Text)
    }

    pub fn viewport_scan(&self, budget: Duration) -> Result<ViewportScan, RuntimeError> {
        self.ask_within(budget, Command::Viewport)
    }

    pub fn bracketed_paste_enabled(&self) -> Result<bool, RuntimeError> {
        self.ask(Command::BracketedPaste)
    }

    pub fn output(&self, from: u64, max: usize) -> Result<OutputSlice, RuntimeError> {
        self.ask(|reply| Command::Output { from, max, reply })
    }

    pub fn input(&self, bytes: Vec<u8>) -> Result<usize, RuntimeError> {
        if !self.is_live() {
            return Err(RuntimeError::NotLive);
        }
        self.ask(|reply| Command::Input(bytes, reply))
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), RuntimeError> {
        self.ask(|reply| Command::Resize { cols, rows, reply })
    }

    pub fn stop(&self) -> Result<StopReport, RuntimeError> {
        self.stop_until(Instant::now() + STOP_BUDGET)
    }

    pub fn stop_until(&self, deadline: Instant) -> Result<StopReport, RuntimeError> {
        self.shared.stop_requested.store(true, Ordering::Release);
        let (reply_tx, reply_rx) = sync_channel(1);
        let remaining = deadline.saturating_duration_since(Instant::now());
        self.send_bounded(
            Message::Command(Command::Stop {
                deadline,
                reply: reply_tx,
            }),
            remaining,
        )?;
        match reply_rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(report) => Ok(report),
            Err(RecvTimeoutError::Timeout) => Err(RuntimeError::Deadline(STOP_BUDGET)),
            Err(RecvTimeoutError::Disconnected) => Err(RuntimeError::Gone),
        }
    }

    #[cfg(test)]
    pub(crate) fn inject_panic(&self) {
        let _ = self.send_bounded(Message::Command(Command::InjectPanic), REQUEST_DEADLINE);
    }
}

#[cfg(unix)]
use crate::process::UnixProcessTreeOwner as ProcessTreeOwner;
#[cfg(windows)]
use crate::process::WindowsProcessTreeOwner as ProcessTreeOwner;

struct Session {
    terminal: ghostty::DisplayTerminal,
    tail: OutputTail,
    writer: Option<SyncSender<Vec<u8>>>,
    master: Option<Box<dyn portable_pty::MasterPty + Send>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    child_pid: u32,
    process_tree: ProcessTreeOwner,
    next_process_scan: Instant,
    cols: u16,
    rows: u16,
    reader_eof: bool,
    exit: Option<ExitOutcome>,
    descendants_unresolved: usize,
    generation: SessionGeneration,
    observer: RuntimeObserver,
    shared: Arc<Shared>,
}

fn run(
    spec: SpawnSpec,
    generation: SessionGeneration,
    tx: SyncSender<Message>,
    rx: Receiver<Message>,
    shared: Arc<Shared>,
    observer: RuntimeObserver,
    startup_tx: SyncSender<Result<ProcessIdentity, String>>,
) {
    let mut session = match start(spec, generation, tx, &shared, observer) {
        Ok(session) => session,
        Err(reason) => {
            let _ = startup_tx.send(Err(reason));
            return;
        }
    };
    if shared.stop_requested.load(Ordering::Acquire) {
        session.mark_unverified(LATE_LAUNCH_CANCELLED.to_string());
    }
    if startup_tx
        .send(Ok(ProcessIdentity::capture(session.child_pid)))
        .is_err()
    {
        recover_without_requester(&mut session);
        return;
    }
    if shared.unverified().is_some() {
        recovery_loop(&mut session, &rx);
        return;
    }
    let served = catch_unwind(AssertUnwindSafe(|| serve_loop(&mut session, &rx)));
    match served {
        Ok(()) => session.shutdown(),
        Err(_) => {
            session.mark_unverified(RUNTIME_PANIC_REASON.to_string());
            recovery_loop(&mut session, &rx);
        }
    }
}

fn serve_loop(session: &mut Session, rx: &Receiver<Message>) {
    loop {
        match rx.recv_timeout(RUNTIME_TICK) {
            Ok(Message::Output(chunk)) => session.feed(&chunk),
            Ok(Message::Eof) => session.reader_eof = true,
            Ok(Message::Command(command)) => session.handle(command),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        session.observe_exit();
    }
}

fn recovery_loop(session: &mut Session, rx: &Receiver<Message>) {
    loop {
        let message = match rx.recv() {
            Ok(message) => message,
            Err(_) => break,
        };
        let Message::Command(command) = message else {
            continue;
        };
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            session.handle_while_unverified(command)
        }));
        if outcome.is_err() {
            log::error!(
                "paneflow-host: the recovery owner of pid={} panicked again",
                session.child_pid
            );
        }
        if stop_is_confirmed(&session.report()) {
            break;
        }
    }
    if !stop_is_confirmed(&session.report()) {
        recover_without_requester(session);
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        session.writer = None;
        session.release_master();
    }));
}

fn stop_is_confirmed(report: &StopReport) -> bool {
    report.exit.is_some() && report.unverified.is_none() && report.descendants_unresolved == 0
}

fn recover_without_requester(session: &mut Session) {
    loop {
        let outcome = catch_unwind(AssertUnwindSafe(|| session.terminate()));
        if outcome.as_ref().is_ok_and(stop_is_confirmed) {
            return;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn new_terminal(spec: &SpawnSpec) -> Result<ghostty::DisplayTerminal, String> {
    let size = ghostty::WindowSize::new(
        usize::from(spec.cols),
        usize::from(spec.rows),
        CELL_WIDTH_PX,
        CELL_HEIGHT_PX,
    )
    .map_err(|e| format!("invalid terminal size: {e}"))?;
    let mut terminal = ghostty::DisplayTerminal::new(
        size,
        spec.scrollback_lines,
        ghostty::TerminalAppearance::default(),
    )
    .map_err(|e| format!("terminal engine initialization failed: {e}"))?;
    terminal
        .set_continuation_max_bytes(CONTINUATION_MAX_BYTES)
        .map_err(|e| format!("continuation tracking could not be enabled: {e}"))?;
    if let Err(error) = terminal.set_terminfo_name(TERMINFO_NAME) {
        log::warn!("paneflow-host: terminfo name could not be configured: {error}");
    }
    let scrollback_bytes = spec
        .scrollback_lines
        .saturating_mul(SCROLLBACK_BYTES_PER_LINE)
        .min(MAX_SCROLLBACK_BYTES);
    if let Err(error) = terminal.set_scrollback_max_bytes(Some(scrollback_bytes)) {
        log::warn!("paneflow-host: scrollback budget could not be configured: {error}");
    }
    Ok(terminal)
}

fn start(
    spec: SpawnSpec,
    generation: SessionGeneration,
    tx: SyncSender<Message>,
    shared: &Arc<Shared>,
    observer: RuntimeObserver,
) -> Result<Session, String> {
    let terminal = new_terminal(&spec)?;

    let pair = crate::pty::open(pty_size(spec.cols, spec.rows))
        .map_err(|e| format!("failed to open a native PTY: {e}"))?;
    let mut command = CommandBuilder::new(&spec.shell);
    command.args(&spec.args);
    command.cwd(&spec.cwd);
    for key in crate::env::inherited_env_keys_to_strip() {
        command.env_remove(&key);
    }
    for (key, value) in &spec.env {
        command.env(key, value);
    }
    #[cfg(debug_assertions)]
    if let Some(delay) = spec
        .env
        .get("PANEFLOW_TEST_SPAWN_DELAY_MS")
        .and_then(|delay| delay.parse::<u64>().ok())
    {
        std::thread::sleep(Duration::from_millis(delay.min(30_000)));
    }
    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|e| format!("failed to spawn {} in the PTY: {e}", spec.shell))?;
    let child_pid = child.process_id().unwrap_or(0);
    drop(pair.slave);
    let mut session = Session {
        terminal,
        tail: OutputTail::new(MAX_OUTPUT_TAIL_BYTES),
        writer: None,
        master: Some(pair.master),
        child,
        child_pid,
        process_tree: ProcessTreeOwner::new(ProcessIdentity::capture(child_pid)),
        next_process_scan: Instant::now(),
        cols: spec.cols,
        rows: spec.rows,
        reader_eof: false,
        exit: None,
        descendants_unresolved: 0,
        generation,
        observer,
        shared: Arc::clone(shared),
    };
    let wired = catch_unwind(AssertUnwindSafe(|| {
        #[cfg(test)]
        if spec.env.contains_key("PANEFLOW_TEST_WIRING_PANIC") {
            panic!("injected PTY wiring panic");
        }
        #[cfg(test)]
        if spec.env.contains_key("PANEFLOW_TEST_WIRING_FAILURE") {
            return Err("injected PTY thread creation failure".into());
        }
        session
            .master
            .as_ref()
            .ok_or_else(|| "the PTY master is missing".to_string())
            .and_then(|master| wire_pty(master.as_ref(), tx))
    }))
    .unwrap_or_else(|_| Err("PTY wiring panicked after child creation".into()));
    if ProcessIdentity::capture(child_pid).started_at.is_none() {
        session.mark_unverified("the spawned child identity could not be captured".into());
    }
    match wired {
        Ok(writer) => session.writer = Some(writer),
        Err(reason) => session.mark_unverified(reason),
    }
    Ok(session)
}

fn wire_pty(
    master: &(dyn portable_pty::MasterPty + Send),
    tx: SyncSender<Message>,
) -> Result<SyncSender<Vec<u8>>, String> {
    let reader = master
        .try_clone_reader()
        .map_err(|e| format!("failed to clone the PTY reader: {e}"))?;
    let writer = master
        .take_writer()
        .map_err(|e| format!("failed to take the PTY writer: {e}"))?;
    std::thread::Builder::new()
        .name("paneflow-host-pty-reader".into())
        .spawn(move || read_pty(reader, tx))
        .map_err(|e| format!("failed to start the PTY reader: {e}"))?;
    let (input_tx, input_rx) = sync_channel::<Vec<u8>>(INPUT_QUEUE_SLOTS);
    std::thread::Builder::new()
        .name("paneflow-host-pty-writer".into())
        .spawn(move || write_pty_queue(writer, input_rx))
        .map_err(|e| format!("failed to start the PTY writer: {e}"))?;
    Ok(input_tx)
}

fn read_pty(mut reader: Box<dyn Read + Send>, tx: SyncSender<Message>) {
    let mut buffer = vec![0u8; READ_CHUNK_BYTES];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                if tx.send(Message::Output(buffer[..read].to_vec())).is_err() {
                    return;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::yield_now();
            }
            Err(_) => break,
        }
    }
    let _ = tx.send(Message::Eof);
}

fn write_pty_queue(mut writer: Box<dyn Write + Send>, rx: Receiver<Vec<u8>>) {
    while let Ok(bytes) = rx.recv() {
        if let Err(error) = writer.write_all(&bytes).and_then(|()| writer.flush()) {
            log::debug!("paneflow-host: PTY write failed, closing the input queue: {error}");
            break;
        }
    }
}

impl Session {
    fn feed(&mut self, chunk: &[u8]) {
        self.tail.append(chunk);
        self.shared
            .output_changed_at_ms
            .store(crate::manifest::now_ms(), Ordering::Release);
        if let Err(error) = self.terminal.feed(chunk) {
            log::warn!("paneflow-host: terminal feed failed: {error}");
        }
        for event in self.terminal.drain_events() {
            match event {
                ghostty::BackendEvent::WritePty(bytes) => {
                    if let Err(error) = self.write_pty(&bytes) {
                        log::debug!("paneflow-host: terminal reply to the PTY failed: {error}");
                    }
                }
                ghostty::BackendEvent::Title(title) => (self.observer)(RuntimeNotice::Title(title)),
                ghostty::BackendEvent::WorkingDirectory(cwd) => {
                    (self.observer)(RuntimeNotice::WorkingDirectory(cwd));
                }
                ghostty::BackendEvent::ClipboardStore(_)
                | ghostty::BackendEvent::Bell
                | ghostty::BackendEvent::DesktopNotification { .. }
                | ghostty::BackendEvent::Progress(_) => {}
                ghostty::BackendEvent::UnknownSequence { .. }
                | ghostty::BackendEvent::CallbackPanicked
                | ghostty::BackendEvent::InputDropped { .. }
                | ghostty::BackendEvent::EffectsOverflow { .. } => {
                    log::debug!("paneflow-host: terminal engine event ignored: {event:?}");
                }
            }
        }
    }

    fn write_pty(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        use std::sync::mpsc::TrySendError;
        let Some(writer) = self.writer.as_ref() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "the PTY writer is closed",
            ));
        };
        match writer.try_send(bytes.to_vec()) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "the child is not reading its input; the input queue is full",
            )),
            Err(TrySendError::Disconnected(_)) => {
                self.writer = None;
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "the PTY writer is closed",
                ))
            }
        }
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Checkpoint(reply) => {
                let _ = reply.send(self.checkpoint());
            }
            Command::Text(reply) => {
                let _ = reply.send(self.text());
            }
            Command::Viewport(reply) => {
                let _ = reply.send(self.viewport_scan());
            }
            Command::BracketedPaste(reply) => {
                let modes = self
                    .terminal
                    .modes()
                    .map_err(|e| RuntimeError::Engine(e.to_string()));
                let _ = reply.send(modes.map(|modes| modes.bracketed_paste));
            }
            Command::Output { from, max, reply } => {
                let _ = reply.send(self.output(from, max));
            }
            Command::Input(bytes, reply) => {
                let result = if self.exit.is_some() || self.writer.is_none() {
                    Err(RuntimeError::NotLive)
                } else {
                    self.write_pty(&bytes)
                        .map(|()| bytes.len())
                        .map_err(|e| RuntimeError::Pty(e.to_string()))
                };
                let _ = reply.send(result);
            }
            Command::Resize { cols, rows, reply } => {
                let _ = reply.send(self.resize(cols, rows));
            }
            Command::Stop { deadline, reply } => {
                let report = self.terminate_until(deadline);
                let _ = reply.send(report);
            }
            #[cfg(test)]
            Command::InjectPanic => panic!("injected runtime panic"),
        }
    }

    fn handle_while_unverified(&mut self, command: Command) {
        let reason = self
            .shared
            .unverified()
            .unwrap_or_else(|| RUNTIME_PANIC_REASON.to_string());
        match command {
            Command::Stop { deadline, reply } => {
                let report = self.terminate_until(deadline);
                let _ = reply.send(report);
            }
            Command::Checkpoint(reply) => {
                let _ = reply.send(Err(RuntimeError::Unverified(reason)));
            }
            Command::Text(reply) => {
                let _ = reply.send(Err(RuntimeError::Unverified(reason)));
            }
            Command::Viewport(reply) => {
                let _ = reply.send(Err(RuntimeError::Unverified(reason)));
            }
            Command::BracketedPaste(reply) => {
                let _ = reply.send(Err(RuntimeError::Unverified(reason)));
            }
            Command::Output { reply, .. } => {
                let _ = reply.send(Err(RuntimeError::Unverified(reason)));
            }
            Command::Input(_, reply) => {
                let _ = reply.send(Err(RuntimeError::Unverified(reason)));
            }
            Command::Resize { reply, .. } => {
                let _ = reply.send(Err(RuntimeError::Unverified(reason)));
            }
            #[cfg(test)]
            Command::InjectPanic => panic!("injected runtime panic"),
        }
    }

    fn checkpoint(&mut self) -> Result<Checkpoint, RuntimeError> {
        let bytes = self
            .terminal
            .encode_snapshot_size()
            .map_err(|e| RuntimeError::Engine(e.to_string()))?;
        if bytes > MAX_CHECKPOINT_BYTES {
            return Err(RuntimeError::CheckpointTooLarge {
                bytes,
                limit: MAX_CHECKPOINT_BYTES,
            });
        }
        let snapshot = self
            .terminal
            .encode_snapshot()
            .map_err(|e| RuntimeError::Engine(e.to_string()))?;
        if snapshot.len() > MAX_CHECKPOINT_BYTES {
            return Err(RuntimeError::CheckpointTooLarge {
                bytes: snapshot.len(),
                limit: MAX_CHECKPOINT_BYTES,
            });
        }
        Ok(Checkpoint {
            generation: self.generation,
            offset: self.tail.end_offset(),
            cols: self.cols,
            rows: self.rows,
            snapshot,
        })
    }

    fn text(&mut self) -> Result<String, RuntimeError> {
        let history = self
            .terminal
            .extract_scrollback()
            .map_err(|e| RuntimeError::Engine(e.to_string()))?;
        let screen = self
            .terminal
            .format(ghostty::FormatterOptions::plain_text())
            .map_err(|e| RuntimeError::Engine(e.to_string()))?;
        let screen = screen.trim_end_matches([NEWLINE_CHAR, ' ']).to_string();
        Ok(match (history, screen.is_empty()) {
            (Some(history), false) => [history, screen].join(NEWLINE),
            (Some(history), true) => history,
            (None, false) => screen,
            (None, true) => String::new(),
        })
    }

    fn viewport_scan(&mut self) -> Result<ViewportScan, RuntimeError> {
        let screen = self
            .terminal
            .format(ghostty::FormatterOptions::plain_text())
            .map_err(|e| RuntimeError::Engine(e.to_string()))?;
        Ok(ViewportScan {
            screen,
            foreground_process_group: self.foreground_process_group(),
        })
    }

    #[cfg(unix)]
    fn foreground_process_group(&self) -> Option<i32> {
        self.master
            .as_ref()
            .and_then(|master| master.process_group_leader())
    }

    #[cfg(not(unix))]
    fn foreground_process_group(&self) -> Option<i32> {
        None
    }

    fn output(&self, from: u64, max: usize) -> Result<OutputSlice, RuntimeError> {
        let (offset, data) =
            self.tail
                .read_from(from, max)
                .map_err(|evicted| RuntimeError::OutputEvicted {
                    requested: evicted.requested,
                    tail_start: evicted.tail_start,
                    tail_end: evicted.tail_end,
                })?;
        Ok(OutputSlice {
            offset,
            data,
            end_offset: self.tail.end_offset(),
            live: self.exit.is_none() || !self.reader_eof,
        })
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), RuntimeError> {
        if cols == 0 || rows == 0 {
            return Err(RuntimeError::Engine(
                "terminal size must be non-zero".to_string(),
            ));
        }
        if let Some(master) = self.master.as_ref() {
            master
                .resize(pty_size(cols, rows))
                .map_err(|e| RuntimeError::Pty(e.to_string()))?;
        }
        let size = ghostty::WindowSize::new(
            usize::from(cols),
            usize::from(rows),
            CELL_WIDTH_PX,
            CELL_HEIGHT_PX,
        )
        .map_err(|e| RuntimeError::Engine(e.to_string()))?;
        self.terminal
            .resize(size)
            .map_err(|e| RuntimeError::Engine(e.to_string()))?;
        self.cols = cols;
        self.rows = rows;
        Ok(())
    }

    fn observe_exit(&mut self) {
        if Instant::now() >= self.next_process_scan {
            self.process_tree.discover();
            self.next_process_scan = Instant::now() + Duration::from_millis(500);
        }
        if self.exit.is_some() {
            let before = self.descendants_unresolved;
            self.refresh_descendants();
            if before > 0 && self.descendants_unresolved == 0 {
                self.shared.set_unverified(None);
                if let Some(outcome) = &self.exit {
                    (self.observer)(RuntimeNotice::Exited(outcome.clone()));
                }
            }
            return;
        }
        if !self.process_tree.root_is_running() {
            self.process_tree.discover();
        }
        match self.child.try_wait() {
            Ok(Some(status)) => {
                self.process_tree.discover();
                self.refresh_descendants();
                self.record_exit(exit_outcome(&status));
            }
            Ok(None) => {}
            Err(error) => self.mark_unverified(format!("child wait failed: {error}")),
        }
    }

    fn refresh_descendants(&mut self) {
        self.descendants_unresolved = self.process_tree.unresolved();
        self.shared
            .descendants_unresolved
            .store(self.descendants_unresolved, Ordering::Release);
    }

    fn mark_unverified(&mut self, reason: String) {
        if self.shared.unverified().as_deref() == Some(reason.as_str()) {
            return;
        }
        log::warn!(
            "paneflow-host: child pid={} ownership is unverified: {reason}",
            self.child_pid
        );
        self.shared.set_unverified(Some(reason.clone()));
        (self.observer)(RuntimeNotice::Unverified(reason));
    }

    fn record_exit(&mut self, outcome: ExitOutcome) {
        #[cfg(target_os = "macos")]
        self.process_tree.root_reaped();
        self.exit = Some(outcome.clone());
        self.shared.set_unverified(None);
        *self
            .shared
            .exit
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(outcome.clone());
        self.writer = None;
        self.release_master();
        if self.descendants_unresolved == 0 {
            (self.observer)(RuntimeNotice::Exited(outcome));
        } else {
            self.mark_unverified(format!(
                "{} descendant process(es) remain unresolved",
                self.descendants_unresolved
            ));
        }
    }

    fn release_master(&mut self) {
        if let Some(master) = self.master.take() {
            let _ = std::thread::Builder::new()
                .name("paneflow-host-pty-closer".into())
                .spawn(move || drop(master));
        }
    }

    fn report(&self) -> StopReport {
        StopReport {
            exit: self.exit.clone(),
            descendants_unresolved: self.descendants_unresolved,
            unverified: self.shared.unverified(),
        }
    }

    fn terminate(&mut self) -> StopReport {
        self.terminate_until(Instant::now() + STOP_BUDGET)
    }

    fn terminate_until(&mut self, deadline: Instant) -> StopReport {
        self.writer = None;
        #[cfg(windows)]
        let _ = self.process_tree.terminate(deadline);
        #[cfg(unix)]
        self.process_tree.signal(false);
        #[cfg(unix)]
        let force_at = Instant::now() + Duration::from_millis(100);
        loop {
            #[cfg(unix)]
            if Instant::now() >= force_at {
                self.process_tree.signal(true);
            }
            if self.exit.is_none() {
                match self.child.try_wait() {
                    Ok(Some(status)) => {
                        self.refresh_descendants();
                        self.record_exit(exit_outcome(&status));
                    }
                    Ok(None) => {}
                    Err(error) => {
                        self.mark_unverified(format!(
                            "child wait after termination failed: {error}"
                        ));
                        return self.report();
                    }
                }
            }
            self.refresh_descendants();
            if self.exit.is_some() && self.descendants_unresolved == 0 {
                self.shared.set_unverified(None);
                if let Some(outcome) = &self.exit {
                    (self.observer)(RuntimeNotice::Exited(outcome.clone()));
                }
                return self.report();
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(
                Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
        self.mark_unverified(format!(
            "child pid={} or {} descendant process(es) remain unresolved after termination",
            self.child_pid, self.descendants_unresolved
        ));
        self.report()
    }

    fn shutdown(mut self) {
        if !stop_is_confirmed(&self.report()) {
            recover_without_requester(&mut self);
        }
        self.writer = None;
        self.release_master();
    }
}

fn exit_outcome(status: &portable_pty::ExitStatus) -> ExitOutcome {
    ExitOutcome {
        code: i64::from(status.exit_code()),
        signal: status.signal().map(str::to_owned),
    }
}

fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: u16::try_from(u32::from(cols) * CELL_WIDTH_PX).unwrap_or(u16::MAX),
        pixel_height: u16::try_from(u32::from(rows) * CELL_HEIGHT_PX).unwrap_or(u16::MAX),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn echo_shell_spec(cols: u16, rows: u16) -> SpawnSpec {
        #[cfg(windows)]
        let (shell, args) = (
            "cmd.exe".to_string(),
            vec!["/Q".to_string(), "/D".to_string()],
        );
        #[cfg(unix)]
        let (shell, args) = ("/bin/sh".to_string(), Vec::new());
        SpawnSpec {
            shell,
            args,
            cwd: std::env::temp_dir(),
            env: BTreeMap::from([("TERM".to_string(), "xterm-256color".to_string())]),
            cols,
            rows,
            scrollback_lines: 500,
        }
    }

    fn wait_for_output(runtime: &SessionRuntime, marker: &str, from: u64) -> (u64, Vec<u8>) {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut collected = Vec::new();
        let mut offset = from;
        while Instant::now() < deadline {
            let slice = runtime.output(offset, 64 * 1024).expect("output readable");
            collected.extend_from_slice(&slice.data);
            offset = slice.offset + slice.data.len() as u64;
            if String::from_utf8_lossy(&collected).contains(marker) {
                return (offset, collected);
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        panic!(
            "marker {marker:?} never appeared; got {:?}",
            String::from_utf8_lossy(&collected)
        );
    }

    #[test]
    fn a_session_runs_a_shell_answers_input_and_checkpoints_atomically() {
        let notices = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&notices);
        let observer: RuntimeObserver = Arc::new(move |notice| sink.lock().unwrap().push(notice));
        let runtime =
            SessionRuntime::spawn(echo_shell_spec(80, 24), SessionGeneration::FIRST, observer)
                .expect("shell spawns");
        assert!(runtime.is_live());
        assert!(runtime.process().pid != 0);

        runtime
            .input(b"echo PANEFLOW_HOST_ALPHA\r\n".to_vec())
            .expect("input accepted");
        let (offset, _) = wait_for_output(&runtime, "PANEFLOW_HOST_ALPHA", 0);

        let checkpoint = runtime.checkpoint().expect("checkpoint");
        assert!(
            checkpoint.offset + 32 >= offset,
            "offset covers the echoed output"
        );
        assert!(!checkpoint.snapshot.is_empty());
        assert_eq!((checkpoint.cols, checkpoint.rows), (80, 24));
        let mut decoder =
            ghostty::SnapshotDecoder::from_bytes(&checkpoint.snapshot).expect("snapshot decodes");
        let terminal = decoder
            .decode(ghostty::SnapshotRestore {
                cell_width: 8,
                cell_height: 16,
                max_scrollback: 500,
                appearance: ghostty::TerminalAppearance::default(),
            })
            .expect("terminal restores from the checkpoint");
        let content = terminal.snapshot().expect("restored screen renders");
        let text: String = content.cells.iter().map(|cell| cell.character).collect();
        assert!(
            text.contains("PANEFLOW_HOST_ALPHA"),
            "restored screen must contain the echoed marker; got {text:?}"
        );

        runtime.resize(100, 30).expect("resize");
        let after = runtime.checkpoint().expect("checkpoint after resize");
        assert_eq!((after.cols, after.rows), (100, 30));
        assert!(
            after.offset >= checkpoint.offset,
            "offsets never go backwards"
        );

        let report = runtime.stop().expect("stop completes");
        assert!(report.exit.is_some(), "stop records an exit outcome");
        assert_eq!(report.unverified, None);
        assert!(!runtime.is_live());
        assert_eq!(runtime.input(b"x".to_vec()), Err(RuntimeError::NotLive));
        let observed = notices.lock().unwrap();
        assert!(
            observed
                .iter()
                .any(|n| matches!(n, RuntimeNotice::Exited(_))),
            "the observer sees the exit"
        );
    }

    #[test]
    fn startup_wiring_failures_and_panics_retain_a_stoppable_owner() {
        for fault in ["PANEFLOW_TEST_WIRING_FAILURE", "PANEFLOW_TEST_WIRING_PANIC"] {
            let mut spec = echo_shell_spec(80, 24);
            spec.env.insert(fault.into(), "1".into());
            let launch =
                SessionRuntime::launch(spec, SessionGeneration::FIRST, Arc::new(|_| {})).unwrap();
            let LaunchWait::Recovery(runtime) = launch.wait(STARTUP_DEADLINE) else {
                panic!("wiring fault must retain the native child");
            };
            assert!(runtime.owns_process());
            let identity = runtime.process();
            assert!(stop_is_confirmed(&runtime.stop().unwrap()));
            assert!(!identity.is_provably_live());
        }
    }

    #[test]
    fn a_startup_deadline_then_cancellation_retains_the_late_child() {
        let mut spec = echo_shell_spec(80, 24);
        spec.env
            .insert("PANEFLOW_TEST_SPAWN_DELAY_MS".into(), "100".into());
        let launch =
            SessionRuntime::launch(spec, SessionGeneration::FIRST, Arc::new(|_| {})).unwrap();
        let LaunchWait::Pending(launch) = launch.wait(Duration::from_millis(10)) else {
            panic!("spawn must remain pending after its caller deadline");
        };
        launch.cancel();
        let LaunchWait::Recovery(runtime) = launch.wait(STARTUP_DEADLINE) else {
            panic!("late child must transfer to recovery");
        };
        let identity = runtime.process();
        assert!(stop_is_confirmed(&runtime.stop().unwrap()));
        assert!(!identity.is_provably_live());
    }

    #[test]
    fn a_checkpoint_survives_a_partial_escape_sequence() {
        let mut terminal = new_terminal(&echo_shell_spec(80, 24)).expect("terminal");
        terminal.feed(b"\x1b[1;2").expect("partial CSI parses");
        let snapshot = terminal
            .encode_snapshot()
            .expect("continuation tracking lets the checkpoint capture the pending bytes");
        assert!(!snapshot.is_empty());
        assert_eq!(
            terminal.continuation().expect("continuation readback"),
            Some(b"\x1b[1;2".to_vec())
        );
    }

    #[test]
    fn a_restarted_generation_is_carried_into_its_checkpoints() {
        let observer: RuntimeObserver = Arc::new(|_| {});
        let generation = SessionGeneration::FIRST.next();
        let runtime = SessionRuntime::spawn(echo_shell_spec(80, 24), generation, observer)
            .expect("shell spawns");
        assert_eq!(runtime.generation(), generation);
        assert_eq!(
            runtime.checkpoint().expect("checkpoint").generation,
            generation
        );
        let _ = runtime.stop();
    }

    #[test]
    fn output_before_the_tail_is_reported_as_evicted_not_fabricated() {
        let observer: RuntimeObserver = Arc::new(|_| {});
        let runtime =
            SessionRuntime::spawn(echo_shell_spec(80, 24), SessionGeneration::FIRST, observer)
                .expect("shell spawns");
        let checkpoint = runtime.checkpoint().expect("checkpoint");
        let beyond = checkpoint.offset + 1_000_000;
        assert!(matches!(
            runtime.output(beyond, 1024),
            Err(RuntimeError::OutputEvicted { requested, .. }) if requested == beyond
        ));
        let _ = runtime.stop();
    }

    #[test]
    fn a_cancelled_launch_terminates_the_late_child_instead_of_publishing_it() {
        let observer: RuntimeObserver = Arc::new(|_| {});
        let handle =
            SessionRuntime::launch(echo_shell_spec(80, 24), SessionGeneration::FIRST, observer)
                .expect("the session thread starts");
        handle.cancel();
        let outcome = handle.wait(Duration::from_secs(30));
        match outcome {
            LaunchWait::Recovery(runtime) => {
                assert!(!runtime.is_live());
                assert!(runtime.owns_process());
                let process = runtime.process();
                let report = runtime.stop().expect("the recovery owner accepts stop");
                assert!(stop_is_confirmed(&report), "{report:?}");
                assert!(!process.is_provably_live());
            }
            LaunchWait::Failed(reason) => {
                panic!("a created child must transfer to its recovery owner: {reason}")
            }
            LaunchWait::Ready(_) => panic!("a cancelled launch must never publish a runtime"),
            LaunchWait::Pending(_) => panic!("the cancellation must resolve within the budget"),
        }
    }

    #[test]
    fn a_runtime_panic_keeps_the_child_owned_until_an_explicit_stop_confirms_its_exit() {
        let notices = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&notices);
        let observer: RuntimeObserver = Arc::new(move |notice| sink.lock().unwrap().push(notice));
        let runtime =
            SessionRuntime::spawn(echo_shell_spec(80, 24), SessionGeneration::FIRST, observer)
                .expect("shell spawns");
        let process = runtime.process();
        runtime.inject_panic();
        let deadline = Instant::now() + Duration::from_secs(5);
        while runtime.unverified().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            runtime.unverified().as_deref(),
            Some(RUNTIME_PANIC_REASON),
            "a panic leaves the session unverified, never exited"
        );
        assert!(runtime.exit().is_none());
        assert!(
            process.is_provably_live(),
            "the child survives the runtime panic under a recovery owner"
        );
        assert!(matches!(
            runtime.checkpoint(),
            Err(RuntimeError::Unverified(_))
        ));
        let report = runtime.stop().expect("the recovery owner answers a stop");
        assert!(report.exit.is_some(), "the stop confirms the child outcome");
        assert!(!process.is_provably_live());
        assert!(
            notices
                .lock()
                .unwrap()
                .iter()
                .any(|n| matches!(n, RuntimeNotice::Unverified(_))),
            "the observer saw the unverified transition"
        );
    }
}
