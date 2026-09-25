use std::time::Duration;

use base64::Engine as _;
use paneflow_config::schema::HostInstanceToken;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use paneflow_ipc_client::host_control::{
    ERR_NO_CONTROLLER, ERR_SESSION_NOT_FOUND, HOST_PROTOCOL_VERSION, MAX_CONTROL_FRAME_BYTES,
    METHOD_AGENT_EVENT, METHOD_AGENT_FOLLOW, METHOD_AGENT_SNAPSHOT,
};

pub const LOCAL_BUILD_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const MAX_CHECKPOINT_BYTES: usize = 64 * 1024 * 1024;

pub const MAX_OUTPUT_TAIL_BYTES: usize = 8 * 1024 * 1024;

pub const REQUEST_DEADLINE: Duration = Duration::from_secs(10);

pub const DATA_CHUNK_RAW_BYTES: usize = 32 * 1024;

const _: () = assert!(DATA_CHUNK_RAW_BYTES * 4 / 3 + 1024 <= MAX_CONTROL_FRAME_BYTES);

pub const ERR_PARSE: i64 = -32700;
pub const ERR_INVALID_REQUEST: i64 = -32600;
pub const ERR_METHOD_NOT_FOUND: i64 = -32601;
pub const ERR_INVALID_PARAMS: i64 = -32602;
pub const ERR_INTERNAL: i64 = -32603;
pub const ERR_BUSY: i64 = -32000;
pub const ERR_HANDSHAKE_REQUIRED: i64 = -32010;
pub const ERR_INCOMPATIBLE: i64 = -32011;
pub const ERR_GENERATION_MISMATCH: i64 = -32021;
pub const ERR_SESSION_NOT_LIVE: i64 = -32022;
pub const ERR_PROCESS_UNVERIFIED: i64 = -32023;
pub const ERR_OUTPUT_EVICTED: i64 = -32024;
pub const ERR_CHECKPOINT_TOO_LARGE: i64 = -32025;
pub const ERR_FRAME_TOO_LARGE: i64 = -32026;
pub const ERR_SPAWN_FAILED: i64 = -32027;
pub const ERR_DEADLINE: i64 = -32028;
pub const ERR_SESSION_LIVE: i64 = -32029;
pub const ERR_ENGINE_REQUIRED: i64 = -32031;
pub const ERR_LAUNCH_PENDING: i64 = -32032;
pub const ERR_OWNERSHIP_UNRESOLVED: i64 = -32033;
pub const ERR_SHUTTING_DOWN: i64 = -32034;
pub const ERR_DURABILITY: i64 = -32035;

