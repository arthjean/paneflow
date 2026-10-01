use paneflow_agent_config::runtime_catalog::{
    Runtime, RuntimeLifecycleAuthority, runtime_by_id, runtime_for_tool,
};
use paneflow_ipc_client::IpcTransport;
use paneflow_ipc_client::host_control::HostTransport;
use serde_json::Value;

const REDUCED_METHODS: [&str; 2] = ["surface.status", "fleet.list"];

const PROJECTED_FACTS: [&str; 2] = ["outcome", "menu_prompt_active"];

const FOREGROUND_RUNTIME: &str = "foreground_runtime";

const MAX_BLOCKED_REASON_CHARS: usize = 120;

pub(super) const WORKING: &str = "working";
pub(super) const ATTENTION: &str = "attention";
pub(super) const BLOCKED: &str = "blocked";
pub(super) const IDLE: &str = "idle";

pub(super) fn reduced_state(status: &Value) -> Option<&'static str> {
    match status.get("state").and_then(Value::as_str)? {
        "thinking" => Some(WORKING),
        "waiting_for_input" if waits_without_a_decision(status) => Some(ATTENTION),
        "waiting_for_input" => Some(BLOCKED),
        "errored" => Some(ATTENTION),
        "finished" | "idle" => Some(IDLE),
        _ => None,
    }
}

fn waits_without_a_decision(status: &Value) -> bool {
    status.get("attention_reason").and_then(Value::as_str) == Some("bell")
        || status
            .get("outcome")
            .and_then(Value::as_str)
            .is_some_and(|outcome| outcome.starts_with("failed"))
}

pub(super) fn blocked_reason(status: &Value) -> String {
    if status.get("menu_prompt_active").and_then(Value::as_bool) == Some(true) {
        return "menu prompt".to_string();
    }
    status
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(|message| message.chars().take(MAX_BLOCKED_REASON_CHARS).collect())
        .unwrap_or_else(|| "permission or question".to_string())
}

pub(super) fn agent_runtime(status: &Value) -> Option<&'static Runtime> {
    status
        .get("agent_runtime")
        .and_then(Value::as_str)
        .and_then(runtime_by_id)
        .or_else(|| {
            status
                .get("tool")
                .and_then(Value::as_str)
                .and_then(runtime_for_tool)
        })
}

pub(super) fn reports_turns(runtime: &Runtime) -> bool {
    runtime.lifecycle.authority != RuntimeLifecycleAuthority::None
}

pub(super) fn foreground_departure(status: &Value, runtime: &Runtime) -> Option<String> {
    match status.get(FOREGROUND_RUNTIME)? {
        Value::String(id) if id == runtime.id => None,
        Value::String(id) => {
            Some(runtime_by_id(id).map_or_else(|| id.clone(), |held| held.label.to_string()))
        }
        _ => Some("a shell or another program".to_string()),
    }
}

pub(super) fn with_worker_state(method: &str, mut result: Value) -> Value {
    if !REDUCED_METHODS.contains(&method) {
        return result;
    }
    if let Some(projections) = worker_projections() {
        apply_projections(method, &mut result, &projections);
    }
    result
}

pub(super) fn with_host_foreground(status: &mut Value) {
    if status.get(FOREGROUND_RUNTIME).is_some() {
        return;
    }
    let Some(session) = status.get("session").and_then(Value::as_str) else {
        return;
    };
    let Some(foreground) = host_foreground(session) else {
        return;
    };
    if let Some(object) = status.as_object_mut() {
        object.insert(FOREGROUND_RUNTIME.into(), foreground);
    }
}

fn host_foreground(session: &str) -> Option<Value> {
    let endpoint = paneflow_host::endpoint::host_endpoint_path_for_current_home()?;
    let host = HostTransport::connect(&endpoint, super::CLIENT_NAME).ok()?;
    host.call("surface.status", serde_json::json!({ "session": session }))
        .ok()?
        .get(FOREGROUND_RUNTIME)
        .cloned()
}

fn worker_projections() -> Option<Vec<Value>> {
    let endpoint = paneflow_home::serve_endpoint_path_for_current_home()?;
    let mut controller = paneflow_serve::controller::Controller::connect(&endpoint).ok()?;
    Some(controller.snapshot().ok()?.sessions)
}

fn apply_projections(method: &str, result: &mut Value, projections: &[Value]) {
    match method {
        "surface.status" => reduce_row(result, projections),
        "fleet.list" => {
            if let Some(agents) = result.get_mut("agents").and_then(Value::as_array_mut) {
                for agent in agents {
                    reduce_row(agent, projections);
                }
            }
        }
        _ => {}
    }
}

