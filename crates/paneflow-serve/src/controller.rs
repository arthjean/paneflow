use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use paneflow_ipc_client::host_control::{ControlConnectError, HostControl};
use serde_json::{Value, json};

use crate::protocol::{
    CAPABILITY_AGENT_FOLLOW, METHOD_AGENT_ACKNOWLEDGE, METHOD_AGENT_ACTIVITY_LOG,
    METHOD_AGENT_FOLLOW, METHOD_AGENT_SNAPSHOT, REQUEST_DEADLINE, WorkerIdentity,
};

pub const CLIENT_NAME: &str = "paneflow-controller";

const RECONNECT_POLL: Duration = Duration::from_millis(50);

const READ_SLICE: Duration = Duration::from_millis(50);

#[derive(Debug, thiserror::Error)]
pub enum ControllerError {
    #[error("permission denied: {0} is owned by another user account")]
    PermissionDenied(PathBuf),
    #[error("the paneflow worker is not reachable at {endpoint}: {reason}")]
    Unreachable { endpoint: PathBuf, reason: String },
    #[error("capability not advertised: {0}")]
    CapabilityNotAdvertised(String),
    #[error("{0}")]
    Protocol(String),
}

impl ControllerError {
    pub fn from_connect(endpoint: &Path, error: ControlConnectError) -> Self {
        match error {
            ControlConnectError::Transport(error)
                if error.kind() == io::ErrorKind::PermissionDenied =>
            {
                Self::PermissionDenied(endpoint.to_path_buf())
            }
            ControlConnectError::Transport(error) => Self::Unreachable {
                endpoint: endpoint.to_path_buf(),
                reason: error.to_string(),
            },
            ControlConnectError::Handshake(reason) => Self::Unreachable {
                endpoint: endpoint.to_path_buf(),
                reason,
            },
        }
    }
}

pub const ROW_FIELDS: &[&str] = &[
    "session",
    "title",
    "lifecycle",
    "live",
    "status",
    "activity_source",
    "outcome",
    "runtime_id",
    "menu_prompt_active",
    "unread",
    "restart_recommended",
];

pub const ROW_ACTIVITY_FIELDS: &[&str] = &["tool", "state"];

