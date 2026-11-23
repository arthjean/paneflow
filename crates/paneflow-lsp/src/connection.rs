use std::collections::HashMap;
use std::fmt;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use serde_json::{json, Value};

use crate::framing::{encode_message, Frame, FrameError, FrameReader};
use crate::protocol::{
    configuration_answer, ConfigurationParams, LogMessageParams, METHOD_NOT_FOUND,
};

pub const CONNECTION_CLOSED: i64 = -32099;

#[derive(Clone, Debug, PartialEq)]
pub struct ResponseError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl ResponseError {
    fn closed() -> Self {
        Self {
            code: CONNECTION_CLOSED,
            message: "the connection to the language server closed".to_string(),
            data: None,
        }
    }

    fn from_value(value: &Value) -> Self {
        Self {
            code: value.get("code").and_then(Value::as_i64).unwrap_or(0),
            message: value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("the language server returned an error")
                .to_string(),
            data: value.get("data").cloned(),
        }
    }
}

impl fmt::Display for ResponseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloseCause {
    EndOfStream,
    Protocol(String),
    Io(String),
}

impl CloseCause {
    pub fn is_protocol_error(&self) -> bool {
        matches!(self, Self::Protocol(_))
    }
}

impl fmt::Display for CloseCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EndOfStream => f.write_str("the server closed its output"),
            Self::Protocol(detail) => write!(f, "protocol error: {detail}"),
            Self::Io(detail) => write!(f, "read error: {detail}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConnectionEvent {
    Notification { method: String, params: Value },
    SemanticTokensRefresh,
    Closed(CloseCause),
}

fn attach_params(message: &mut Value, params: Value) {
    if let (false, Some(object)) = (params.is_null(), message.as_object_mut()) {
        object.insert("params".to_string(), params);
    }
}

pub type ResponseCallback = Box<dyn FnOnce(Result<Value, ResponseError>) + Send>;
pub type EventSink = Box<dyn Fn(ConnectionEvent) + Send>;

struct Shared {
    server_id: String,
    settings: Value,
    outgoing: Mutex<Option<Sender<Vec<u8>>>>,
    pending: Mutex<HashMap<i64, ResponseCallback>>,
    next_id: AtomicI64,
    closed: AtomicBool,
}

#[derive(Clone)]
pub struct Connection {
    shared: Arc<Shared>,
}

impl Connection {
    pub fn start<R, W>(
        server_id: &str,
        settings: Value,
        reader: R,
        writer: W,
        events: EventSink,
    ) -> io::Result<Self>
    where
        R: Read + Send + 'static,
        W: Write + Send + 'static,
    {
        let (outgoing, queue) = mpsc::channel::<Vec<u8>>();
        let shared = Arc::new(Shared {
            server_id: server_id.to_string(),
            settings,
            outgoing: Mutex::new(Some(outgoing)),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicI64::new(1),
            closed: AtomicBool::new(false),
        });
        let writer_label = server_id.to_string();
        thread::Builder::new()
            .name("paneflow-lsp-writer".to_string())
            .spawn(move || {
                let mut writer = writer;
                for frame in queue {
                    if let Err(error) = writer.write_all(&frame).and_then(|()| writer.flush()) {
                        log::debug!(
                            "{writer_label}: writing to the language server failed: {error}"
                        );
                        break;
                    }
                }
            })?;
        let connection = Self { shared };
        let reader_side = connection.clone();
        thread::Builder::new()
            .name("paneflow-lsp-reader".to_string())
            .spawn(move || reader_side.read_loop(reader, events))?;
        Ok(connection)
    }

    pub fn is_closed(&self) -> bool {
        self.shared.closed.load(Ordering::Acquire)
    }

    pub fn request(
        &self,
        method: &str,
        params: Value,
        on_response: ResponseCallback,
    ) -> Option<i64> {
        if self.is_closed() {
            on_response(Err(ResponseError::closed()));
            return None;
        }
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        self.pending().insert(id, on_response);
        let mut message = json!({"jsonrpc": "2.0", "id": id, "method": method});
        attach_params(&mut message, params);
        if self.send(&message) {
            Some(id)
        } else {
            if let Some(callback) = self.pending().remove(&id) {
                callback(Err(ResponseError::closed()));
            }
            None
        }
    }

    pub fn notify(&self, method: &str, params: Value) -> bool {
        let mut message = json!({"jsonrpc": "2.0", "method": method});
        attach_params(&mut message, params);
        self.send(&message)
    }

    pub fn cancel(&self, id: i64) -> bool {
        self.notify("$/cancelRequest", json!({"id": id}))
    }

