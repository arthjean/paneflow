pub(super) use paneflow_agent_config::delivery::{
    ATTENTION, BLOCKED, IDLE, agent_runtime, reduced_state, reports_turns,
};
use paneflow_agent_config::delivery::{FOREGROUND_RUNTIME, reduce_row};
use paneflow_ipc_client::IpcTransport;
use paneflow_ipc_client::host_control::HostTransport;
use serde_json::Value;

const REDUCED_METHODS: [&str; 2] = ["surface.status", "fleet.list"];

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

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_agent_config::delivery::{WORKING, blocked_reason, foreground_departure};
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