pub fn row_fingerprint(session: &Value) -> Value {
    let mut fingerprint = serde_json::Map::new();
    for field in ROW_FIELDS {
        if let Some(value) = session.get(*field).filter(|value| !value.is_null()) {
            fingerprint.insert((*field).to_string(), value.clone());
        }
    }
    let activity = session.get("activity").or_else(|| session.get("agent"));
    for field in ROW_ACTIVITY_FIELDS {
        if let Some(value) = activity
            .and_then(|activity| activity.get(*field))
            .filter(|value| !value.is_null())
        {
            fingerprint.insert(format!("activity.{field}"), value.clone());
        }
    }
    Value::Object(fingerprint)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bootstrap {
    pub identity: Box<WorkerIdentity>,
    pub core_connected: bool,
    pub sessions: Vec<Value>,
    pub fresh: Vec<Value>,
    pub resumed: bool,
}

impl Bootstrap {
    pub fn to_value(&self) -> Value {
        json!({
            "type": "bootstrap",
            "worker": *self.identity,
            "capabilities": self.identity.capabilities,
            "core_connected": self.core_connected,
            "resumed": self.resumed,
            "sessions": self.sessions,
            "fresh": self.fresh,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FollowFrame {
    Bootstrap(Box<Bootstrap>),
    Event(Box<Value>),
    Idle,
    Disconnected(String),
}

pub struct Controller {
    endpoint: PathBuf,
    control: HostControl,
    identity: WorkerIdentity,
}

impl Controller {
    pub fn connect(endpoint: &Path) -> Result<Self, ControllerError> {
        let control = HostControl::open(endpoint, CLIENT_NAME, REQUEST_DEADLINE)
            .map_err(|error| ControllerError::from_connect(endpoint, error))?;
        let identity: WorkerIdentity =
            serde_json::from_value(control.identity().clone()).map_err(|error| {
                ControllerError::Unreachable {
                    endpoint: endpoint.to_path_buf(),
                    reason: format!("the endpoint did not answer with a worker identity: {error}"),
                }
            })?;
        Ok(Self {
            endpoint: endpoint.to_path_buf(),
            control,
            identity,
        })
    }

    pub fn identity(&self) -> &WorkerIdentity {
        &self.identity
    }

    pub fn advertises(&self, capability: &str) -> bool {
        self.identity
            .capabilities
            .iter()
            .any(|held| held == capability)
    }

    pub fn require(&self, capability: &str) -> Result<(), ControllerError> {
        if self.advertises(capability) {
            return Ok(());
        }
        Err(ControllerError::CapabilityNotAdvertised(
            capability.to_owned(),
        ))
    }

    pub fn snapshot(&mut self) -> Result<Bootstrap, ControllerError> {
        self.require(METHOD_AGENT_SNAPSHOT)?;
        let answered = self
            .control
            .request(METHOD_AGENT_SNAPSHOT, json!({}))
            .map_err(ControllerError::Protocol)?;
        Ok(self.bootstrap_from(&answered, false))
    }

    pub fn activity_log(&mut self, limit: Option<usize>) -> Result<Value, ControllerError> {
        self.require(METHOD_AGENT_ACTIVITY_LOG)?;
        let params = match limit {
            Some(limit) => json!({"limit": limit}),
            None => json!({}),
        };
        self.control
            .request(METHOD_AGENT_ACTIVITY_LOG, params)
            .map_err(ControllerError::Protocol)
    }

    pub fn acknowledge(&mut self, sessions: &[String]) -> Result<Value, ControllerError> {
        self.require(crate::protocol::CAPABILITY_AGENT_UNREAD)?;
        self.control
            .request(METHOD_AGENT_ACKNOWLEDGE, json!({"sessions": sessions}))
            .map_err(ControllerError::Protocol)
    }

    fn bootstrap_from(&self, answered: &Value, resumed: bool) -> Bootstrap {
        let sessions: Vec<Value> = answered["sessions"].as_array().cloned().unwrap_or_default();
        Bootstrap {
            identity: Box::new(self.identity.clone()),
            core_connected: answered["core_connected"].as_bool().unwrap_or_default(),
            fresh: sessions.clone(),
            sessions,
            resumed,
        }
    }

    fn start_follow(&mut self, resumed: bool) -> Result<Bootstrap, ControllerError> {
        self.require(CAPABILITY_AGENT_FOLLOW)?;
        let header = self
            .control
            .request(METHOD_AGENT_FOLLOW, json!({}))
            .map_err(ControllerError::Protocol)?;
        Ok(self.bootstrap_from(&header, resumed))
    }

    pub fn follow(self) -> Result<FollowSession, ControllerError> {
        FollowSession::start(self)
    }
}

pub struct FollowSession {
    endpoint: PathBuf,
    controller: Option<Controller>,
    published: BTreeMap<String, Value>,
    pending: Option<FollowFrame>,
}

impl FollowSession {
    pub fn open(endpoint: &Path) -> Result<Self, ControllerError> {
        Controller::connect(endpoint)?.follow()
    }

    fn start(mut controller: Controller) -> Result<Self, ControllerError> {
        let bootstrap = controller.start_follow(false)?;
        let mut session = Self {
            endpoint: controller.endpoint.clone(),
            controller: Some(controller),
            published: BTreeMap::new(),
            pending: None,
        };
        let frame = session.record_bootstrap(bootstrap);
        session.pending = Some(frame);
        Ok(session)
    }

    pub fn endpoint(&self) -> &Path {
        &self.endpoint
    }

    pub fn identity(&self) -> Option<&WorkerIdentity> {
        self.controller.as_ref().map(Controller::identity)
    }

    fn record_bootstrap(&mut self, mut bootstrap: Bootstrap) -> FollowFrame {
        if bootstrap.resumed {
            bootstrap
                .fresh
                .retain(|session| !self.already_published(session));
        }
        for session in &bootstrap.sessions {
            self.remember(session);
        }
        FollowFrame::Bootstrap(Box::new(bootstrap))
    }

    fn already_published(&self, session: &Value) -> bool {
        let Some(key) = session["session"].as_str() else {
            return false;
        };
        self.published.get(key) == Some(&row_fingerprint(session))
    }

    fn remember(&mut self, session: &Value) {
        if let Some(key) = session["session"].as_str() {
            self.published
                .insert(key.to_owned(), row_fingerprint(session));
        }
    }

    fn remember_event(&mut self, event: &Value) {
        let Some(key) = event["session"].as_str() else {
            return;
        };
        let fresh = row_fingerprint(event);
        let held = self
            .published
            .entry(key.to_owned())
            .or_insert_with(|| json!({"session": key}));
        let (Some(held), Some(fresh)) = (held.as_object_mut(), fresh.as_object()) else {
            return;
        };
        for (field, value) in fresh {
            held.insert(field.clone(), value.clone());
        }
    }

    pub fn next(&mut self, timeout: Duration) -> FollowFrame {
        if let Some(frame) = self.pending.take() {
            return frame;
        }
        let deadline = Instant::now() + timeout;
        loop {
            if self.controller.is_none() {
                match self.reconnect() {
                    Some(frame) => return frame,
                    None if Instant::now() >= deadline => return FollowFrame::Idle,
                    None => {
                        std::thread::sleep(RECONNECT_POLL);
                        continue;
                    }
                }
            }
            let slice = READ_SLICE.min(deadline.saturating_duration_since(Instant::now()));
            let read = self
                .controller
                .as_mut()
                .map(|controller| controller.control.read_stream_line(slice));
            match read {
                Some(Ok(Some(line))) => {
                    if let Some(frame) = self.classify(&line) {
                        return frame;
                    }
                }
                Some(Ok(None)) => return self.drop_link("the worker closed the agent stream"),
                Some(Err(error)) if error.kind() == io::ErrorKind::TimedOut => {}
                Some(Err(error)) if error.kind() == io::ErrorKind::WouldBlock => {}
                Some(Err(error)) => return self.drop_link(&error.to_string()),
                None => {}
            }
            if Instant::now() >= deadline {
                return FollowFrame::Idle;
            }
        }
    }

    fn classify(&mut self, line: &str) -> Option<FollowFrame> {
        let value: Value = serde_json::from_str(line.trim()).ok()?;
        match value["type"].as_str() {
            Some("event") => {
                self.remember_event(&value);
                Some(FollowFrame::Event(Box::new(value)))
            }
            Some("snapshot") => {
                for session in value["sessions"].as_array()? {
                    self.remember(session);
                }
                None
            }
            Some("end") => {
                let reason = value["reason"]
                    .as_str()
                    .unwrap_or("the worker ended the agent stream")
                    .to_owned();
                Some(self.drop_link(&reason))
            }
            _ => None,
        }
    }

    fn drop_link(&mut self, reason: &str) -> FollowFrame {
        self.controller = None;
        FollowFrame::Disconnected(reason.to_owned())
    }

    fn reconnect(&mut self) -> Option<FollowFrame> {
        let mut controller = match Controller::connect(&self.endpoint) {
            Ok(controller) => controller,
            Err(ControllerError::PermissionDenied(path)) => {
                return Some(FollowFrame::Disconnected(
                    ControllerError::PermissionDenied(path).to_string(),
                ));
            }
            Err(_) => return None,
        };
        let bootstrap = controller.start_follow(true).ok()?;
        self.controller = Some(controller);
        Some(self.record_bootstrap(bootstrap))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_row(id: &str, status: &str) -> Value {
        json!({"session": id, "status": status, "unread": false, "updated_at_ms": 1})
    }

    fn bootstrap(sessions: Vec<Value>, resumed: bool) -> Bootstrap {
        Bootstrap {
            identity: Box::new(identity()),
            core_connected: true,
            fresh: sessions.clone(),
            sessions,
            resumed,
        }
    }

    fn follow_session() -> FollowSession {
        FollowSession {
            endpoint: PathBuf::from("endpoint"),
            controller: None,
            published: BTreeMap::new(),
            pending: None,
        }
    }

    #[test]
    fn a_resumed_bootstrap_keeps_only_the_rows_that_moved_while_the_link_was_down() {
        let mut session = follow_session();
        let first = session.record_bootstrap(bootstrap(
            vec![session_row("a", "idle"), session_row("b", "busy")],
            false,
        ));
        let FollowFrame::Bootstrap(first) = first else {
            panic!("the first frame is a bootstrap");
        };
        assert_eq!(first.fresh.len(), 2);

        let resumed = session.record_bootstrap(bootstrap(
            vec![
                json!({"session": "a", "status": "idle", "unread": false, "updated_at_ms": 99}),
                session_row("b", "idle"),
            ],
            true,
        ));
        let FollowFrame::Bootstrap(resumed) = resumed else {
            panic!("the resumed frame is a bootstrap");
        };
        assert_eq!(
            resumed.sessions.len(),
            2,
            "the resumed bootstrap still carries the whole fleet"
        );
        assert_eq!(
            resumed
                .fresh
                .iter()
                .map(|row| row["session"].as_str().unwrap_or_default())
                .collect::<Vec<_>>(),
            vec!["b"],
            "a row whose only change is its recency stamp is never printed twice across a reconnect"
        );
    }

    #[test]
    fn an_event_updates_the_published_row_so_a_reconnect_does_not_replay_it() {
        let mut session = follow_session();
        let _ = session.record_bootstrap(bootstrap(vec![session_row("a", "idle")], false));
        session.remember_event(&json!({
            "type": "event",
            "session": "a",
            "status": "busy",
            "unread": true,
            "updated_at_ms": 42,
        }));
        let resumed = session.record_bootstrap(bootstrap(
            vec![json!({"session": "a", "status": "busy", "unread": true, "updated_at_ms": 77})],
            true,
        ));
        let FollowFrame::Bootstrap(resumed) = resumed else {
            panic!("the resumed frame is a bootstrap");
        };
        assert!(
            resumed.fresh.is_empty(),
            "a row the stream already carried is not replayed by the fresh bootstrap"
        );
    }

    #[test]
    fn a_capability_the_worker_does_not_advertise_is_refused_without_a_request() {
        let controller = Controller {
            endpoint: PathBuf::from("endpoint"),
            control: unreachable_control(),
            identity: identity(),
        };
        assert!(controller.advertises(CAPABILITY_AGENT_FOLLOW));
        let refused = controller
            .require(crate::protocol::CAPABILITY_SESSION_RUNTIME_RESUME)
            .expect_err("an unadvertised capability is refused");
        assert_eq!(
            refused.to_string(),
            "capability not advertised: session.runtime.resume"
        );
    }

    fn identity() -> WorkerIdentity {
        WorkerIdentity {
            name: "paneflow-serve".to_string(),
            version: "0.0.0".to_string(),
            build_id: "test".to_string(),
            protocol: crate::protocol::WORKER_PROTOCOL_VERSION,
            required_core_protocol: crate::protocol::REQUIRED_CORE_PROTOCOL,
            pid: 1,
            home: "home".to_string(),
            endpoint: "endpoint".to_string(),
            started_at_ms: 0,
            capabilities: crate::protocol::advertised_capabilities(),
        }
    }

    fn unreachable_control() -> HostControl {
        let home = tempfile::tempdir().expect("a temporary home");
        let endpoint = paneflow_home::serve_endpoint_path(home.path());
        let running = crate::open(home.path()).expect("a worker takes the home");
        let control = HostControl::open(&endpoint, CLIENT_NAME, REQUEST_DEADLINE)
            .expect("the worker answers the handshake");
        running.stop();
        control
    }
}