pub const METHODS: &[&str] = &[
    "host.hello",
    "host.status",
    "host.shutdown",
    "session.list",
    "session.create",
    "session.inspect",
    "session.stop",
    "session.restart",
    "session.remove",
    "session.attach",
    "session.output",
    "session.input",
    "session.runtime.bind",
    "session.resize",
    "session.text",
    METHOD_AGENT_SNAPSHOT,
    METHOD_AGENT_EVENT,
    METHOD_AGENT_FOLLOW,
    "surface.list",
    "surface.read",
    "surface.search",
    "surface.status",
    "surface.send_text",
    "fleet.list",
    "system.capabilities",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineIdentity {
    pub engine: String,
    pub app_version: String,
    pub api_version: String,
    pub source_sha: String,
}

pub fn host_build_id() -> String {
    let build = paneflow_terminal_ghostty::build_identity();
    let engine: String = build.source_sha.chars().take(12).collect();
    format!("{LOCAL_BUILD_VERSION}+{engine}")
}

pub fn local_engine_identity() -> EngineIdentity {
    let build = paneflow_terminal_ghostty::build_identity();
    EngineIdentity {
        engine: "libghostty-vt".to_string(),
        app_version: paneflow_terminal_ghostty::GHOSTTY_APP_VERSION.to_string(),
        api_version: build.api_version.to_string(),
        source_sha: build.source_sha.to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostIdentity {
    pub name: String,
    pub version: String,
    pub protocol: u32,
    pub build_id: String,
    pub host_instance: HostInstanceToken,
    pub engine: EngineIdentity,
    pub pid: u32,
    pub home: String,
    pub endpoint: String,
    pub started_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientHello {
    pub client: String,
    pub protocol: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<EngineIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
}

impl ClientHello {
    pub fn local(client: impl Into<String>) -> Self {
        Self {
            client: client.into(),
            protocol: HOST_PROTOCOL_VERSION,
            engine: Some(local_engine_identity()),
            build: Some(LOCAL_BUILD_VERSION.to_string()),
        }
    }

    pub fn control(client: impl Into<String>) -> Self {
        Self {
            client: client.into(),
            protocol: HOST_PROTOCOL_VERSION,
            engine: None,
            build: None,
        }
    }

    pub fn attaches(&self) -> bool {
        self.engine.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Incompatibility {
    #[error("host protocol {offered} does not match the expected protocol {expected}")]
    Protocol { expected: u32, offered: u32 },
    #[error("terminal engine {field} mismatch: expected {expected}, host offers {offered}")]
    Engine {
        field: String,
        expected: String,
        offered: String,
    },
    #[error("host build {offered} does not match this build {expected}")]
    Build { expected: String, offered: String },
}

pub fn check_compatibility(
    expected_protocol: u32,
    expected_engine: &EngineIdentity,
    offered_protocol: u32,
    offered_engine: Option<&EngineIdentity>,
) -> Result<(), Incompatibility> {
    if expected_protocol != offered_protocol {
        return Err(Incompatibility::Protocol {
            expected: expected_protocol,
            offered: offered_protocol,
        });
    }
    let Some(offered_engine) = offered_engine else {
        return Ok(());
    };
    let fields = [
        ("engine", &expected_engine.engine, &offered_engine.engine),
        (
            "api_version",
            &expected_engine.api_version,
            &offered_engine.api_version,
        ),
        (
            "source_sha",
            &expected_engine.source_sha,
            &offered_engine.source_sha,
        ),
    ];
    for (field, expected, offered) in fields {
        if expected != offered {
            return Err(Incompatibility::Engine {
                field: field.to_string(),
                expected: expected.clone(),
                offered: offered.clone(),
            });
        }
    }
    Ok(())
}

pub fn build_drift(expected: &str, offered: Option<&str>) -> Option<Incompatibility> {
    let offered = offered?;
    if expected == offered {
        return None;
    }
    Some(Incompatibility::Build {
        expected: expected.to_string(),
        offered: offered.to_string(),
    })
}

pub fn encode_data(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn decode_data(text: &str) -> Result<Vec<u8>, String> {
    if text.len() > MAX_CONTROL_FRAME_BYTES {
        return Err("data payload exceeds the control frame limit".to_string());
    }
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(|e| format!("invalid base64 payload: {e}"))
}

pub fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

pub fn result_envelope(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

pub fn error_envelope(
    id: &Value,
    code: i64,
    message: impl Into<String>,
    data: Option<Value>,
) -> Value {
    let mut error = json!({"code": code, "message": message.into()});
    if let Some(data) = data
        && let Some(map) = error.as_object_mut()
    {
        map.insert("data".to_string(), data);
    }
    json!({"jsonrpc": "2.0", "id": id, "error": error})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatibility_requires_protocol_and_engine_identity_to_match() {
        let engine = local_engine_identity();
        assert_eq!(engine.source_sha.len(), 40);
        assert_eq!(
            check_compatibility(
                HOST_PROTOCOL_VERSION,
                &engine,
                HOST_PROTOCOL_VERSION,
                Some(&engine)
            ),
            Ok(())
        );
        assert_eq!(
            check_compatibility(HOST_PROTOCOL_VERSION, &engine, HOST_PROTOCOL_VERSION, None),
            Ok(()),
            "a control-only client never claims snapshot compatibility"
        );
        assert!(ClientHello::local("desktop").attaches());
        assert!(!ClientHello::control("paneflow-cli").attaches());
        assert_eq!(
            check_compatibility(
                HOST_PROTOCOL_VERSION,
                &engine,
                HOST_PROTOCOL_VERSION + 1,
                Some(&engine)
            ),
            Err(Incompatibility::Protocol {
                expected: HOST_PROTOCOL_VERSION,
                offered: HOST_PROTOCOL_VERSION + 1
            })
        );
        let mut other = engine.clone();
        other.source_sha = "0".repeat(40);
        let error = check_compatibility(
            HOST_PROTOCOL_VERSION,
            &engine,
            HOST_PROTOCOL_VERSION,
            Some(&other),
        )
        .unwrap_err();
        assert!(
            matches!(error, Incompatibility::Engine { ref field, .. } if field == "source_sha")
        );
    }

    #[test]
    fn data_chunks_round_trip_and_oversized_payloads_are_refused() {
        let bytes: Vec<u8> = (0..=255u8).collect();
        assert_eq!(decode_data(&encode_data(&bytes)).unwrap(), bytes);
        let huge = "A".repeat(MAX_CONTROL_FRAME_BYTES + 1);
        assert!(decode_data(&huge).is_err());
        assert!(decode_data("not base64!").is_err());
    }

    #[test]
    fn a_full_chunk_fits_inside_one_control_frame() {
        let chunk = vec![0xffu8; DATA_CHUNK_RAW_BYTES];
        let line = json!({"type": "chunk", "index": 1_000_000u64, "data": encode_data(&chunk)});
        assert!(serde_json::to_vec(&line).unwrap().len() < MAX_CONTROL_FRAME_BYTES);
    }

    #[test]
    fn a_build_mismatch_is_reported_as_drift_and_control_clients_skip_the_check() {
        assert_eq!(build_drift(LOCAL_BUILD_VERSION, None), None);
        assert_eq!(
            build_drift(LOCAL_BUILD_VERSION, Some(LOCAL_BUILD_VERSION)),
            None
        );
        assert_eq!(
            build_drift("0.15.1", Some("0.15.0")),
            Some(Incompatibility::Build {
                expected: "0.15.1".to_string(),
                offered: "0.15.0".to_string()
            })
        );
    }

    #[test]
    fn only_attaching_hellos_carry_a_build() {
        assert_eq!(
            ClientHello::local("test").build.as_deref(),
            Some(LOCAL_BUILD_VERSION)
        );
        assert_eq!(ClientHello::control("test").build, None);
    }
}
