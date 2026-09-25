use super::*;

pub(super) fn ipc_scripting_enabled() -> bool {
    scripting_enabled_from(std::env::var("PANEFLOW_IPC_SCRIPTING").ok().as_deref())
}

fn scripting_enabled_from(value: Option<&str>) -> bool {
    matches!(value, Some("1"))
}

pub(super) fn ipc_orchestration_enabled() -> bool {
    orchestration_enabled_from(
        std::env::var("PANEFLOW_IPC_ORCHESTRATION").ok().as_deref(),
        std::env::var("PANEFLOW_IPC_SCRIPTING").ok().as_deref(),
    )
}

fn orchestration_enabled_from(orchestration: Option<&str>, scripting: Option<&str>) -> bool {
    matches!(orchestration, Some("1")) || scripting_enabled_from(scripting)
}

fn env_param_has_strings(value: Option<&serde_json::Value>) -> bool {
    value
        .and_then(|v| v.as_object())
        .is_some_and(|obj| obj.values().any(serde_json::Value::is_string))
}

fn string_param_is_nonempty(value: Option<&serde_json::Value>) -> bool {
    value
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty())
}

pub(super) fn pane_spec_requires_orchestration(spec: &serde_json::Value) -> bool {
    string_param_is_nonempty(spec.get("command"))
        || string_param_is_nonempty(spec.get("prompt"))
        || env_param_has_strings(spec.get("env"))
}

pub(super) fn orchestration_disabled_error(method: &str) -> JsonRpcError {
    JsonRpcError::method_not_enabled(format!(
        "{method} orchestration disabled; set PANEFLOW_IPC_ORCHESTRATION=1 \
         or PANEFLOW_IPC_SCRIPTING=1 to enable command, prompt, or env"
    ))
}

pub(super) fn send_text_gate_open(scripting_enabled: bool, unrestricted: bool) -> bool {
    scripting_enabled || unrestricted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_text_rejected_when_scripting_disabled() {
        assert!(
            !super::scripting_enabled_from(None),
            "unset env must read as disabled"
        );
        assert!(
            !super::scripting_enabled_from(Some("")),
            "empty string must read as disabled"
        );
        assert!(
            !super::scripting_enabled_from(Some("0")),
            "explicit 0 must read as disabled"
        );
        assert!(
            !super::scripting_enabled_from(Some("true")),
            "truthy strings other than \"1\" must read as disabled"
        );
        assert!(
            super::scripting_enabled_from(Some("1")),
            "the documented opt-in value must enable"
        );

        let err = JsonRpcError {
            code: -32601,
            message: "surface.send_text disabled; set PANEFLOW_IPC_SCRIPTING=1 to enable"
                .to_string(),
        };
        let envelope = promote_response(err.into_value(), serde_json::json!(42));
        assert_eq!(envelope["error"]["code"], -32601);
        assert!(envelope.get("result").is_none());
        assert_eq!(envelope["id"], 42);
    }

    #[test]
    fn send_text_gate_opens_for_env_or_free_access() {
        assert!(
            !super::send_text_gate_open(false, false),
            "both off must stay closed (unchanged legacy behavior)"
        );
        assert!(
            super::send_text_gate_open(true, false),
            "the env gate alone still opens it"
        );
        assert!(
            super::send_text_gate_open(false, true),
            "free-access mode opens it without the env gate"
        );
        assert!(super::send_text_gate_open(true, true));
    }

    #[test]
    fn orchestration_gate_accepts_specific_gate_or_scripting_superset() {
        assert!(!super::orchestration_enabled_from(None, None));
        assert!(!super::orchestration_enabled_from(Some("0"), Some("0")));
        assert!(super::orchestration_enabled_from(Some("1"), None));
        assert!(super::orchestration_enabled_from(None, Some("1")));
    }

    #[test]
    fn pane_spec_requires_orchestration_for_spawn_primitives_only() {
        assert!(!super::pane_spec_requires_orchestration(
            &serde_json::json!({"cwd": "."})
        ));
        assert!(super::pane_spec_requires_orchestration(
            &serde_json::json!({"command": "cargo test"})
        ));
        assert!(super::pane_spec_requires_orchestration(
            &serde_json::json!({"prompt": "inspect this"})
        ));
        assert!(!super::pane_spec_requires_orchestration(
            &serde_json::json!({"context": "notes"})
        ));
        assert!(super::pane_spec_requires_orchestration(
            &serde_json::json!({"env": {"PROMPT_COMMAND": "date"}})
        ));
        assert!(!super::pane_spec_requires_orchestration(
            &serde_json::json!({"env": {"IGNORED": 7}})
        ));
    }
}
