use std::io;
use std::path::Path;

use paneflow_config::schema::{SessionGeneration, SessionId};
use serde_json::{Value, json};

use crate::protocol::{
    self, ClientHello, HostIdentity, Incompatibility, MAX_CHECKPOINT_BYTES, REQUEST_DEADLINE,
    decode_data, request,
};
use crate::runtime::Checkpoint;
use crate::wire::{LineRead, Wire};

#[derive(Debug, thiserror::Error)]
pub enum HostClientError {
    #[error("local host unreachable at {endpoint}: {source}")]
    Unreachable { endpoint: String, source: io::Error },
    #[error("local host incompatible: {0}")]
    Incompatible(String),
    #[error("host error {code}: {message}")]
    Rpc {
        code: i64,
        message: String,
        data: Option<Value>,
    },
    #[error("host protocol violation: {0}")]
    Protocol(String),
    #[error("connection to the local host failed: {0}")]
    Io(#[from] io::Error),
}

impl HostClientError {
    pub fn code(&self) -> Option<i64> {
        match self {
            Self::Rpc { code, .. } => Some(*code),
            _ => None,
        }
    }
}

pub struct HostClient {
    wire: Wire,
    next_id: u64,
    identity: HostIdentity,
}

impl HostClient {
    pub fn connect(endpoint: &Path, hello: &ClientHello) -> Result<Self, HostClientError> {
        let wire = Wire::connect(endpoint).map_err(|source| HostClientError::Unreachable {
            endpoint: endpoint.display().to_string(),
            source,
        })?;
        let mut client = Self {
            wire,
            next_id: 1,
            identity: HostIdentity {
                name: String::new(),
                version: String::new(),
                protocol: 0,
                host_instance: paneflow_config::schema::HostInstanceToken::new(),
                engine: hello.engine.clone(),
                pid: 0,
                home: String::new(),
                endpoint: String::new(),
                started_at_ms: 0,
            },
        };
        let offered = match client.call("host.hello", to_value(hello)?) {
            Ok(value) => value,
            Err(HostClientError::Rpc { code, message, .. })
                if code == protocol::ERR_INCOMPATIBLE =>
            {
                return Err(HostClientError::Incompatible(message));
            }
            Err(error) => return Err(error),
        };
        let identity: HostIdentity = serde_json::from_value(offered)
            .map_err(|e| HostClientError::Protocol(format!("invalid host identity: {e}")))?;
        protocol::check_compatibility(
            hello.protocol,
            &hello.engine,
            identity.protocol,
            &identity.engine,
        )
        .map_err(|incompatibility: Incompatibility| {
            HostClientError::Incompatible(incompatibility.to_string())
        })?;
        client.identity = identity;
        Ok(client)
    }

    pub fn identity(&self) -> &HostIdentity {
        &self.identity
    }

    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, HostClientError> {
        let id = self.next_id;
        self.next_id += 1;
        self.wire.write_json(&request(id, method, params))?;
        let value = self.read_value()?;
        expect_id(&value, id)?;
        result_or_error(value)
    }

    fn read_value(&mut self) -> Result<Value, HostClientError> {
        match self.wire.read_line(REQUEST_DEADLINE)? {
            LineRead::Line(line) => serde_json::from_str(&line)
                .map_err(|e| HostClientError::Protocol(format!("invalid host frame: {e}"))),
            LineRead::Eof => Err(HostClientError::Protocol(
                "the host closed the connection".to_string(),
            )),
            LineRead::TooLong => Err(HostClientError::Protocol(
                "host frame exceeds the 64 KiB control frame limit".to_string(),
            )),
        }
    }

