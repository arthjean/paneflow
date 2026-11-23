use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use paneflow_process::{ProcError, StdioSession};
use ropey::Rope;
use serde_json::{json, Value};

use crate::connection::{Connection, ConnectionEvent, ResponseError, CONNECTION_CLOSED};
use crate::encoding::position_at;
use crate::protocol::{
    initialize_params, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, Negotiated, Range, SyncKind, TextDocumentContentChangeEvent,
    TextDocumentIdentifier, TextDocumentItem, VersionedTextDocumentIdentifier,
};
use crate::resolve::resolve_command;

pub const EXIT_GRACE: Duration = Duration::from_secs(5);
pub const SHUTDOWN_REPLY_GRACE: Duration = Duration::from_secs(5);
const CRASH_STDERR_LINES: usize = 8;
const CRASH_STDERR_DRAIN: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ServerKey {
    pub server_id: String,
    pub root: PathBuf,
}

impl fmt::Display for ServerKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} for {}", self.server_id, self.root.display())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ServerSpec {
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub initialization_options: Option<Value>,
    pub settings: Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerState {
    Starting,
    Running,
    Missing { command: String },
    Crashed { stderr: Vec<String> },
    Failed { message: String },
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocumentId(u64);

#[derive(Clone, Debug)]
pub struct OpenDocument {
    pub path: PathBuf,
    pub language_id: String,
    pub text: Rope,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextChange {
    pub start_byte: usize,
    pub old_end_byte: usize,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientEvent {
    State { key: ServerKey, state: ServerState },
    Unsupported { document: DocumentId },
    Stopped { key: ServerKey, killed: bool },
}

pub type ClientEventSink = Arc<dyn Fn(ClientEvent) + Send + Sync>;

struct Shared {
    states: Mutex<BTreeMap<ServerKey, ServerState>>,
    negotiated: Mutex<BTreeMap<ServerKey, Negotiated>>,
    unsupported: Mutex<HashSet<DocumentId>>,
}

#[derive(Clone)]
pub struct LanguageClient {
    commands: Sender<Message>,
    shared: Arc<Shared>,
    next_document: Arc<AtomicU64>,
}

impl LanguageClient {
    pub fn start(events: ClientEventSink) -> io::Result<Self> {
        let (commands, inbox) = mpsc::channel();
        let shared = Arc::new(Shared {
            states: Mutex::new(BTreeMap::new()),
            negotiated: Mutex::new(BTreeMap::new()),
            unsupported: Mutex::new(HashSet::new()),
        });
        let manager = Manager {
            outbox: commands.clone(),
            shared: shared.clone(),
            events,
            servers: BTreeMap::new(),
            documents: HashMap::new(),
            quitting: false,
        };
        thread::Builder::new()
            .name("paneflow-lsp-client".to_string())
            .spawn(move || manager.run(&inbox))?;
        Ok(Self {
            commands,
            shared,
            next_document: Arc::new(AtomicU64::new(1)),
        })
    }

    pub fn open_document(
        &self,
        key: ServerKey,
        spec: ServerSpec,
        document: OpenDocument,
    ) -> DocumentId {
        let id = DocumentId(self.next_document.fetch_add(1, Ordering::Relaxed));
        self.send(Message::Open {
            id,
            key,
            spec,
            document,
        });
        id
    }

    pub fn change_document(&self, id: DocumentId, changes: Vec<TextChange>, text: Rope) {
        self.send(Message::Change { id, changes, text });
    }

    pub fn replace_document(&self, id: DocumentId, text: Rope) {
        self.send(Message::Replace { id, text });
    }

    pub fn close_document(&self, id: DocumentId) {
        self.send(Message::Close { id });
    }

    pub fn report_missing(&self, key: ServerKey, command: String) {
        self.send(Message::Missing { key, command });
    }

    pub fn state(&self, key: &ServerKey) -> Option<ServerState> {
        self.shared
            .states
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .cloned()
    }

    pub fn negotiated(&self, key: &ServerKey) -> Option<Negotiated> {
        self.shared
            .negotiated
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .cloned()
    }

    pub fn is_unsupported(&self, id: DocumentId) -> bool {
        self.shared
            .unsupported
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&id)
    }

    pub fn shutdown_all(&self, budget: Duration) {
        let (done, finished) = mpsc::channel();
        self.send(Message::ShutdownAll { budget, done });
        let _ = finished.recv_timeout(budget.saturating_add(Duration::from_secs(1)));
    }

    fn send(&self, message: Message) {
        if self.commands.send(message).is_err() {
            log::warn!("the language client manager stopped; a request was dropped");
        }
    }
}

enum Message {
    Open {
        id: DocumentId,
        key: ServerKey,
        spec: ServerSpec,
        document: OpenDocument,
    },
    Change {
        id: DocumentId,
        changes: Vec<TextChange>,
        text: Rope,
    },
    Replace {
        id: DocumentId,
        text: Rope,
    },
    Close {
        id: DocumentId,
    },
    Missing {
        key: ServerKey,
        command: String,
    },
    ShutdownAll {
        budget: Duration,
        done: Sender<()>,
    },
    Initialized {
        key: ServerKey,
        generation: u64,
        result: Result<Value, ResponseError>,
    },
    Connection {
        key: ServerKey,
        generation: u64,
        event: ConnectionEvent,
    },
}

struct Running {
    session: StdioSession,
    connection: Connection,
    negotiated: Option<Negotiated>,
}

struct Slot {
    spec: ServerSpec,
    state: ServerState,
    generation: u64,
    failures: u32,
    running: Option<Running>,
}

struct Document {
    key: ServerKey,
    uri: String,
    language_id: String,
    version: i32,
    text: Rope,
    opened_on: Option<u64>,
}

struct Manager {
    outbox: Sender<Message>,
    shared: Arc<Shared>,
    events: ClientEventSink,
    servers: BTreeMap<ServerKey, Slot>,
    documents: HashMap<DocumentId, Document>,
    quitting: bool,
}

impl Manager {
    fn run(mut self, inbox: &Receiver<Message>) {
        while let Ok(message) = inbox.recv() {
            match message {
                Message::Open {
                    id,
                    key,
                    spec,
                    document,
                } => self.open(id, key, spec, document),
                Message::Change { id, changes, text } => self.change(id, &changes, text),
                Message::Replace { id, text } => self.replace(id, text),
                Message::Close { id } => self.close(id),
                Message::Missing { key, command } => self.missing(key, command),
                Message::ShutdownAll { budget, done } => {
                    self.shutdown_all(budget);
                    let _ = done.send(());
                }
                Message::Initialized {
                    key,
                    generation,
                    result,
                } => self.initialized(&key, generation, result),
                Message::Connection {
                    key,
                    generation,
                    event,
                } => self.connection_event(&key, generation, event),
            }
        }
    }

    fn set_state(&mut self, key: &ServerKey, state: ServerState) {
        let Some(slot) = self.servers.get_mut(key) else {
            return;
        };
        if slot.state == state {
            return;
        }
        slot.state = state.clone();
        self.shared
            .states
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.clone(), state.clone());
        if !matches!(state, ServerState::Running) {
            self.shared
                .negotiated
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(key);
        }
        (self.events)(ClientEvent::State {
            key: key.clone(),
            state,
        });
    }

    fn open(&mut self, id: DocumentId, key: ServerKey, spec: ServerSpec, document: OpenDocument) {
        if self.quitting {
            return;
        }
        let Some(uri) = file_uri(&document.path) else {
            log::warn!(
                "{key}: {} has no file URI; no language server is involved",
                document.path.display()
            );
            return;
        };
        self.documents.insert(
            id,
            Document {
                key: key.clone(),
                uri,
                language_id: document.language_id,
                version: 1,
                text: document.text,
                opened_on: None,
            },
        );
        let slot = self.servers.entry(key.clone()).or_insert_with(|| Slot {
            spec: spec.clone(),
            state: ServerState::Stopped,
            generation: 0,
            failures: 0,
            running: None,
        });
        match slot.state {
            ServerState::Stopped => {
                slot.spec = spec;
                slot.failures = 0;
                self.start(&key);
            }
            ServerState::Running => self.open_on_server(id),
            ServerState::Starting
            | ServerState::Missing { .. }
            | ServerState::Crashed { .. }
            | ServerState::Failed { .. } => {}
        }
    }

    fn missing(&mut self, key: ServerKey, command: String) {
        let slot = self.servers.entry(key.clone()).or_insert_with(|| Slot {
            spec: ServerSpec {
                command: command.clone(),
                args: Vec::new(),
                env: Vec::new(),
                initialization_options: None,
                settings: Value::Null,
            },
            state: ServerState::Stopped,
            generation: 0,
            failures: 0,
            running: None,
        });
        if slot.running.is_none() {
            self.set_state(&key, ServerState::Missing { command });
        }
    }

    fn start(&mut self, key: &ServerKey) {
        let Some(slot) = self.servers.get_mut(key) else {
            return;
        };
        slot.generation += 1;
        let generation = slot.generation;
        let spec = slot.spec.clone();
        let program = match resolve_command(&spec.command) {
            Ok(program) => program,
            Err(error) => {
                log::info!("{key}: {error}");
                self.set_state(
                    key,
                    ServerState::Missing {
                        command: spec.command,
                    },
                );
                return;
            }
        };
        let mut command = Command::new(&program);
        command.args(&spec.args).current_dir(&key.root);
        for (name, value) in &spec.env {
            command.env(name, value);
        }
        let mut session = match paneflow_process::spawn_stdio_session(command) {
            Ok(session) => session,
            Err(ProcError::NotFound(error)) => {
                log::info!("{key}: {} could not be started: {error}", program.display());
                self.set_state(
                    key,
                    ServerState::Missing {
                        command: spec.command,
                    },
                );
                return;
            }
            Err(error) => {
                log::warn!("{key}: {} could not be started: {error}", program.display());
                self.set_state(
                    key,
                    ServerState::Failed {
                        message: error.to_string(),
                    },
                );
                return;
            }
        };
        let (Some(stdin), Some(stdout)) = (session.take_stdin(), session.take_stdout()) else {
            session.kill();
            self.set_state(
                key,
                ServerState::Failed {
                    message: "the server's standard streams were not available".to_string(),
                },
            );
            return;
        };
        let outbox = Mutex::new(self.outbox.clone());
        let event_key = key.clone();
        let connection = match Connection::start(
            &key.server_id,
            spec.settings.clone(),
            stdout,
            stdin,
            Box::new(move |event| {
                let _ = outbox.lock().unwrap_or_else(PoisonError::into_inner).send(
                    Message::Connection {
                        key: event_key.clone(),
                        generation,
                        event,
                    },
                );
            }),
        ) {
            Ok(connection) => connection,
            Err(error) => {
                session.kill();
                self.set_state(
                    key,
                    ServerState::Failed {
                        message: error.to_string(),
                    },
                );
                return;
            }
        };
        let root_uri = directory_uri(&key.root).unwrap_or_default();
        let root_name = key
            .root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| key.root.display().to_string());
        let outbox = Mutex::new(self.outbox.clone());
        let init_key = key.clone();
        connection.request(
            "initialize",
            initialize_params(&root_uri, &root_name, spec.initialization_options.as_ref()),
            Box::new(move |result| {
                let _ = outbox.lock().unwrap_or_else(PoisonError::into_inner).send(
                    Message::Initialized {
                        key: init_key,
                        generation,
                        result,
                    },
                );
            }),
        );
        if let Some(slot) = self.servers.get_mut(key) {
            slot.running = Some(Running {
                session,
                connection,
                negotiated: None,
            });
        }
        self.set_state(key, ServerState::Starting);
    }

    fn initialized(
        &mut self,
        key: &ServerKey,
        generation: u64,
        result: Result<Value, ResponseError>,
    ) {
        let Some(slot) = self.servers.get_mut(key) else {
            return;
        };
        if slot.generation != generation || slot.running.is_none() {
            return;
        }
        let negotiated = match result {
            Ok(result) => {
                Negotiated::from_initialize_result(&result).map_err(|error| error.to_string())
            }
            Err(error) if error.code == CONNECTION_CLOSED => return,
            Err(error) => Err(error.message),
        };
        let negotiated = match negotiated {
            Ok(negotiated) => negotiated,
            Err(message) => {
                log::warn!("{key}: initialize failed: {message}");
                if let Some(mut running) = slot.running.take() {
                    running.connection.close();
                    running.session.kill();
                }
                self.set_state(key, ServerState::Failed { message });
                return;
            }
        };
        if let Some(running) = slot.running.as_mut() {
            running.connection.notify("initialized", json!({}));
            running.negotiated = Some(negotiated.clone());
        }
        self.set_state(key, ServerState::Running);
        self.shared
            .negotiated
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.clone(), negotiated);
        let mut documents: Vec<DocumentId> = self
            .documents
            .iter()
            .filter(|(_, document)| &document.key == key)
            .map(|(id, _)| *id)
            .collect();
        documents.sort();
        for id in documents {
            self.open_on_server(id);
        }
    }

    fn open_on_server(&mut self, id: DocumentId) {
        let Some(document) = self.documents.get_mut(&id) else {
            return;
        };
        let Some(slot) = self.servers.get(&document.key) else {
            return;
        };
        let Some((running, negotiated)) = slot
            .running
            .as_ref()
            .and_then(|running| Some((running, running.negotiated.as_ref()?)))
        else {
            return;
        };
        if !negotiated.sync.offers_sync() {
            log::debug!("{}: the server offers no document sync", document.key);
            let newly = self
                .shared
                .unsupported
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(id);
            if newly {
                (self.events)(ClientEvent::Unsupported { document: id });
            }
            return;
        }
        document.opened_on = Some(slot.generation);
        if negotiated.sync.open_close {
            let params = DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: document.uri.clone(),
                    language_id: document.language_id.clone(),
                    version: document.version,
                    text: document.text.to_string(),
                },
            };
            running
                .connection
                .notify("textDocument/didOpen", to_value(&params));
        }
    }

    fn live_server(&self, id: DocumentId) -> Option<(&Running, &Negotiated)> {
        let document = self.documents.get(&id)?;
        let slot = self.servers.get(&document.key)?;
        let running = slot.running.as_ref()?;
        let negotiated = running.negotiated.as_ref()?;
        (document.opened_on == Some(slot.generation)).then_some((running, negotiated))
    }

    fn change(&mut self, id: DocumentId, changes: &[TextChange], text: Rope) {
        let message = match self.live_server(id) {
            Some((running, negotiated)) => {
                let document = self.documents.get(&id);
                document.and_then(|document| {
                    let content_changes = match negotiated.sync.change {
                        SyncKind::None => return None,
                        SyncKind::Full => vec![TextDocumentContentChangeEvent {
                            range: None,
                            text: text.to_string(),
                        }],
                        SyncKind::Incremental => {
                            incremental_changes(&document.text, changes, negotiated)
                        }
                    };
                    Some((
                        running.connection.clone(),
                        document.uri.clone(),
                        content_changes,
                    ))
                })
            }
            None => None,
        };
        let Some(document) = self.documents.get_mut(&id) else {
            return;
        };
        document.text = text;
        if let Some((connection, uri, content_changes)) = message {
            document.version += 1;
            let params = DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri,
                    version: document.version,
                },
                content_changes,
            };
            connection.notify("textDocument/didChange", to_value(&params));
        }
    }

    fn replace(&mut self, id: DocumentId, text: Rope) {
        let message = self.live_server(id).and_then(|(running, negotiated)| {
            (negotiated.sync.change != SyncKind::None).then(|| running.connection.clone())
        });
        let Some(document) = self.documents.get_mut(&id) else {
            return;
        };
        document.text = text;
        if let Some(connection) = message {
            document.version += 1;
            let params = DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: document.uri.clone(),
                    version: document.version,
                },
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: None,
                    text: document.text.to_string(),
                }],
            };
            connection.notify("textDocument/didChange", to_value(&params));
        }
    }

    fn close(&mut self, id: DocumentId) {
        let close_message = self.live_server(id).and_then(|(running, negotiated)| {
            negotiated
                .sync
                .open_close
                .then(|| running.connection.clone())
        });
        self.shared
            .unsupported
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
        let Some(document) = self.documents.remove(&id) else {
            return;
        };
        if let Some(connection) = close_message {
            let params = DidCloseTextDocumentParams {
                text_document: TextDocumentIdentifier { uri: document.uri },
            };
            connection.notify("textDocument/didClose", to_value(&params));
        }
        let key = document.key;
        if self.documents.values().any(|document| document.key == key) {
            return;
        }
        let Some(slot) = self.servers.get_mut(&key) else {
            return;
        };
        let running = slot.running.take();
        slot.generation += 1;
        slot.failures = 0;
        if let Some(running) = running {
            let events = self.events.clone();
            let stop_key = key.clone();
            let spawned = thread::Builder::new()
                .name("paneflow-lsp-stop".to_string())
                .spawn(move || {
                    let killed = stop(running, SHUTDOWN_REPLY_GRACE, EXIT_GRACE);
                    events(ClientEvent::Stopped {
                        key: stop_key,
                        killed,
                    });
                });
            if let Err(error) = spawned {
                log::warn!("{key}: could not start the stop thread: {error}");
            }
        }
        self.set_state(&key, ServerState::Stopped);
    }

    fn connection_event(&mut self, key: &ServerKey, generation: u64, event: ConnectionEvent) {
        let Some(slot) = self.servers.get_mut(key) else {
            return;
        };
        if slot.generation != generation || slot.running.is_none() {
            return;
        }
        match event {
            ConnectionEvent::Closed(cause) => {
                let stderr = slot
                    .running
                    .take()
                    .map(|mut running| {
                        running.connection.close();
                        let tail = running.session.stderr_tail();
                        running.session.kill();
                        tail.wait_closed(CRASH_STDERR_DRAIN);
                        tail.last_lines(CRASH_STDERR_LINES)
                    })
                    .unwrap_or_default();
                slot.failures += 1;
                let failures = slot.failures;
                for document in self.documents.values_mut() {
                    if &document.key == key {
                        document.opened_on = None;
                    }
                }
                if failures == 1 && !self.quitting {
                    log::warn!(
                        "{key}: the server stopped unexpectedly ({cause}); restarting it once"
                    );
                    self.start(key);
                } else {
                    log::warn!("{key}: the server stopped unexpectedly again ({cause}); leaving it stopped");
                    self.set_state(key, ServerState::Crashed { stderr });
                }
            }
            ConnectionEvent::Notification { method, .. } => {
                log::debug!("{key}: ignored notification {method}");
            }
            ConnectionEvent::SemanticTokensRefresh => {
                log::debug!("{key}: semantic token refresh requested");
            }
        }
    }

    fn shutdown_all(&mut self, budget: Duration) {
        self.quitting = true;
        let deadline = Instant::now() + budget;
        let half = budget / 2;
        let mut stops: Vec<JoinHandle<()>> = Vec::new();
        let keys: Vec<ServerKey> = self.servers.keys().cloned().collect();
        for key in keys {
            let Some(slot) = self.servers.get_mut(&key) else {
                continue;
            };
            slot.generation += 1;
            let Some(running) = slot.running.take() else {
                continue;
            };
            let events = self.events.clone();
            let stop_key = key.clone();
            match thread::Builder::new()
                .name("paneflow-lsp-quit".to_string())
                .spawn(move || {
                    let killed = stop(running, half, half);
                    events(ClientEvent::Stopped {
                        key: stop_key,
                        killed,
                    });
                }) {
                Ok(handle) => stops.push(handle),
                Err(error) => log::warn!("{key}: could not stop the server: {error}"),
            }
            self.set_state(&key, ServerState::Stopped);
        }
        while stops.iter().any(|handle| !handle.is_finished()) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
    }
}

