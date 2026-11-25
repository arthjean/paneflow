use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};

use paneflow_browser_proto::{
    Command as HostCommand, CommandError, ENDPOINT_ARG, Event, ExitReason, Hello, HostMessage,
    OriginDecision, PROFILES_ARG, PROTOCOL_VERSION, ProfileName, READY_LINE, Reply, RequestId,
    TabId, WindowMessage, check_version, read_message, write_message,
};

use crate::transport::{self, Stream};
use crate::{ClientEvent, HOST_STOPPED_MESSAGE, HostConfig, TabStatus, Ticket, host_environment};

const STDERR_TAIL_BYTES: usize = 8 * 1024;

type Answer = Sender<Result<Reply, CommandError>>;

struct Pending {
    answer: Answer,
    opens: Option<TabId>,
}

struct TabRecord {
    profile: ProfileName,
    url: String,
    status: TabStatus,
    unknown_to_host: bool,
}

enum HostState {
    Stopped,
    Starting {
        generation: u64,
        queue: Vec<(RequestId, HostCommand)>,
    },
    Running {
        generation: u64,
        pid: u32,
        writer: Sender<WindowMessage>,
    },
}

struct State {
    host: HostState,
    tabs: BTreeMap<TabId, TabRecord>,
    pending: HashMap<RequestId, Pending>,
    next_tab: u64,
    next_request: u64,
    next_generation: u64,
}

struct Shared {
    config: HostConfig,
    state: Mutex<State>,
    events: Sender<ClientEvent>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn emit(&self, event: ClientEvent) {
        let _ = self.events.send(event);
    }
}

#[derive(Clone)]
pub struct BrowserClient {
    shared: Arc<Shared>,
}

impl BrowserClient {
    pub fn new(config: HostConfig) -> (Self, Receiver<ClientEvent>) {
        let (events, receiver) = mpsc::channel();
        let shared = Arc::new(Shared {
            config,
            state: Mutex::new(State {
                host: HostState::Stopped,
                tabs: BTreeMap::new(),
                pending: HashMap::new(),
                next_tab: 1,
                next_request: 1,
                next_generation: 1,
            }),
            events,
        });
        (Self { shared }, receiver)
    }

    pub fn host_pid(&self) -> Option<u32> {
        match self.shared.lock().host {
            HostState::Running { pid, .. } => Some(pid),
            _ => None,
        }
    }

    pub fn tab_status(&self, tab: TabId) -> Option<TabStatus> {
        self.shared
            .lock()
            .tabs
            .get(&tab)
            .map(|record| record.status.clone())
    }

    pub fn tabs(&self) -> Vec<TabId> {
        self.shared.lock().tabs.keys().copied().collect()
    }

    pub fn open_tab(&self, profile: ProfileName, url: impl Into<String>) -> (TabId, Ticket) {
        let url = url.into();
        let mut state = self.shared.lock();
        let tab = TabId(state.next_tab);
        state.next_tab += 1;
        state.tabs.insert(
            tab,
            TabRecord {
                profile: profile.clone(),
                url: url.clone(),
                status: TabStatus::Starting,
                unknown_to_host: false,
            },
        );
        let ticket = self.submit(&mut state, HostCommand::OpenTab { tab, profile, url });
        (tab, ticket)
    }

    pub fn navigate(&self, tab: TabId, url: impl Into<String>) -> Ticket {
        let url = url.into();
        let mut state = self.shared.lock();
        if let Some(record) = state.tabs.get_mut(&tab) {
            record.url = url.clone();
        }
        self.submit_for_tab(&mut state, tab, HostCommand::Navigate { tab, url })
    }

    pub fn reload(&self, tab: TabId) -> Ticket {
        let mut state = self.shared.lock();
        if let Some(record) = state
            .tabs
            .get_mut(&tab)
            .filter(|record| record.unknown_to_host)
        {
            record.status = TabStatus::Starting;
            record.unknown_to_host = false;
            let command = HostCommand::OpenTab {
                tab,
                profile: record.profile.clone(),
                url: record.url.clone(),
            };
            return self.submit(&mut state, command);
        }
        self.submit_for_tab(&mut state, tab, HostCommand::Reload { tab })
    }

