use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::runtime_catalog::{runtime_by_id, runtime_for_tool, Runtime};

pub const WORKING: &str = "working";
pub const ATTENTION: &str = "attention";
pub const BLOCKED: &str = "blocked";
pub const IDLE: &str = "idle";

pub const FOREGROUND_RUNTIME: &str = "foreground_runtime";

pub const DECLARED_SOURCE: &str = "declared";

pub const SUBMIT_START_TIMEOUT: Duration = Duration::from_secs(5);

pub const SUBMIT_START_POLL: Duration = Duration::from_millis(60);

const PROJECTED_FACTS: [&str; 2] = ["outcome", "activity_source"];

const MAX_BLOCKED_REASON_CHARS: usize = 120;

pub fn reduced_state(status: &Value) -> Option<&'static str> {
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
    status
        .get("outcome")
        .and_then(Value::as_str)
        .is_some_and(|outcome| outcome.starts_with("failed"))
}

pub fn blocked_reason(status: &Value) -> String {
    status
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(|message| message.chars().take(MAX_BLOCKED_REASON_CHARS).collect())
        .unwrap_or_else(|| "permission or question".to_string())
}

pub fn agent_runtime(status: &Value) -> Option<&'static Runtime> {
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

pub fn reports_turns(status: &Value) -> bool {
    status.get("activity_source").and_then(Value::as_str) == Some(DECLARED_SOURCE)
}

pub fn foreground_departure(status: &Value, runtime: &Runtime) -> Option<String> {
    match status.get(FOREGROUND_RUNTIME)? {
        Value::String(id) if id == runtime.id => None,
        Value::String(id) => {
            Some(runtime_by_id(id).map_or_else(|| id.clone(), |held| held.label.to_string()))
        }
        _ => Some("a shell or another program".to_string()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryRefusal {
    Blocked {
        reason: String,
    },
    LeftForeground {
        runtime: &'static str,
        holder: String,
    },
}

pub fn delivery_refusal(status: &Value) -> Option<DeliveryRefusal> {
    if reduced_state(status) == Some(BLOCKED) {
        return Some(DeliveryRefusal::Blocked {
            reason: blocked_reason(status),
        });
    }
    let runtime = agent_runtime(status)?;
    let holder = foreground_departure(status, runtime)?;
    Some(DeliveryRefusal::LeftForeground {
        runtime: runtime.label,
        holder,
    })
}

pub fn reduce_row(row: &mut Value, projections: &[Value]) {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnStart {
    pub started: Option<bool>,
    pub reason: &'static str,
    pub state: Option<&'static str>,
}

impl TurnStart {
    pub fn annotate(&self, result: &mut Value) {
        result["delivered"] = json!(true);
        result["started"] = json!(self.started);
        result["reason"] = json!(self.reason);
        if let Some(state) = self.state {
            result["state"] = json!(state);
        }
    }
}

pub fn confirm_turn_start(
    mut status_now: impl FnMut() -> Option<Value>,
    before: Option<&Value>,
    timeout: Duration,
    poll: Duration,
) -> TurnStart {
    let baseline = before
        .filter(|status| reports_turns(status))
        .and_then(|status| status.get("state_seq").and_then(Value::as_u64));
    let Some(baseline) = baseline else {
        return TurnStart {
            started: None,
            reason: "no_signal",
            state: status_now().as_ref().and_then(reduced_state),
        };
    };
    let deadline = Instant::now() + timeout;
    let mut last_state = before.and_then(reduced_state);
    loop {
        if let Some(status) = status_now() {
            let state = reduced_state(&status);
            last_state = state.or(last_state);
            let advanced = status
                .get("state_seq")
                .and_then(Value::as_u64)
                .is_some_and(|seq| seq > baseline);
            if advanced && state.is_some() {
                return TurnStart {
                    started: Some(true),
                    reason: "state_transition",
                    state,
                };
            }
        }
        if Instant::now() >= deadline {
            return TurnStart {
                started: Some(false),
                reason: "no_state_transition",
                state: last_state,
            };
        }
        std::thread::sleep(poll);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE: &str = "com.anthropic.claude-code";

    #[test]
    fn a_blocked_agent_and_a_departed_agent_are_both_refused() {
        let blocked = json!({"state": "waiting_for_input", "message": "Allow Bash?"});
        assert_eq!(
            delivery_refusal(&blocked),
            Some(DeliveryRefusal::Blocked {
                reason: "Allow Bash?".to_string()
            })
        );
        let departed =
            json!({"state": "idle", "agent_runtime": CLAUDE, "foreground_runtime": null});
        assert!(matches!(
            delivery_refusal(&departed),
            Some(DeliveryRefusal::LeftForeground { .. })
        ));
        let present =
            json!({"state": "idle", "agent_runtime": CLAUDE, "foreground_runtime": CLAUDE});
        assert_eq!(delivery_refusal(&present), None);
        assert_eq!(delivery_refusal(&json!({})), None);
    }

    #[test]
    fn a_turn_starts_only_on_a_later_sequence() {
        let before = json!({"activity_source": DECLARED_SOURCE, "state": "idle", "state_seq": 4});
        let mut answers = vec![
            json!({"activity_source": DECLARED_SOURCE, "state": "idle", "state_seq": 4}),
            json!({"activity_source": DECLARED_SOURCE, "state": "waiting_for_input", "state_seq": 5}),
        ]
        .into_iter();
        let start = confirm_turn_start(
            || answers.next(),
            Some(&before),
            Duration::from_secs(1),
            Duration::from_millis(1),
        );
        assert_eq!(start.started, Some(true));
        assert_eq!(start.state, Some(BLOCKED));

        let still = confirm_turn_start(
            || Some(before.clone()),
            Some(&before),
            Duration::from_millis(5),
            Duration::from_millis(1),
        );
        assert_eq!(still.started, Some(false));
        assert_eq!(still.reason, "no_state_transition");

        let silent = confirm_turn_start(|| None, None, Duration::ZERO, Duration::ZERO);
        assert_eq!(silent.started, None);
        assert_eq!(silent.reason, "no_signal");

        let undeclared = json!({"agent_runtime": CLAUDE, "state": "idle", "state_seq": 4});
        let unreported = confirm_turn_start(
            || Some(undeclared.clone()),
            Some(&undeclared),
            Duration::ZERO,
            Duration::ZERO,
        );
        assert_eq!(unreported.reason, "no_signal");
    }
}