fn stop(mut running: Running, reply_grace: Duration, exit_grace: Duration) -> bool {
    let (replied, reply) = mpsc::channel();
    let sent = running.connection.request(
        "shutdown",
        Value::Null,
        Box::new(move |result| {
            let _ = replied.send(result.is_ok());
        }),
    );
    if sent.is_some() {
        let _ = reply.recv_timeout(reply_grace);
    }
    running.connection.notify("exit", Value::Null);
    running.connection.close();
    let exited = running.session.wait_for_exit(exit_grace).is_some();
    running.session.kill();
    !exited
}

fn incremental_changes(
    before: &Rope,
    changes: &[TextChange],
    negotiated: &Negotiated,
) -> Vec<TextDocumentContentChangeEvent> {
    let mut text = before.clone();
    let mut events = Vec::with_capacity(changes.len());
    for change in changes {
        let end = change.old_end_byte.min(text.len_bytes());
        let start = change.start_byte.min(end);
        events.push(TextDocumentContentChangeEvent {
            range: Some(Range {
                start: position_at(&text, start, negotiated.encoding),
                end: position_at(&text, end, negotiated.encoding),
            }),
            text: change.text.clone(),
        });
        let start_char = text.byte_to_char(start);
        let end_char = text.byte_to_char(end);
        text.remove(start_char..end_char);
        text.insert(start_char, &change.text);
    }
    events
}

