use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};

use crate::line_wire::{LineRead, Wire};
use crate::{jsonrpc_error_message_from_value, IpcTransport};

pub const HOST_PROTOCOL_VERSION: u32 = 1;

pub const MAX_CONTROL_FRAME_BYTES: usize = 64 * 1024;

pub const REQUEST_DEADLINE: Duration = Duration::from_secs(10);

pub const ENV_HOST_ENDPOINT: &str = "PANEFLOW_HOST_ENDPOINT";

pub const METHOD_HOST_HELLO: &str = "host.hello";

pub const METHOD_AGENT_EVENT: &str = "agent.event";

pub const METHOD_AGENT_SNAPSHOT: &str = "agent.snapshot";

pub const METHOD_AGENT_FOLLOW: &str = "agent.follow";

pub const ERR_NO_CONTROLLER: i64 = -32030;

pub const ERR_SESSION_NOT_FOUND: i64 = -32020;

pub fn host_endpoint_from(raw: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    raw.filter(|value| !value.is_empty()).map(PathBuf::from)
}

pub fn host_endpoint_from_env() -> Option<PathBuf> {
    host_endpoint_from(std::env::var_os(ENV_HOST_ENDPOINT).as_deref())
}

pub fn session_id_from(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

pub fn control_hello(client: &str) -> Value {
    json!({"client": client, "protocol": HOST_PROTOCOL_VERSION})
}

fn request_frame(id: u64, method: &str, params: Value) -> Value {
    let mut frame = serde_json::Map::new();
    frame.insert("jsonrpc".into(), Value::from("2.0"));
    frame.insert("id".into(), Value::from(id));
    frame.insert("method".into(), Value::from(method));
    frame.insert("params".into(), params);
    Value::Object(frame)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlTarget {
    Controller(PathBuf),
    Host(PathBuf),
}

pub fn choose_control_target(
    controller_socket: Option<PathBuf>,
    controller_is_listening: bool,
    host_endpoint: Option<PathBuf>,
) -> Option<ControlTarget> {
    match (controller_socket, host_endpoint) {
        (Some(socket), _) if controller_is_listening => Some(ControlTarget::Controller(socket)),
        (_, Some(endpoint)) => Some(ControlTarget::Host(endpoint)),
        (Some(socket), None) => Some(ControlTarget::Controller(socket)),
        (None, None) => None,
    }
}

pub fn resolve_host_endpoint(
    controller: Option<&paneflow_home::IpcEndpoint>,
    home_host_endpoint: Option<PathBuf>,
    reserved_host_endpoint: Option<PathBuf>,
) -> Option<PathBuf> {
    let home_is_isolated = controller.is_some_and(|endpoint| endpoint.home_is_isolated);
    let owned_host_endpoint = home_is_isolated
        .then(|| home_host_endpoint.clone())
        .flatten();
    paneflow_home::honored_endpoint_override(
        host_endpoint_from_env(),
        owned_host_endpoint.as_deref(),
        reserved_host_endpoint.as_deref(),
        paneflow_home::endpoint_override_allowed(),
    )
    .or(home_host_endpoint)
}

pub fn resolve_control_target(
    controller: Option<paneflow_home::IpcEndpoint>,
    home_host_endpoint: Option<PathBuf>,
    reserved_host_endpoint: Option<PathBuf>,
) -> Option<ControlTarget> {
    let host_endpoint = resolve_host_endpoint(
        controller.as_ref(),
        home_host_endpoint,
        reserved_host_endpoint,
    );
    let controller = controller.map(|endpoint| endpoint.path);
    let listening = controller
        .as_deref()
        .is_some_and(crate::socket_is_listening);
    choose_control_target(controller, listening, host_endpoint)
}

#[derive(Debug)]
pub enum ControlConnectError {
    Transport(io::Error),
    Handshake(String),
}

pub const FIRST_ANSWER_WINDOW: Duration = Duration::from_secs(1);
pub const SHORTEST_PROBE: Duration = Duration::from_millis(100);

pub fn answer_windows(patience: Duration) -> impl Iterator<Item = Duration> {
    let patience = patience.max(SHORTEST_PROBE);
    let mut granted = Duration::ZERO;
    let mut next = FIRST_ANSWER_WINDOW;
    std::iter::from_fn(move || {
        let left = patience.saturating_sub(granted);
        if left.is_zero() {
            return None;
        }
        let window = next.min(left);
        granted += window;
        next = next.saturating_mul(2);
        Some(window)
    })
}

#[derive(Debug)]
pub enum ControlProbeError {
    Absent(io::Error),
    Unanswered(String),
    Faulted(String),
}

fn probe_io_failure(during: &str, error: io::Error) -> ControlProbeError {
    match error.kind() {
        io::ErrorKind::TimedOut
        | io::ErrorKind::WouldBlock
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::BrokenPipe => ControlProbeError::Unanswered(format!("{during}: {error}")),
        _ => ControlProbeError::Faulted(format!("{during}: {error}")),
    }
}

pub struct HostControl {
    wire: Wire,
    next_id: u64,
    identity: Value,
}

impl HostControl {
    pub fn connect(endpoint: &Path, client: &str) -> Result<Self, String> {
        Self::connect_with_deadline(endpoint, client, REQUEST_DEADLINE)
    }

    pub fn connect_with_deadline(
        endpoint: &Path,
        client: &str,
        deadline: Duration,
    ) -> Result<Self, String> {
        Self::open(endpoint, client, deadline).map_err(|error| match error {
            ControlConnectError::Transport(error) => format!(
                "the local Paneflow host is not reachable at {} ({error})",
                endpoint.display()
            ),
            ControlConnectError::Handshake(message) => message,
        })
    }

    pub fn open(
        endpoint: &Path,
        client: &str,
        deadline: Duration,
    ) -> Result<Self, ControlConnectError> {
        let started = std::time::Instant::now();
        let wire = Wire::connect_with_timeout(endpoint, MAX_CONTROL_FRAME_BYTES, deadline)
            .map_err(ControlConnectError::Transport)?;
        let mut control = Self {
            wire,
            next_id: 1,
            identity: Value::Null,
        };
        control.identity = control
            .request_with_deadline(
                METHOD_HOST_HELLO,
                control_hello(client),
                deadline.saturating_sub(started.elapsed()),
            )
            .map_err(ControlConnectError::Handshake)?;
        Ok(control)
    }

    pub fn probe_open(
        endpoint: &Path,
        client: &str,
        window: Duration,
    ) -> Result<Self, ControlProbeError> {
        let started = std::time::Instant::now();
        let wire = Wire::connect_with_timeout(endpoint, MAX_CONTROL_FRAME_BYTES, window).map_err(
            |error| match error.kind() {
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => {
                    ControlProbeError::Absent(error)
                }
                _ => probe_io_failure(&format!("connecting to {}", endpoint.display()), error),
            },
        )?;
        let mut control = Self {
            wire,
            next_id: 1,
            identity: Value::Null,
        };
        let answer = window.saturating_sub(started.elapsed()).max(SHORTEST_PROBE);
        control.identity = control.hello_within(client, answer)?;
        Ok(control)
    }

    fn hello_within(&mut self, client: &str, answer: Duration) -> Result<Value, ControlProbeError> {
        let id = self.next_id;
        self.next_id += 1;
        let hello = request_frame(id, METHOD_HOST_HELLO, control_hello(client));
        self.wire
            .write_json_with_timeout(&hello, answer)
            .map_err(|error| probe_io_failure("sending host.hello", error))?;
        loop {
            let line = match self.wire.read_line(answer) {
                Ok(LineRead::Line(line)) => line,
                Ok(LineRead::Idle) => {
                    return Err(ControlProbeError::Unanswered(format!(
                        "no host.hello answer within {} ms",
                        answer.as_millis()
                    )));
                }
                Ok(LineRead::Eof) => {
                    return Err(ControlProbeError::Unanswered(
                        "the endpoint closed the connection before answering host.hello".into(),
                    ));
                }
                Ok(LineRead::TooLong) => {
                    return Err(ControlProbeError::Faulted(
                        "the endpoint oversized its host.hello answer".into(),
                    ));
                }
                Err(error) => return Err(probe_io_failure("reading host.hello", error)),
            };
            let value: Value = serde_json::from_str(line.trim()).map_err(|error| {
                ControlProbeError::Faulted(format!("invalid host.hello answer: {error}"))
            })?;
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(message) = jsonrpc_error_message_from_value(&value) {
                return Err(ControlProbeError::Faulted(message));
            }
            return value.get("result").cloned().ok_or_else(|| {
                ControlProbeError::Faulted("host.hello answered without a result".into())
            });
        }
    }

    pub fn identity(&self) -> &Value {
        &self.identity
    }

    pub fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.request_with_deadline(method, params, REQUEST_DEADLINE)
    }

    pub fn request_with_deadline(
        &mut self,
        method: &str,
        params: Value,
        deadline: Duration,
    ) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let request = request_frame(id, method, params);
        self.wire
            .write_json_with_timeout(&request, deadline)
            .map_err(|error| format!("paneflow host request {method} failed: {error}"))?;
        loop {
            let line = match self.wire.read_line(deadline) {
                Ok(LineRead::Line(line)) => line,
                Ok(LineRead::Eof) => {
                    return Err(format!(
                        "the local Paneflow host closed the connection during {method}"
                    ));
                }
                Ok(LineRead::TooLong) => {
                    return Err(format!(
                        "the local Paneflow host oversized its {method} reply"
                    ));
                }
                Ok(LineRead::Idle) => {
                    return Err(format!(
                        "paneflow host request {method} timed out after {} ms",
                        deadline.as_millis()
                    ));
                }
                Err(error) => {
                    return Err(format!("paneflow host request {method} failed: {error}"));
                }
            };
            let value: Value = serde_json::from_str(line.trim()).map_err(|error| {
                format!("invalid JSON-RPC response from the local Paneflow host: {error}")
            })?;
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(message) = jsonrpc_error_message_from_value(&value) {
                return Err(message);
            }
            return value.get("result").cloned().ok_or_else(|| {
                "the local Paneflow host answered without a result or an error".to_string()
            });
        }
    }

    pub fn read_stream_line(&mut self, timeout: Duration) -> io::Result<Option<String>> {
        match self.wire.read_line(timeout) {
            Ok(LineRead::Line(line)) => Ok(Some(line)),
            Ok(LineRead::Eof) => Ok(None),
            Ok(LineRead::TooLong) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the local Paneflow host oversized a stream frame",
            )),
            Ok(LineRead::Idle) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the local Paneflow host sent no stream frame within the timeout",
            )),
            Err(error) => Err(error),
        }
    }

    pub fn write_request(&mut self, method: &str, params: Value) -> io::Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        let request = request_frame(id, method, params);
        self.wire.write_json(&request)?;
        Ok(id)
    }
}

