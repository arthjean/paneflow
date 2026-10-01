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

fn surface_requires_orchestration(surface: &paneflow_config::schema::SurfaceDefinition) -> bool {
    let non_empty = |value: &Option<String>| value.as_deref().is_some_and(|text| !text.is_empty());
    surface.env.as_ref().is_some_and(|env| !env.is_empty())
        || surface.session.is_some()
        || non_empty(&surface.scrollback)
        || non_empty(&surface.command)
        || non_empty(&surface.prompt)
}

pub(super) fn layout_requires_orchestration(node: &LayoutNode) -> bool {
    match node {
        LayoutNode::Pane { surfaces } => surfaces.iter().any(surface_requires_orchestration),
        LayoutNode::Split { children, .. } => children.iter().any(layout_requires_orchestration),
    }
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

pub(super) fn log_pane_write(
    method: &str,
    surface_id: u64,
    caller_pid: Option<i64>,
    length: usize,
    unrestricted: bool,
    forced: bool,
) {
    let authorization = if unrestricted {
        "ai_unrestricted"
    } else {
        "scripting"
    };
    tracing::info!(
        target: "paneflow::ipc::write",
        method,
        surface_id,
        caller_pid = ?caller_pid,
        length = length as u64,
        authorization,
        forced,
        "authorized PTY write to pane"
    );
}

pub(super) fn capabilities_value(scripting: bool, orchestration: bool) -> serde_json::Value {
    let methods = [
        "system.ping",
        "system.capabilities",
        "system.identify",
        "workspace.list",
        "workspace.create",
        "workspace.select",
        "workspace.close",
        "workspace.current",
        "workspace.restore_layout",
        "workspace.up",
        "surface.list",
        "surface.read",
        "surface.search",
        "surface.rename",
        "surface.send_text",
        "surface.send_keystroke",
        "surface.split",
        "surface.focus",
        "surface.status",
        "fleet.list",
        "events.subscribe",
    ];
    serde_json::json!({
        "scripting": scripting,
        "orchestration": orchestration,
        "methods": methods,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tracing_test::traced_test]
    #[test]
    fn every_authorized_pane_write_is_logged_with_its_length_in_both_modes() {
        log_pane_write("surface.send_text", 7, Some(42), 11, false, true);
        log_pane_write("surface.send_keystroke", 8, None, 6, true, false);
        assert!(logs_contain("INFO"));
        assert!(logs_contain("method=\"surface.send_text\" surface_id=7"));
        assert!(logs_contain(
            "length=11 authorization=\"scripting\" forced=true"
        ));
        assert!(logs_contain(
            "method=\"surface.send_keystroke\" surface_id=8"
        ));
        assert!(logs_contain("length=6 authorization=\"ai_unrestricted\""));
    }

    #[test]
    fn capabilities_never_advertise_the_retired_ai_methods() {
        let value = capabilities_value(true, true);
        let methods = value["methods"].as_array().expect("methods");
        assert!(methods.iter().any(|method| method == "events.subscribe"));
        assert!(
            methods
                .iter()
                .filter_map(serde_json::Value::as_str)
                .all(|method| !method.starts_with("ai.")),
            "the app socket answers ai.* with -32601, so it never lists them"
        );
    }

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
    fn capabilities_report_scripting_opened_by_free_access() {
        let caps = capabilities_value(send_text_gate_open(false, true), false);
        assert_eq!(caps["scripting"], true);
        assert_eq!(caps["orchestration"], false);
        let closed = capabilities_value(send_text_gate_open(false, false), false);
        assert_eq!(closed["scripting"], false);
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
    fn layout_surfaces_with_spawn_or_attach_fields_require_orchestration() {
        let layout = |surface: serde_json::Value| -> LayoutNode {
            serde_json::from_value(serde_json::json!({
                "type": "split",
                "direction": "vertical",
                "children": [
                    {"type": "pane", "surfaces": [{"cwd": "."}]},
                    {"type": "pane", "surfaces": [surface]}
                ]
            }))
            .expect("valid layout")
        };
        assert!(!super::layout_requires_orchestration(&layout(
            serde_json::json!({"cwd": "/tmp", "name": "logs"})
        )));
        for gated in [
            serde_json::json!({"env": {"PROMPT_COMMAND": "date"}}),
            serde_json::json!({"session": "123e4567-e89b-42d3-a456-426614174000"}),
            serde_json::json!({"scrollback": "forged history"}),
            serde_json::json!({"command": "make"}),
            serde_json::json!({"prompt": "do it"}),
        ] {
            assert!(
                super::layout_requires_orchestration(&layout(gated.clone())),
                "{gated}"
            );
        }
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