fn reduce_row(row: &mut Value, projections: &[Value]) {
    let Some(session) = row.get("session").and_then(Value::as_str) else {
        return;
    };
    let Some(projection) = projections.iter().find(|projection| {
        projection.get("session").and_then(Value::as_str) == Some(session)
            && row
                .get("generation")
                .is_none_or(|generation| projection.get("generation") == Some(generation))
    }) else {
        return;
    };
    let state = projection
        .get("activity")
        .and_then(|activity| activity.get("state"))
        .and_then(Value::as_str)
        .unwrap_or("idle");
    let Some(object) = row.as_object_mut() else {
        return;
    };
    object.insert("state".into(), Value::String(state.to_owned()));
    if let Some(state_seq) = projection.get("state_seq").and_then(Value::as_u64) {
        object.insert("state_seq".into(), Value::from(state_seq));
    }
    if let Some(reason) = projection
        .get("attention_reason")
        .filter(|reason| !reason.is_null())
    {
        object.insert("attention_reason".into(), reason.clone());
    }
    for fact in PROJECTED_FACTS {
        if let Some(value) = projection.get(fact) {
            object.insert(fact.into(), value.clone());
        }
    }
    if let Some(activity) = projection
        .get("activity")
        .filter(|activity| activity.is_object())
    {
        if let Some(runtime) = projection.get("runtime_id").filter(|id| id.is_string()) {
            object.insert("agent_runtime".into(), runtime.clone());
        }
        if let Some(message) = activity
            .get("message")
            .filter(|message| message.is_string())
        {
            object.insert("message".into(), message.clone());
        }
    }
    object.insert("reduced_by".into(), Value::String("worker".to_owned()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_host_status_takes_the_reduced_state_and_sequence_of_its_worker_projection() {
        let mut status =
            json!({"session": "a", "generation": 2, "hooked": true, "output_generation": 40});
        let projections = [
            json!({"session": "b", "generation": 2, "state_seq": 9, "activity": {"state": "finished"}}),
            json!({"session": "a", "generation": 2, "state_seq": 4, "activity": {"state": "thinking"}}),
        ];
        apply_projections("surface.status", &mut status, &projections);
        assert_eq!(status["state"], "thinking");
        assert_eq!(status["state_seq"], 4);
        assert_eq!(status["reduced_by"], "worker");
        assert_eq!(status["output_generation"], 40);
    }

    #[test]
    fn without_a_projection_of_the_current_generation_the_state_stays_absent() {
        let mut status = json!({"session": "a", "generation": 3, "hooked": false});
        apply_projections(
            "surface.status",
            &mut status,
            &[
                json!({"session": "a", "generation": 2, "state_seq": 4, "activity": {"state": "thinking"}}),
            ],
        );
        assert!(status.get("state").is_none());
        assert!(status.get("state_seq").is_none());
        apply_projections("surface.status", &mut status, &[]);
        assert!(status.get("state").is_none());
        assert!(!status.to_string().contains("unknown"));
    }

    #[test]
    fn a_status_carries_the_facts_that_decide_a_delivery() {
        let mut status = json!({"session": "a", "generation": 1, "hooked": true});
        apply_projections(
            "surface.status",
            &mut status,
            &[json!({
                "session": "a",
                "generation": 1,
                "state_seq": 6,
                "status": "attention",
                "outcome": null,
                "menu_prompt_active": false,
                "runtime_id": "com.anthropic.claude-code",
                "activity": {"state": "waiting_for_input", "tool": "claude", "message": "Approve edit?"},
            })],
        );
        assert_eq!(reduced_state(&status), Some(BLOCKED));
        assert_eq!(blocked_reason(&status), "Approve edit?");
        let runtime = agent_runtime(&status).expect("the agent line names its runtime");
        assert_eq!(runtime.id, "com.anthropic.claude-code");
        assert_eq!(
            foreground_departure(&status, runtime),
            None,
            "the worker keeps its last observation, so only the host names the foreground"
        );
        status["foreground_runtime"] = json!("com.anthropic.claude-code");
        assert_eq!(foreground_departure(&status, runtime), None);

        status["foreground_runtime"] = Value::Null;
        assert_eq!(
            foreground_departure(&status, runtime).as_deref(),
            Some("a shell or another program")
        );
        status["attention_reason"] = json!("bell");
        assert_eq!(reduced_state(&status), Some(ATTENTION));
    }

    #[test]
    fn the_reduced_state_folds_the_wire_states_into_four_words() {
        for (state, extra, reduced) in [
            ("thinking", json!({}), Some(WORKING)),
            ("waiting_for_input", json!({}), Some(BLOCKED)),
            (
                "waiting_for_input",
                json!({"menu_prompt_active": true}),
                Some(BLOCKED),
            ),
            (
                "waiting_for_input",
                json!({"outcome": "failed:rate limit"}),
                Some(ATTENTION),
            ),
            (
                "waiting_for_input",
                json!({"attention_reason": "bell"}),
                Some(ATTENTION),
            ),
            ("errored", json!({}), Some(ATTENTION)),
            ("finished", json!({}), Some(IDLE)),
            ("idle", json!({}), Some(IDLE)),
            ("unknown", json!({}), None),
        ] {
            let mut status = extra.clone();
            status["state"] = json!(state);
            assert_eq!(reduced_state(&status), reduced, "{state} {extra}");
        }
        assert_eq!(reduced_state(&json!({"hooked": false})), None);
        assert_eq!(
            blocked_reason(&json!({"menu_prompt_active": true, "message": "x"})),
            "menu prompt"
        );
        assert_eq!(blocked_reason(&json!({})), "permission or question");
    }

    #[test]
    fn a_projection_without_an_agent_reads_idle_and_a_bell_keeps_its_reason() {
        let mut fleet = json!({"agents": [
            {"session": "a", "generation": 1},
            {"session": "b", "generation": 1},
        ]});
        apply_projections(
            "fleet.list",
            &mut fleet,
            &[
                json!({"session": "a", "generation": 1, "state_seq": 0, "activity": null}),
                json!({"session": "b", "generation": 1, "state_seq": 3, "attention_reason": "bell", "activity": {"state": "waiting_for_input"}}),
            ],
        );
        assert_eq!(fleet["agents"][0]["state"], "idle");
        assert_eq!(fleet["agents"][1]["state"], "waiting_for_input");
        assert_eq!(fleet["agents"][1]["attention_reason"], "bell");
    }
}