pub struct HostTransport {
    endpoint: PathBuf,
    client: String,
    calls: AtomicU64,
    listed_sessions: Mutex<Vec<(u64, String)>>,
}

impl HostTransport {
    pub fn connect(endpoint: &Path, client: &str) -> Result<Self, String> {
        HostControl::connect(endpoint, client)?;
        Ok(Self {
            endpoint: endpoint.to_path_buf(),
            client: client.to_owned(),
            calls: AtomicU64::new(0),
            listed_sessions: Mutex::new(Vec::new()),
        })
    }

    fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let result = HostControl::connect(&self.endpoint, &self.client)?.request(method, params)?;
        if method == SURFACE_LIST {
            *self.lock_listed_sessions() = listed_sessions(&result);
        }
        Ok(result)
    }

    fn lock_listed_sessions(&self) -> std::sync::MutexGuard<'_, Vec<(u64, String)>> {
        self.listed_sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn listed_session(&self, surface_id: u64) -> Option<String> {
        self.lock_listed_sessions()
            .iter()
            .find(|(listed, _)| *listed == surface_id)
            .map(|(_, session)| session.clone())
    }

    fn session_for(&self, surface_id: u64) -> Result<String, String> {
        if let Some(session) = self.listed_session(surface_id) {
            return Ok(session);
        }
        self.request(SURFACE_LIST, json!({}))?;
        self.listed_session(surface_id)
            .ok_or_else(|| format!("Surface not found: no pane {surface_id} on this host"))
    }

    pub fn calls(&self) -> u64 {
        self.calls.load(Ordering::Relaxed)
    }
}

