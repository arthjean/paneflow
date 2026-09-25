#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

pub(crate) const JSONRPC_ERROR_KEY: &str = "_jsonrpc_error";

impl JsonRpcError {
    pub(crate) const INVALID_PARAMS: i32 = -32602;
    pub(crate) const METHOD_NOT_ENABLED: i32 = -32601;
    pub(crate) const METHOD_NOT_FOUND: i32 = -32601;

    pub(crate) fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: Self::INVALID_PARAMS,
            message: message.into(),
        }
    }

    pub(crate) fn method_not_enabled(message: impl Into<String>) -> Self {
        Self {
            code: Self::METHOD_NOT_ENABLED,
            message: message.into(),
        }
    }

    pub(crate) fn method_not_found(message: impl Into<String>) -> Self {
        Self {
            code: Self::METHOD_NOT_FOUND,
            message: message.into(),
        }
    }

    pub(crate) fn into_value(self) -> serde_json::Value {
        serde_json::json!({
            JSONRPC_ERROR_KEY: {
                "code": self.code,
                "message": self.message,
            }
        })
    }
}

pub(crate) fn promote_response(
    handler_result: serde_json::Value,
    id: serde_json::Value,
) -> serde_json::Value {
    if let Some(err) = handler_result.get(JSONRPC_ERROR_KEY) {
        let code = err.get("code").and_then(|c| c.as_i64()).unwrap_or(-32603);
        let message = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("Unknown error")
            .to_string();
        return serde_json::json!({
            "jsonrpc": "2.0",
            "error": { "code": code, "message": message },
            "id": id,
        });
    }
    if let Some(message) = handler_result.get("error").and_then(|m| m.as_str()) {
        return serde_json::json!({
            "jsonrpc": "2.0",
            "error": { "code": -32603, "message": message },
            "id": id,
        });
    }
    serde_json::json!({
        "jsonrpc": "2.0",
        "result": handler_result,
        "id": id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promote_response_wraps_value_under_result_by_default() {
        let id = serde_json::json!(7);
        let resp = promote_response(serde_json::json!({"index": 0, "title": "ws"}), id);
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 7);
        assert_eq!(resp["result"]["index"], 0);
        assert!(resp.get("error").is_none());
    }

    #[test]
    fn promote_response_extracts_jsonrpc_error_sentinel() {
        let err_val = JsonRpcError::invalid_params("bad layout").into_value();
        let resp = promote_response(err_val, serde_json::json!("req-1"));
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], "req-1");
        assert!(resp.get("result").is_none());
        assert_eq!(resp["error"]["code"], -32602);
        assert_eq!(resp["error"]["message"], "bad layout");
    }

    #[test]
    fn promote_response_promotes_legacy_application_error_strings() {
        let id = serde_json::json!(null);
        let legacy = serde_json::json!({"error": "Workspace limit reached"});
        let resp = promote_response(legacy, id);
        assert_eq!(resp["error"]["code"], -32603);
        assert_eq!(resp["error"]["message"], "Workspace limit reached");
        assert!(resp.get("result").is_none());
    }
}