    pub fn close_tab(&self, tab: TabId) -> Ticket {
        let mut state = self.shared.lock();
        let unknown_to_host = state
            .tabs
            .get(&tab)
            .is_some_and(|record| record.unknown_to_host);
        if unknown_to_host || matches!(state.host, HostState::Stopped) {
            return match state.tabs.remove(&tab) {
                Some(_) => answered(Ok(Reply::Done)),
                None => answered(Err(CommandError::TabClosed { tab })),
            };
        }
        self.submit_for_tab(&mut state, tab, HostCommand::CloseTab { tab })
    }

    pub fn devtools(
        &self,
        tab: TabId,
        method: impl Into<String>,
        params: serde_json::Value,
    ) -> Ticket {
        let mut state = self.shared.lock();
        let command = HostCommand::DevTools {
            tab,
            method: method.into(),
            params,
        };
        self.submit_for_tab(&mut state, tab, command)
    }

    pub fn decide_origin(
        &self,
        tab: TabId,
        origin: impl Into<String>,
        decision: OriginDecision,
    ) -> Ticket {
        let mut state = self.shared.lock();
        let command = HostCommand::DecideOrigin {
            tab,
            origin: origin.into(),
            decision,
        };
        self.submit_for_tab(&mut state, tab, command)
    }

    pub fn shutdown(&self) -> Ticket {
        let mut state = self.shared.lock();
        if matches!(state.host, HostState::Stopped) {
            return answered(Ok(Reply::Done));
        }
        self.submit(&mut state, HostCommand::Shutdown)
    }

    fn submit_for_tab(&self, state: &mut State, tab: TabId, command: HostCommand) -> Ticket {
        let unknown_to_host = state
            .tabs
            .get(&tab)
            .is_some_and(|record| record.unknown_to_host);
        if unknown_to_host || matches!(state.host, HostState::Stopped) {
            let Some(record) = state.tabs.get(&tab) else {
                return answered(Err(CommandError::TabClosed { tab }));
            };
            let error = match &record.status {
                TabStatus::Crashed { reason } | TabStatus::Failed { reason } => {
                    CommandError::TabCrashed {
                        tab,
                        reason: reason.clone(),
                    }
                }
                _ => CommandError::HostUnavailable {
                    reason: HOST_STOPPED_MESSAGE.to_string(),
                },
            };
            return answered(Err(error));
        }
        self.submit(state, command)
    }

    fn submit(&self, state: &mut State, command: HostCommand) -> Ticket {
        let id = RequestId(state.next_request);
        state.next_request += 1;
        let (answer, receiver) = mpsc::channel();
        let opens = match &command {
            HostCommand::OpenTab { tab, .. } => Some(*tab),
            _ => None,
        };
        state.pending.insert(id, Pending { answer, opens });
        match &mut state.host {
            HostState::Running { writer, .. } => {
                let _ = writer.send(WindowMessage::Request { id, command });
            }
            HostState::Starting { queue, .. } => queue.push((id, command)),
            HostState::Stopped => {
                let generation = state.next_generation;
                state.next_generation += 1;
                state.host = HostState::Starting {
                    generation,
                    queue: vec![(id, command)],
                };
                let shared = Arc::clone(&self.shared);
                let spawned = std::thread::Builder::new()
                    .name("browser-host-start".into())
                    .spawn(move || start_host(&shared, generation));
                if let Err(error) = spawned {
                    let reason = format!("cannot start the browser engine thread: {error}");
                    fail_start(&self.shared, state, generation, &reason);
                }
            }
        }
        Ticket::new(receiver)
    }
}

fn answered(result: Result<Reply, CommandError>) -> Ticket {
    let (answer, receiver) = mpsc::channel();
    let _ = answer.send(result);
    Ticket::new(receiver)
}