fn to_value<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

pub fn file_uri(path: &Path) -> Option<String> {
    url::Url::from_file_path(path).ok().map(String::from)
}

pub fn directory_uri(path: &Path) -> Option<String> {
    url::Url::from_directory_path(path).ok().map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{PositionEncoding, TextSync};

    fn negotiated(encoding: PositionEncoding) -> Negotiated {
        Negotiated {
            encoding,
            sync: TextSync {
                open_close: true,
                change: SyncKind::Incremental,
            },
            semantic_tokens: None,
        }
    }

    #[test]
    fn a_batch_of_edits_becomes_ranges_computed_on_the_text_each_edit_sees() {
        let before = Rope::from_str("let 🦀 = a;\nb\n");
        let crab_end = "let 🦀".len();
        let changes = vec![
            TextChange {
                start_byte: crab_end + 3,
                old_end_byte: crab_end + 4,
                text: "xyz".to_string(),
            },
            TextChange {
                start_byte: 0,
                old_end_byte: 0,
                text: "\n".to_string(),
            },
        ];
        let utf16 = incremental_changes(&before, &changes, &negotiated(PositionEncoding::Utf16));
        assert_eq!(utf16[0].range.unwrap().start.character, 9);
        assert_eq!(utf16[0].range.unwrap().end.character, 10);
        assert_eq!(utf16[0].text, "xyz");
        assert_eq!(utf16[1].range.unwrap().start.line, 0);
        let utf8 = incremental_changes(&before, &changes, &negotiated(PositionEncoding::Utf8));
        assert_eq!(utf8[0].range.unwrap().start.character, 11);
    }

    #[test]
    fn a_path_becomes_a_file_uri_and_a_root_a_directory_uri() {
        let dir = std::env::temp_dir();
        let file = dir.join("a b.rs");
        assert!(file_uri(&file).unwrap().starts_with("file:///"));
        assert!(file_uri(&file).unwrap().ends_with("a%20b.rs"));
        assert!(directory_uri(&dir).unwrap().ends_with('/'));
        assert!(file_uri(Path::new("relative.rs")).is_none());
    }
}