    pub fn attach(
        &mut self,
        session: &SessionId,
        generation: Option<SessionGeneration>,
    ) -> Result<Checkpoint, HostClientError> {
        let header = self.call(
            "session.attach",
            json!({"session": session, "generation": generation}),
        )?;
        let generation: SessionGeneration = serde_json::from_value(header["generation"].clone())
            .map_err(|e| HostClientError::Protocol(format!("invalid generation: {e}")))?;
        let expected = header["bytes"]
            .as_u64()
            .ok_or_else(|| HostClientError::Protocol("missing checkpoint size".to_string()))?;
        if expected > MAX_CHECKPOINT_BYTES as u64 {
            return Err(HostClientError::Protocol(format!(
                "checkpoint of {expected} bytes exceeds the attachment limit"
            )));
        }
        let cols = header["cols"].as_u64().unwrap_or(0) as u16;
        let rows = header["rows"].as_u64().unwrap_or(0) as u16;
        let offset = header["offset"]
            .as_u64()
            .ok_or_else(|| HostClientError::Protocol("missing checkpoint offset".to_string()))?;
        let mut snapshot = Vec::with_capacity(expected as usize);
        let mut next_index = 0u64;
        loop {
            let frame = self.read_value()?;
            match frame["type"].as_str() {
                Some("chunk") => {
                    if frame["index"].as_u64() != Some(next_index) {
                        return Err(HostClientError::Protocol(
                            "checkpoint chunks arrived out of order".to_string(),
                        ));
                    }
                    next_index += 1;
                    let data = frame["data"].as_str().unwrap_or_default();
                    let bytes = decode_data(data).map_err(HostClientError::Protocol)?;
                    if snapshot.len() + bytes.len() > expected as usize {
                        return Err(HostClientError::Protocol(
                            "checkpoint exceeded its announced size".to_string(),
                        ));
                    }
                    snapshot.extend_from_slice(&bytes);
                }
                Some("end") => break,
                _ => {
                    return Err(result_or_error(frame).err().unwrap_or_else(|| {
                        HostClientError::Protocol(
                            "unexpected frame in checkpoint stream".to_string(),
                        )
                    }));
                }
            }
        }
        if snapshot.len() as u64 != expected {
            return Err(HostClientError::Protocol(format!(
                "checkpoint delivered {} of {expected} bytes",
                snapshot.len()
            )));
        }
        Ok(Checkpoint {
            generation,
            offset,
            cols,
            rows,
            snapshot,
        })
    }

    pub fn output(
        &mut self,
        session: &SessionId,
        generation: Option<SessionGeneration>,
        from: u64,
        follow: bool,
        mut on_output: impl FnMut(u64, &[u8]) -> bool,
    ) -> Result<u64, HostClientError> {
        self.call(
            "session.output",
            json!({"session": session, "generation": generation, "from": from, "follow": follow}),
        )?;
        let mut deliver = true;
        loop {
            let frame = self.read_value()?;
            match frame["type"].as_str() {
                Some("output") => {
                    let offset = frame["offset"].as_u64().ok_or_else(|| {
                        HostClientError::Protocol("output frame without offset".to_string())
                    })?;
                    let data = frame["data"].as_str().unwrap_or_default();
                    let bytes = decode_data(data).map_err(HostClientError::Protocol)?;
                    if deliver && !on_output(offset, &bytes) {
                        deliver = false;
                    }
                }
                Some("end") => {
                    return frame["next_offset"].as_u64().ok_or_else(|| {
                        HostClientError::Protocol("end frame without next_offset".to_string())
                    });
                }
                _ => {
                    return Err(result_or_error(frame).err().unwrap_or_else(|| {
                        HostClientError::Protocol("unexpected frame in output stream".to_string())
                    }));
                }
            }
        }
    }
}

fn to_value<T: serde::Serialize>(value: &T) -> Result<Value, HostClientError> {
    serde_json::to_value(value).map_err(|e| HostClientError::Protocol(e.to_string()))
}

fn expect_id(value: &Value, id: u64) -> Result<(), HostClientError> {
    match value.get("id") {
        Some(Value::Number(n)) if n.as_u64() == Some(id) => Ok(()),
        Some(Value::Null) | None if value.get("error").is_some() => Ok(()),
        other => Err(HostClientError::Protocol(format!(
            "response id {other:?} does not match request {id}"
        ))),
    }
}

fn result_or_error(value: Value) -> Result<Value, HostClientError> {
    if let Some(error) = value.get("error") {
        return Err(HostClientError::Rpc {
            code: error["code"].as_i64().unwrap_or(0),
            message: error["message"]
                .as_str()
                .unwrap_or("unknown error")
                .to_string(),
            data: error.get("data").cloned(),
        });
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| HostClientError::Protocol("frame carries neither result nor error".into()))
}
