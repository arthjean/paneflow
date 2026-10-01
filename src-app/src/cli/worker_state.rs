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