    pub fn close(&self) {
        self.shared
            .outgoing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, HashMap<i64, ResponseCallback>> {
        self.shared
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn send(&self, message: &Value) -> bool {
        let frame = match encode_message(message) {
            Ok(frame) => frame,
            Err(error) => {
                log::warn!(
                    "{}: could not encode a message: {error}",
                    self.shared.server_id
                );
                return false;
            }
        };
        self.shared
            .outgoing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|outgoing| outgoing.send(frame).is_ok())
    }

    fn read_loop<R: Read>(&self, reader: R, events: EventSink) {
        let mut frames = FrameReader::new(reader);
        let cause = loop {
            match frames.read_frame() {
                Ok(Frame::Message(message)) => self.dispatch(message, &events),
                Ok(Frame::Invalid(error)) => {
                    log::warn!(
                        "{}: skipped a message whose body is not valid JSON: {error}",
                        self.shared.server_id
                    );
                }
                Err(FrameError::EndOfStream) => break CloseCause::EndOfStream,
                Err(FrameError::Io(error)) => break CloseCause::Io(error.to_string()),
                Err(error) => break CloseCause::Protocol(error.to_string()),
            }
        };
        if cause.is_protocol_error() {
            log::warn!("{}: closing the connection: {cause}", self.shared.server_id);
        }
        self.shared.closed.store(true, Ordering::Release);
        self.close();
        let pending: Vec<ResponseCallback> = self
            .pending()
            .drain()
            .map(|(_, callback)| callback)
            .collect();
        for callback in pending {
            callback(Err(ResponseError::closed()));
        }
        events(ConnectionEvent::Closed(cause));
    }

    fn dispatch(&self, message: Value, events: &EventSink) {
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_string);
        let id = message.get("id").cloned().filter(|id| !id.is_null());
        match (method, id) {
            (Some(method), Some(id)) => {
                self.answer_request(&method, id, message.get("params"), events)
            }
            (Some(method), None) => self.handle_notification(method, message, events),
            (None, Some(id)) => self.complete_request(&id, &message),
            (None, None) => log::warn!(
                "{}: skipped a message that is neither a request, a response nor a notification",
                self.shared.server_id
            ),
        }
    }

    fn complete_request(&self, id: &Value, message: &Value) {
        let callback = id.as_i64().and_then(|id| self.pending().remove(&id));
        let Some(callback) = callback else {
            log::warn!(
                "{}: dropped a response for unknown request id {id}",
                self.shared.server_id
            );
            return;
        };
        match message.get("error") {
            Some(error) if !error.is_null() => callback(Err(ResponseError::from_value(error))),
            _ => callback(Ok(message.get("result").cloned().unwrap_or(Value::Null))),
        }
    }

    fn answer_request(&self, method: &str, id: Value, params: Option<&Value>, events: &EventSink) {
        let answer = match method {
            "workspace/configuration" => {
                let params: ConfigurationParams = params
                    .cloned()
                    .and_then(|params| serde_json::from_value(params).ok())
                    .unwrap_or(ConfigurationParams { items: Vec::new() });
                Ok(configuration_answer(&self.shared.settings, &params))
            }
            "window/workDoneProgress/create" | "client/registerCapability" => Ok(Value::Null),
            "workspace/semanticTokens/refresh" => {
                events(ConnectionEvent::SemanticTokensRefresh);
                Ok(Value::Null)
            }
            _ => Err(method),
        };
        let reply = match answer {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(method) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": METHOD_NOT_FOUND, "message": format!("unhandled method {method}")}
            }),
        };
        self.send(&reply);
    }

    fn handle_notification(&self, method: String, message: Value, events: &EventSink) {
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        if method == "window/logMessage" {
            if let Ok(log) = serde_json::from_value::<LogMessageParams>(params) {
                log::debug!("{}: {}", self.shared.server_id, log.message);
            }
            return;
        }
        events(ConnectionEvent::Notification { method, params });
    }
}

#[cfg(test)]
pub(crate) mod test_pipe {
    use std::collections::VecDeque;
    use std::io::{self, Read, Write};
    use std::sync::{Arc, Condvar, Mutex, PoisonError};

    #[derive(Default)]
    struct State {
        bytes: VecDeque<u8>,
        closed: bool,
    }

    type Shared = Arc<(Mutex<State>, Condvar)>;

    pub struct PipeReader(Shared);
    pub struct PipeWriter(Shared);

    pub fn pipe() -> (PipeReader, PipeWriter) {
        let shared: Shared = Arc::default();
        (PipeReader(shared.clone()), PipeWriter(shared))
    }

