use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const MAX_AGENT_LEASE_MS: u64 = 30_000;
pub const MAX_AGENT_ACTION_MS: u64 = 10_000;
pub const MAX_AGENT_OPERATIONS_PER_WORKSPACE: usize = 4;
pub const MAX_AGENT_OPERATIONS_TOTAL: usize = 16;
pub const MAX_AGENT_OPERATION_RECORDS: usize = 1_024;
pub const MAX_AGENT_SNAPSHOT_BYTES: usize = 256 * 1024;
pub const MAX_AGENT_SNAPSHOT_NODES: usize = 2_000;
pub const MAX_AGENT_CAPTURE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_AGENT_CAPTURE_CHUNK_BYTES: usize = 128 * 1024;
pub const MAX_AGENT_CAPTURE_PIXELS: u64 = 16_000_000;
pub const MAX_AGENT_DIAGNOSTIC_ENTRIES: usize = 1_000;
pub const MAX_AGENT_DIAGNOSTIC_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_AGENT_DIAGNOSTIC_AGE_SECS: u64 = 15 * 60;
pub const MAX_AGENT_TYPING_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentAccess {
    #[default]
    Disabled,
    Read,
    Interact,
}

impl AgentAccess {
    pub fn permits_read(self) -> bool {
        matches!(self, Self::Read | Self::Interact)
    }

    pub fn permits_interact(self) -> bool {
        matches!(self, Self::Interact)
    }
}

pub fn exported_origin(value: &str) -> Option<String> {
    let parsed = url::Url::parse(value).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let host = parsed.host_str()?;
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    let authority = match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    };
    Some(format!("{}://{authority}", parsed.scheme()))
}

pub fn exported_url(value: &str) -> Option<String> {
    let mut parsed = url::Url::parse(value).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    parsed.set_query(None);
    parsed.set_fragment(None);
    Some(parsed.into())
}

pub fn redact_headers(headers: &Map<String, Value>) -> Map<String, Value> {
    headers
        .iter()
        .map(|(name, value)| {
            let redacted = if is_sensitive_header(name) {
                Value::String("[REDACTED]".to_string())
            } else {
                value.clone()
            };
            (name.clone(), redacted)
        })
        .collect()
}

pub fn redact_query_parameters(value: &str) -> Option<String> {
    let mut parsed = url::Url::parse(value).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let pairs = parsed
        .query_pairs()
        .filter(|(name, _)| !is_sensitive_query_parameter(name))
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    let query = (!pairs.is_empty()).then(|| {
        pairs
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("&")
    });
    parsed.set_query(query.as_deref());
    parsed.set_fragment(None);
    Some(parsed.into())
}

pub fn is_sensitive_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization" | "cookie" | "set-cookie" | "proxy-authorization"
    )
}

pub fn is_sensitive_query_parameter(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "token",
        "access_token",
        "refresh_token",
        "id_token",
        "api_key",
        "apikey",
        "key",
        "secret",
        "password",
        "passwd",
        "session",
        "session_id",
    ]
    .iter()
    .any(|candidate| name == *candidate || name.ends_with(&format!("_{candidate}")))
}

pub fn cap_text(value: &str, maximum: usize) -> (String, bool) {
    if value.len() <= maximum {
        return (value.to_string(), false);
    }
    let mut end = maximum;
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    (value[..end].to_string(), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn exported_urls_drop_query_and_fragment() {
        assert_eq!(
            exported_url("https://example.com/path?token=secret#section").as_deref(),
            Some("https://example.com/path")
        );
        assert_eq!(
            exported_origin("https://example.com/path?token=secret#section").as_deref(),
            Some("https://example.com")
        );
    }

    #[test]
    fn sensitive_query_parameters_are_removed_without_inventing_content() {
        assert_eq!(
            redact_query_parameters("https://example.com/path?token=secret&mode=full#x").as_deref(),
            Some("https://example.com/path?mode=full")
        );
    }

    #[test]
    fn headers_redact_credentials_case_insensitively() {
        let headers = json!({"Authorization": "Bearer secret", "X-Trace": "ok"});
        let redacted = redact_headers(headers.as_object().unwrap());
        assert_eq!(redacted["Authorization"], "[REDACTED]");
        assert_eq!(redacted["X-Trace"], "ok");
    }

    #[test]
    fn text_cap_preserves_utf8_boundaries() {
        let (value, truncated) = cap_text("a😀b", 3);
        assert_eq!(value, "a");
        assert!(truncated);
    }
}
