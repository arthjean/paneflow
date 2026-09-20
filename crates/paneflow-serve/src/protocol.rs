use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub use paneflow_host::protocol::{
    ERR_BUSY, ERR_FRAME_TOO_LARGE, ERR_HANDSHAKE_REQUIRED, ERR_INCOMPATIBLE, ERR_INTERNAL,
    ERR_INVALID_PARAMS, ERR_INVALID_REQUEST, ERR_METHOD_NOT_FOUND, ERR_PARSE,
    MAX_CONTROL_FRAME_BYTES, METHOD_AGENT_EVENT, METHOD_AGENT_FOLLOW, METHOD_AGENT_SNAPSHOT,
    error_envelope, result_envelope,
};

pub const WORKER_PROTOCOL_VERSION: u32 = 1;

pub const REQUIRED_CORE_PROTOCOL: u32 = 1;

pub const LOCAL_BUILD_VERSION: &str = env!("CARGO_PKG_VERSION");

pub const METHOD_WORKER_HELLO: &str = "worker.hello";
pub const METHOD_WORKER_STATUS: &str = "worker.status";
pub const METHOD_WORKER_SHUTDOWN: &str = "worker.shutdown";
pub const METHOD_HOST_HELLO: &str = "host.hello";

pub const REQUEST_DEADLINE: Duration = Duration::from_secs(10);

pub const RESTART_RECOMMENDED: &str = "restart_recommended";

const CAPABILITY_FILE: &str = include_str!("../../../protocol/host-capabilities-v1.json");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerIdentity {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub build_id: String,
    pub protocol: u32,
    pub required_core_protocol: u32,
    pub pid: u32,
    pub home: String,
    pub endpoint: String,
    pub started_at_ms: u64,
    pub capabilities: Vec<String>,
}

pub fn executable_build_id(path: &std::path::Path) -> std::io::Result<String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub fn advertised_capabilities() -> Vec<String> {
    let document: Value = match serde_json::from_str(CAPABILITY_FILE) {
        Ok(document) => document,
        Err(error) => {
            log::error!("paneflow-serve: the capability file is unreadable: {error}");
            return Vec::new();
        }
    };
    document["capabilities"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

pub fn restart_recommendation(core_protocol: u32) -> Option<Value> {
    if core_protocol >= REQUIRED_CORE_PROTOCOL {
        return None;
    }
    Some(json!({
        "token": RESTART_RECOMMENDED,
        "required_protocol": REQUIRED_CORE_PROTOCOL,
        "core_protocol": core_protocol,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_capability_file_is_the_only_source_of_the_advertised_set() {
        let advertised = advertised_capabilities();
        assert!(advertised.contains(&"agent.snapshot".to_string()));
        assert!(advertised.contains(&"agent.follow".to_string()));
        assert!(advertised.contains(&"restart.recommendation".to_string()));
        assert!(advertised.contains(&"integrations.refresh".to_string()));
        let document: Value = serde_json::from_str(CAPABILITY_FILE).unwrap();
        assert_eq!(
            document["version"].as_u64(),
            Some(u64::from(WORKER_PROTOCOL_VERSION))
        );
        assert_eq!(
            document["core_protocol"].as_u64(),
            Some(u64::from(REQUIRED_CORE_PROTOCOL))
        );
    }

    #[test]
    fn an_older_core_earns_a_stable_restart_token_instead_of_a_failure() {
        assert_eq!(restart_recommendation(REQUIRED_CORE_PROTOCOL), None);
        assert_eq!(restart_recommendation(REQUIRED_CORE_PROTOCOL + 1), None);
        let recommendation =
            restart_recommendation(REQUIRED_CORE_PROTOCOL - 1).expect("an older core is flagged");
        assert_eq!(recommendation["token"], RESTART_RECOMMENDED);
        assert_eq!(
            recommendation["required_protocol"].as_u64(),
            Some(u64::from(REQUIRED_CORE_PROTOCOL))
        );
        assert_eq!(
            restart_recommendation(REQUIRED_CORE_PROTOCOL - 1).map(|value| value["token"].clone()),
            Some(Value::from(RESTART_RECOMMENDED)),
            "the token changes only when the required version changes"
        );
    }
}
