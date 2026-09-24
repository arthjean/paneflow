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

pub const ENV_SESSION_ID: &str = "PANEFLOW_SESSION_ID";

pub const ENV_WORKSPACE_UUID: &str = "PANEFLOW_WORKSPACE_UUID";

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

pub fn session_id_from_env() -> Option<String> {
    session_id_from(std::env::var(ENV_SESSION_ID).ok().as_deref())
}

pub fn control_hello(client: &str) -> Value {
    json!({"client": client, "protocol": HOST_PROTOCOL_VERSION})
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

pub fn resolve_control_target(
    isolated_controller: Option<PathBuf>,
    home_host_endpoint: Option<PathBuf>,
    reserved_host_endpoint: Option<PathBuf>,
) -> Option<ControlTarget> {
    let owned_host_endpoint = isolated_controller
        .is_some()
        .then(|| home_host_endpoint.clone())
        .flatten();
    let host_endpoint = crate::honored_socket_override(
        host_endpoint_from_env(),
        owned_host_endpoint.as_deref(),
        reserved_host_endpoint.as_deref(),
        crate::socket_override_allowed(),
    )
    .or(home_host_endpoint);
    let controller = crate::resolve_socket_path_or(isolated_controller);
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
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
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
                        "paneflow host request {method} timed out after {deadline:?}"
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
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
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

    pub fn endpoint(&self) -> &Path {
        &self.endpoint
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