struct Started {
    child: Child,
    pid: u32,
    stream: Stream,
    stderr_tail: Arc<Mutex<VecDeque<u8>>>,
}

fn start_host(shared: &Arc<Shared>, generation: u64) {
    match launch(&shared.config, generation) {
        Ok(started) => adopt(shared, generation, started),
        Err(reason) => {
            let mut state = shared.lock();
            fail_start(shared, &mut state, generation, &reason);
        }
    }
}

fn fail_start(shared: &Shared, state: &mut State, generation: u64, reason: &str) {
    if !matches!(&state.host, HostState::Starting { generation: current, .. } if *current == generation)
    {
        return;
    }
    let HostState::Starting { queue, .. } = std::mem::replace(&mut state.host, HostState::Stopped)
    else {
        return;
    };
    let error = CommandError::HostUnavailable {
        reason: reason.to_string(),
    };
    for (id, command) in queue {
        if let Some(pending) = state.pending.remove(&id) {
            let _ = pending.answer.send(Err(error.clone()));
        }
        if let HostCommand::OpenTab { tab, .. } = command
            && let Some(record) = state.tabs.get_mut(&tab)
        {
            record.status = TabStatus::Failed {
                reason: reason.to_string(),
            };
            record.unknown_to_host = true;
        }
    }
    shared.emit(ClientEvent::HostFailed {
        reason: reason.to_string(),
    });
}

fn launch(config: &HostConfig, generation: u64) -> Result<Started, String> {
    if !config.binary.is_file() {
        return Err(format!(
            "the browser engine is not installed ({} does not exist)",
            config.binary.display()
        ));
    }
    let endpoint =
        paneflow_home::browser_endpoint_path(&config.home, std::process::id(), generation);
    let profiles = config.profiles_dir();
    let mut command = Command::new(&config.binary);
    command
        .arg(format!("{ENDPOINT_ARG}={}", endpoint.display()))
        .arg(format!("{PROFILES_ARG}={}", profiles.display()))
        .env_clear()
        .envs(host_environment(config.window_environment.clone()))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    transport::detach(&mut command);
    let mut child = command.spawn().map_err(|error| {
        format!(
            "cannot start the browser engine {}: {error}",
            config.binary.display()
        )
    })?;
    let pid = child.id();
    let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL_BYTES)));
    if let Some(stderr) = child.stderr.take() {
        drain_stderr(stderr, Arc::clone(&stderr_tail));
    }
    let ready = child.stdout.take().map(wait_ready);
    let ready = match ready {
        Some(receiver) => receiver.recv_timeout(config.start_deadline).ok(),
        None => None,
    };
    if ready != Some(true) {
        let _ = child.kill();
        let status = child.wait().ok();
        return Err(start_failure(
            "the browser engine did not get ready",
            status,
            &stderr_tail,
        ));
    }
    let handshake = connect_and_greet(&endpoint, config);
    match handshake {
        Ok(stream) => Ok(Started {
            child,
            pid,
            stream,
            stderr_tail,
        }),
        Err(reason) => {
            let status = match child.try_wait() {
                Ok(Some(status)) => Some(status),
                _ => {
                    let _ = child.kill();
                    child.wait().ok()
                }
            };
            Err(start_failure(&reason, status, &stderr_tail))
        }
    }
}

fn start_failure(
    reason: &str,
    status: Option<std::process::ExitStatus>,
    stderr_tail: &Mutex<VecDeque<u8>>,
) -> String {
    let tail = stderr_text(stderr_tail);
    let mut message = reason.to_string();
    if let Some(status) = status {
        message.push_str(&format!(" ({status})"));
    }
    if let Some(line) = tail.lines().rev().find(|line| !line.trim().is_empty()) {
        message.push_str(": ");
        message.push_str(line.trim());
    }
    message
}