    impl Read for PipeReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let (state, ready) = &*self.0;
            let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
            while state.bytes.is_empty() && !state.closed {
                state = ready.wait(state).unwrap_or_else(PoisonError::into_inner);
            }
            let count = buffer.len().min(state.bytes.len());
            for (slot, byte) in buffer.iter_mut().zip(state.bytes.drain(..count)) {
                *slot = byte;
            }
            Ok(count)
        }
    }

    impl Write for PipeWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            let (state, ready) = &*self.0;
            let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
            if state.closed {
                return Err(io::Error::from(io::ErrorKind::BrokenPipe));
            }
            state.bytes.extend(buffer);
            ready.notify_all();
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Drop for PipeWriter {
        fn drop(&mut self) {
            let (state, ready) = &*self.0;
            state.lock().unwrap_or_else(PoisonError::into_inner).closed = true;
            ready.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_pipe::{pipe, PipeReader, PipeWriter};
    use super::*;
    use crate::framing::write_message;
    use std::sync::mpsc::Receiver;
    use std::sync::OnceLock;

    struct CapturingLogger(Mutex<Vec<(log::Level, String)>>);

    impl log::Log for CapturingLogger {
        fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
            true
        }

        fn log(&self, record: &log::Record<'_>) {
            self.0
                .lock()
                .unwrap()
                .push((record.level(), record.args().to_string()));
        }

        fn flush(&self) {}
    }

    fn captured_logs() -> &'static CapturingLogger {
        static LOGGER: OnceLock<&'static CapturingLogger> = OnceLock::new();
        LOGGER.get_or_init(|| {
            let logger: &'static CapturingLogger =
                Box::leak(Box::new(CapturingLogger(Mutex::new(Vec::new()))));
            let _ = log::set_logger(logger);
            log::set_max_level(log::LevelFilter::Debug);
            logger
        })
    }

    fn logged(level: log::Level, text: &str) -> bool {
        captured_logs()
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|(at, line)| *at == level && line == text)
    }

    struct Harness {
        connection: Connection,
        server_out: PipeWriter,
        server_in: FrameReader<PipeReader>,
        events: Receiver<ConnectionEvent>,
    }

    fn harness(settings: Value) -> Harness {
        let (client_reader, server_out) = pipe();
        let (server_in, client_writer) = pipe();
        let (sender, events) = mpsc::channel();
        let sender = Mutex::new(sender);
        let connection = Connection::start(
            "fake",
            settings,
            client_reader,
            client_writer,
            Box::new(move |event| {
                let _ = sender.lock().unwrap().send(event);
            }),
        )
        .unwrap();
        Harness {
            connection,
            server_out,
            server_in: FrameReader::new(server_in),
            events,
        }
    }

    impl Harness {
        fn server_reads(&mut self) -> Value {
            let frame = self.server_in.read_frame().unwrap();
            assert!(matches!(frame, Frame::Message(_)), "{frame:?}");
            match frame {
                Frame::Message(value) => value,
                Frame::Invalid(_) => Value::Null,
            }
        }

        fn server_sends(&mut self, message: Value) {
            write_message(&mut self.server_out, &message).unwrap();
        }
    }

    #[test]
    fn a_response_completes_the_request_with_the_same_id() {
        let mut h = harness(Value::Null);
        let (first_tx, first) = mpsc::channel();
        let (second_tx, second) = mpsc::channel();
        let first_id = h
            .connection
            .request(
                "a",
                json!({}),
                Box::new(move |result| first_tx.send(result).unwrap()),
            )
            .unwrap();
        let second_id = h
            .connection
            .request(
                "b",
                json!({}),
                Box::new(move |result| second_tx.send(result).unwrap()),
            )
            .unwrap();
        assert_eq!(h.server_reads()["id"], json!(first_id));
        assert_eq!(h.server_reads()["id"], json!(second_id));
        h.server_sends(json!({"jsonrpc": "2.0", "id": second_id, "result": "second"}));
        h.server_sends(
            json!({"jsonrpc": "2.0", "id": first_id, "error": {"code": -1, "message": "no"}}),
        );
        assert_eq!(second.recv().unwrap(), Ok(json!("second")));
        assert_eq!(first.recv().unwrap().unwrap_err().message, "no");
    }

    #[test]
    fn a_response_with_an_unknown_id_is_dropped_and_the_connection_keeps_working() {
        captured_logs();
        let mut h = harness(Value::Null);
        h.server_sends(json!({"jsonrpc": "2.0", "id": 999, "result": null}));
        let (tx, rx) = mpsc::channel();
        let id = h
            .connection
            .request(
                "a",
                json!({}),
                Box::new(move |result| tx.send(result).unwrap()),
            )
            .unwrap();
        h.server_reads();
        h.server_sends(json!({"jsonrpc": "2.0", "id": id, "result": 1}));
        assert_eq!(rx.recv().unwrap(), Ok(json!(1)));
        assert!(!h.connection.is_closed());
        assert!(logged(
            log::Level::Warn,
            "fake: dropped a response for unknown request id 999"
        ));
    }

    #[test]
    fn the_three_required_server_requests_get_results_and_any_other_gets_method_not_found() {
        let mut h = harness(json!({"semanticTokens": true}));
        h.server_sends(json!({"jsonrpc": "2.0", "id": "c", "method": "workspace/configuration", "params": {"items": [{"section": "gopls"}]}}));
        h.server_sends(json!({"jsonrpc": "2.0", "id": 1, "method": "window/workDoneProgress/create", "params": {"token": "t"}}));
        h.server_sends(json!({"jsonrpc": "2.0", "id": 2, "method": "client/registerCapability", "params": {"registrations": []}}));
        h.server_sends(
            json!({"jsonrpc": "2.0", "id": 3, "method": "textDocument/hover", "params": {}}),
        );
        let configuration = h.server_reads();
        assert_eq!(configuration["id"], json!("c"));
        assert_eq!(configuration["result"], json!([{"semanticTokens": true}]));
        let progress = h.server_reads();
        assert_eq!(progress["id"], json!(1));
        assert!(progress.get("result").is_some() && progress.get("error").is_none());
        let registration = h.server_reads();
        assert_eq!(registration["id"], json!(2));
        assert!(registration.get("result").is_some() && registration.get("error").is_none());
        let other = h.server_reads();
        assert_eq!(other["id"], json!(3));
        assert_eq!(other["error"]["code"], json!(METHOD_NOT_FOUND));
    }

    #[test]
    fn a_log_message_is_consumed_and_other_notifications_reach_the_lifecycle() {
        captured_logs();
        let mut h = harness(Value::Null);
        h.server_sends(json!({"jsonrpc": "2.0", "method": "window/logMessage", "params": {"type": 4, "message": "indexing"}}));
        h.server_sends(json!({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": "t"}}));
        assert_eq!(
            h.events.recv().unwrap(),
            ConnectionEvent::Notification {
                method: "$/progress".to_string(),
                params: json!({"token": "t"})
            }
        );
        assert!(logged(log::Level::Debug, "fake: indexing"));
    }

    #[test]
    fn a_header_without_content_length_closes_the_connection_as_a_protocol_error() {
        let mut h = harness(Value::Null);
        let (tx, rx) = mpsc::channel();
        h.connection
            .request(
                "a",
                json!({}),
                Box::new(move |result| tx.send(result).unwrap()),
            )
            .unwrap();
        h.server_out.write_all(b"X-Other: 1\r\n\r\n{}").unwrap();
        let event = h.events.recv().unwrap();
        assert!(
            matches!(&event, ConnectionEvent::Closed(cause) if cause.is_protocol_error()),
            "{event:?}"
        );
        assert!(h.connection.is_closed());
        assert_eq!(rx.recv().unwrap().unwrap_err().code, CONNECTION_CLOSED);
        assert!(!h.connection.notify("late", json!({})));
    }

    #[test]
    fn an_oversized_message_closes_the_connection_as_a_protocol_error() {
        let mut h = harness(Value::Null);
        write!(
            h.server_out,
            "Content-Length: {}\r\n\r\n",
            crate::framing::MAX_MESSAGE_BYTES + 1
        )
        .unwrap();
        let event = h.events.recv().unwrap();
        assert!(
            matches!(&event, ConnectionEvent::Closed(cause) if cause.is_protocol_error()),
            "{event:?}"
        );
    }

    #[test]
    fn an_invalid_body_is_skipped_and_the_next_message_arrives_on_the_same_connection() {
        let mut h = harness(Value::Null);
        h.server_out
            .write_all(b"Content-Length: 3\r\n\r\n{{{")
            .unwrap();
        h.server_sends(json!({"jsonrpc": "2.0", "method": "custom/after", "params": 1}));
        assert_eq!(
            h.events.recv().unwrap(),
            ConnectionEvent::Notification {
                method: "custom/after".to_string(),
                params: json!(1)
            }
        );
        assert!(!h.connection.is_closed());
    }

    #[test]
    fn the_server_closing_its_output_ends_the_connection_without_a_protocol_error() {
        let h = harness(Value::Null);
        drop(h.server_out);
        assert_eq!(
            h.events.recv().unwrap(),
            ConnectionEvent::Closed(CloseCause::EndOfStream)
        );
    }
}