const SURFACE_LIST: &str = "surface.list";

fn listed_sessions(result: &Value) -> Vec<(u64, String)> {
    result
        .get("surfaces")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|surface| {
            Some((
                surface.get("surface_id")?.as_u64()?,
                surface.get("session")?.as_str()?.to_owned(),
            ))
        })
        .collect()
}

fn addressed_surface(method: &str, params: &Value) -> Option<u64> {
    if !method.starts_with("surface.") || method == SURFACE_LIST || params.get("session").is_some()
    {
        return None;
    }
    params.get("surface_id")?.as_u64()
}

impl IpcTransport for HostTransport {
    fn call(&self, method: &str, mut params: Value) -> Result<Value, String> {
        let Some(surface_id) = addressed_surface(method, &params) else {
            return self.request(method, params);
        };
        let session = self.session_for(surface_id)?;
        if let Some(object) = params.as_object_mut() {
            object.insert("session".into(), Value::String(session.clone()));
        }
        self.request(method, params).map_err(|message| {
            if message.starts_with(&format!("paneflow error {ERR_SESSION_NOT_FOUND}:")) {
                format!(
                    "Surface not found: pane {surface_id} (session {session}) has left this host"
                )
            } else {
                message
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answer_windows_start_at_one_second_double_and_spend_exactly_the_patience() {
        let windows = |patience| answer_windows(patience).collect::<Vec<_>>();
        assert_eq!(
            windows(REQUEST_DEADLINE),
            [1, 2, 4, 3].map(Duration::from_secs),
            "the former single 10 s deadline, split so a slow peer still gets 4 s"
        );
        assert_eq!(
            windows(Duration::from_secs(5)),
            [1, 2, 2].map(Duration::from_secs)
        );
        assert_eq!(
            windows(Duration::from_millis(400)),
            [Duration::from_millis(400)]
        );
        assert_eq!(
            windows(Duration::ZERO),
            [SHORTEST_PROBE],
            "an exhausted caller still looks once"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_probe_tells_an_absent_endpoint_from_a_silent_or_closing_one() {
        let home = tempfile::tempdir().unwrap();
        let absent = home.path().join("absent.sock");
        assert!(matches!(
            HostControl::probe_open(&absent, "probe-test", SHORTEST_PROBE),
            Err(ControlProbeError::Absent(_))
        ));

        let silent_endpoint = home.path().join("silent.sock");
        let _silent = std::os::unix::net::UnixListener::bind(&silent_endpoint).unwrap();
        assert!(matches!(
            HostControl::probe_open(&silent_endpoint, "probe-test", SHORTEST_PROBE),
            Err(ControlProbeError::Unanswered(_))
        ));

        let closing_endpoint = home.path().join("closing.sock");
        let closing = std::os::unix::net::UnixListener::bind(&closing_endpoint).unwrap();
        let closer = std::thread::spawn(move || drop(closing.accept().unwrap()));
        assert!(matches!(
            HostControl::probe_open(&closing_endpoint, "probe-test", REQUEST_DEADLINE),
            Err(ControlProbeError::Unanswered(_))
        ));
        closer.join().unwrap();
    }

    #[test]
    fn only_surface_calls_by_alias_are_readdressed_to_their_session() {
        assert_eq!(
            addressed_surface("surface.read", &json!({"surface_id": 2})),
            Some(2)
        );
        assert_eq!(
            addressed_surface("surface.read", &json!({"surface_id": 2, "session": "s"})),
            None
        );
        assert_eq!(
            addressed_surface("surface.list", &json!({"surface_id": 2})),
            None
        );
        assert_eq!(
            addressed_surface("fleet.list", &json!({"surface_id": 2})),
            None
        );
        assert_eq!(addressed_surface("surface.read", &json!({})), None);
    }

    #[test]
    fn a_surface_list_maps_each_alias_to_its_session() {
        let listed = listed_sessions(&json!({"surfaces": [
            {"surface_id": 1, "session": "a"},
            {"surface_id": 2, "session": "b"},
            {"surface_id": 3},
        ]}));
        assert_eq!(listed, vec![(1, "a".to_string()), (2, "b".to_string())]);
        assert!(listed_sessions(&json!({})).is_empty());
    }

    #[test]
    fn a_control_hello_declares_no_terminal_engine() {
        let hello = control_hello("paneflow-cli");
        assert_eq!(hello["client"], "paneflow-cli");
        assert_eq!(hello["protocol"], HOST_PROTOCOL_VERSION);
        assert!(
            hello.get("engine").is_none(),
            "a control client never claims snapshot compatibility"
        );
    }

    #[test]
    fn an_open_window_keeps_the_controller_socket_and_a_closed_one_falls_back_to_the_host() {
        let socket = PathBuf::from("/tmp/paneflow.sock");
        let endpoint = PathBuf::from("/tmp/paneflow-host.sock");

        assert_eq!(
            choose_control_target(Some(socket.clone()), true, Some(endpoint.clone())),
            Some(ControlTarget::Controller(socket.clone()))
        );
        assert_eq!(
            choose_control_target(Some(socket.clone()), false, Some(endpoint.clone())),
            Some(ControlTarget::Host(endpoint.clone()))
        );
        assert_eq!(
            choose_control_target(Some(socket.clone()), false, None),
            Some(ControlTarget::Controller(socket)),
            "with no host to fall back to, the existing socket error still surfaces"
        );
        assert_eq!(
            choose_control_target(None, false, Some(endpoint.clone())),
            Some(ControlTarget::Host(endpoint))
        );
        assert_eq!(choose_control_target(None, false, None), None);
    }

    #[test]
    fn endpoint_and_session_ignore_blank_environment_values() {
        use std::ffi::OsStr;

        assert_eq!(host_endpoint_from(None), None);
        assert_eq!(host_endpoint_from(Some(OsStr::new(""))), None);
        assert_eq!(
            host_endpoint_from(Some(OsStr::new("/tmp/paneflow-host.sock"))),
            Some(PathBuf::from("/tmp/paneflow-host.sock"))
        );

        assert_eq!(session_id_from(None), None);
        assert_eq!(session_id_from(Some("   ")), None);
        assert_eq!(session_id_from(Some(" abc ")), Some("abc".to_string()));
    }
}