fn stderr_text(stderr_tail: &Mutex<VecDeque<u8>>) -> String {
    let bytes: Vec<u8> = stderr_tail
        .lock()
        .map(|tail| tail.iter().copied().collect())
        .unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn drain_stderr(mut stderr: impl Read + Send + 'static, tail: Arc<Mutex<VecDeque<u8>>>) {
    let _ = std::thread::Builder::new()
        .name("browser-host-stderr".into())
        .spawn(move || {
            let mut buffer = [0u8; 4096];
            while let Ok(read) = stderr.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                let chunk = buffer.get(..read).unwrap_or_default();
                log::debug!(
                    "browser host: {}",
                    String::from_utf8_lossy(chunk).trim_end()
                );
                if let Ok(mut tail) = tail.lock() {
                    tail.extend(chunk.iter().copied());
                    let excess = tail.len().saturating_sub(STDERR_TAIL_BYTES);
                    tail.drain(..excess);
                }
            }
        });
}

fn wait_ready(stdout: impl Read + Send + 'static) -> Receiver<bool> {
    let (ready, receiver) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("browser-host-stdout".into())
        .spawn(move || {
            let mut signalled = false;
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if !signalled && line.trim() == READY_LINE {
                    signalled = true;
                    let _ = ready.send(true);
                } else {
                    log::debug!("browser host: {line}");
                }
            }
            if !signalled {
                let _ = ready.send(false);
            }
        });
    receiver
}

fn connect_and_greet(endpoint: &std::path::Path, config: &HostConfig) -> Result<Stream, String> {
    let mut stream = transport::connect(endpoint).map_err(|error| {
        format!(
            "cannot reach the browser engine at {}: {error}",
            endpoint.display()
        )
    })?;
    write_message(
        &mut stream,
        &WindowMessage::Hello(Hello::with_idle_exit(config.idle_exit)),
    )
    .map_err(|error| format!("cannot greet the browser engine: {error}"))?;
    let _ = transport::set_read_timeout(&stream, Some(config.start_deadline));
    let answer = read_message::<HostMessage, _>(&mut stream)
        .map_err(|error| format!("the browser engine did not answer its handshake: {error}"))?;
    let _ = transport::set_read_timeout(&stream, None);
    match answer {
        HostMessage::Welcome(welcome) => {
            check_version(PROTOCOL_VERSION, welcome.protocol_version)
                .map_err(|refusal| refusal.to_string())?;
            Ok(stream)
        }
        HostMessage::Refused(refusal) => Err(refusal.to_string()),
        other => Err(format!(
            "the browser engine answered its handshake with {other:?}"
        )),
    }
}

fn adopt(shared: &Arc<Shared>, generation: u64, started: Started) {
    let Started {
        mut child,
        pid,
        stream,
        stderr_tail,
    } = started;
    let (writer, outgoing) = mpsc::channel::<WindowMessage>();
    let write_half = transport::try_clone(&stream);
    let Ok(mut write_half) = write_half else {
        let _ = child.kill();
        let _ = child.wait();
        let mut state = shared.lock();
        fail_start(
            shared,
            &mut state,
            generation,
            "cannot split the browser engine connection",
        );
        return;
    };
    let spawned_writer = std::thread::Builder::new()
        .name("browser-host-writer".into())
        .spawn(move || {
            for message in outgoing {
                if write_message(&mut write_half, &message).is_err() {
                    break;
                }
            }
        });
    if spawned_writer.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        let mut state = shared.lock();
        fail_start(
            shared,
            &mut state,
            generation,
            "cannot start the browser engine writer",
        );
        return;
    }

    {
        let mut state = shared.lock();
        let queue = match std::mem::replace(
            &mut state.host,
            HostState::Running {
                generation,
                pid,
                writer: writer.clone(),
            },
        ) {
            HostState::Starting { queue, .. } => queue,
            _ => Vec::new(),
        };
        for (id, command) in queue {
            let _ = writer.send(WindowMessage::Request { id, command });
        }
    }
    shared.emit(ClientEvent::HostStarted { pid });

    let reader_shared = Arc::clone(shared);
    let spawned_reader = std::thread::Builder::new()
        .name("browser-host-reader".into())
        .spawn(move || {
            let mut stream = stream;
            let mut exiting = None;
            while let Ok(message) = read_message::<HostMessage, _>(&mut stream) {
                if let HostMessage::Event(Event::Exiting { reason }) = &message {
                    exiting = Some(*reason);
                }
                deliver(&reader_shared, message);
            }
            transport::close(&stream);
            let status = child.wait().ok();
            host_gone(&reader_shared, generation, exiting, status, &stderr_tail);
        });
    if spawned_reader.is_err() {
        log::warn!("browser host: cannot start the reader thread");
    }
}

fn deliver(shared: &Shared, message: HostMessage) {
    match message {
        HostMessage::Reply { id, outcome } => {
            let result = outcome.into_result();
            let answer = {
                let mut state = shared.lock();
                let pending = state.pending.remove(&id);
                if let (
                    Some(Pending {
                        opens: Some(tab), ..
                    }),
                    Err(error),
                ) = (&pending, &result)
                    && let Some(record) = state.tabs.get_mut(tab)
                {
                    record.status = TabStatus::Failed {
                        reason: error.to_string(),
                    };
                    record.unknown_to_host = true;
                }
                pending.map(|pending| pending.answer)
            };
            if let Some(answer) = answer {
                let _ = answer.send(result);
            }
        }
        HostMessage::Event(event) => {
            record_event(&mut shared.lock(), &event);
            shared.emit(ClientEvent::Browser(event));
        }
        HostMessage::Welcome(_) | HostMessage::Refused(_) => {}
    }
}

fn record_event(state: &mut State, event: &Event) {
    let Some(tab) = event.tab() else { return };
    if let Event::Closed { .. } = event {
        state.tabs.remove(&tab);
        return;
    }
    let Some(record) = state.tabs.get_mut(&tab) else {
        return;
    };
    let status = match event {
        Event::LoadStarted { url, .. } => TabStatus::Loading { url: url.clone() },
        Event::Loaded {
            url, http_status, ..
        } => TabStatus::Loaded {
            url: url.clone(),
            http_status: *http_status,
        },
        Event::OriginDecisionRequested { origin, url, .. } => TabStatus::AwaitingDecision {
            origin: origin.clone(),
            url: url.clone(),
        },
        Event::NavigationRefused { url, refusal, .. } => TabStatus::Refused {
            url: url.clone(),
            refusal: refusal.clone(),
        },
        Event::Crashed { reason, .. } => TabStatus::Crashed {
            reason: reason.clone(),
        },
        _ => return,
    };
    if let TabStatus::Loading { url } | TabStatus::Loaded { url, .. } = &status
        && url != "about:blank"
    {
        record.url = url.clone();
    }
    record.status = status;
}

fn host_gone(
    shared: &Shared,
    generation: u64,
    exiting: Option<ExitReason>,
    status: Option<std::process::ExitStatus>,
    stderr_tail: &Mutex<VecDeque<u8>>,
) {
    let mut crashed = Vec::new();
    let reason = {
        let mut state = shared.lock();
        let current =
            matches!(&state.host, HostState::Running { generation: g, .. } if *g == generation);
        if !current {
            return;
        }
        state.host = HostState::Stopped;
        let reason = match exiting {
            Some(_) => HOST_STOPPED_MESSAGE.to_string(),
            None => start_failure(HOST_STOPPED_MESSAGE, status, stderr_tail),
        };
        for (_, pending) in state.pending.drain() {
            let _ = pending.answer.send(Err(CommandError::HostUnavailable {
                reason: reason.clone(),
            }));
        }
        for (tab, record) in state.tabs.iter_mut() {
            record.status = TabStatus::Crashed {
                reason: reason.clone(),
            };
            record.unknown_to_host = true;
            crashed.push(*tab);
        }
        reason
    };
    for tab in crashed {
        shared.emit(ClientEvent::Browser(Event::Crashed {
            tab,
            reason: reason.clone(),
        }));
    }
    match exiting {
        Some(reason) => shared.emit(ClientEvent::HostExited { reason }),
        None => shared.emit(ClientEvent::HostCrashed { reason }),
    }
}
