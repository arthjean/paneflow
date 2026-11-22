use std::time::{Duration, Instant};

use paneflow_api::Principal;
use paneflow_api::methods::prompt::{
    AGENT_PROMPT_DEADLINE_SECS, AgentPromptEvent, MAX_BODY_BYTES, MAX_WAIT_MS, OriginKind,
    PickItem, PickItemSpec, PromptAnswer, PromptKind, PromptOrigin, PromptOutcome, PromptStatus,
    RequestParams, ResolveParams, Resolved,
};
use serde_json::{Map, Value, json};

use crate::layout_authority::ConnectionId;

pub const ANSWER_IN_TERMINAL: &str = "Answer in the terminal";
pub const DECIDE_IN_TERMINAL: &str = "Decide in the terminal";
pub const ALLOW_ONCE: &str = "Allow once";
pub const DENY: &str = "Deny";
pub const DENIED_MESSAGE: &str = "The user denied this in Paneflow";
const TERMINAL_ID: &str = "terminal";
const ASK_USER_QUESTION: &str = "AskUserQuestion";

#[derive(Debug, Clone, PartialEq)]
pub struct Question {
    pub text: String,
    pub labels: Vec<String>,
    pub descriptions: Vec<Option<String>>,
    pub multiple: bool,
}

pub fn questions_of(payload: &Value) -> Option<Vec<Question>> {
    let questions = payload.get("tool_input")?.get("questions")?.as_array()?;
    if questions.is_empty() {
        return None;
    }
    questions
        .iter()
        .map(|question| {
            let text = question.get("question")?.as_str()?.to_string();
            let options = question.get("options")?.as_array()?;
            let mut labels = Vec::with_capacity(options.len());
            let mut descriptions = Vec::with_capacity(options.len());
            for option in options {
                labels.push(option.get("label")?.as_str()?.to_string());
                descriptions.push(
                    option
                        .get("description")
                        .and_then(Value::as_str)
                        .filter(|text| !text.trim().is_empty())
                        .map(str::to_string),
                );
            }
            Some(Question {
                text,
                labels,
                descriptions,
                multiple: question
                    .get("multiSelect")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .collect()
}

fn item(id: String, title: &str, subtitle: Option<String>) -> PickItem {
    PickItem::Item(PickItemSpec {
        id,
        title: Some(title.to_string()),
        subtitle,
        ..PickItemSpec::default()
    })
}

pub fn question_items(question: &Question) -> Vec<PickItem> {
    let mut items: Vec<PickItem> = question
        .labels
        .iter()
        .zip(&question.descriptions)
        .enumerate()
        .map(|(index, (label, description))| item(index.to_string(), label, description.clone()))
        .collect();
    items.push(item(TERMINAL_ID.to_string(), ANSWER_IN_TERMINAL, None));
    items
}

pub fn chosen_label(question: &Question, answer: &PromptAnswer) -> Option<String> {
    let label_of = |id: &str| -> Option<String> {
        if id == TERMINAL_ID {
            return None;
        }
        question.labels.get(id.parse::<usize>().ok()?).cloned()
    };
    match answer {
        PromptAnswer::Text(id) => label_of(id),
        PromptAnswer::Ids(ids) if !ids.is_empty() => ids
            .iter()
            .map(|id| label_of(id))
            .collect::<Option<Vec<_>>>()
            .map(|labels| labels.join(", ")),
        PromptAnswer::Ids(_) => None,
        PromptAnswer::Other(other) => {
            Some(other.text.trim().to_string()).filter(|text| !text.is_empty())
        }
    }
}

pub fn question_output(payload: &Value, answers: Map<String, Value>) -> Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "updatedInput": {
                "questions": payload["tool_input"]["questions"].clone(),
                "answers": answers,
            }
        }
    })
}

pub fn permission_output(allow: bool) -> Value {
    let decision = if allow {
        json!({"behavior": "allow"})
    } else {
        json!({"behavior": "deny", "message": DENIED_MESSAGE})
    };
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PermissionRequest",
            "decision": decision,
        }
    })
}

pub const TOO_LONG_TO_SHOW: &str = "The input is too long to show here, so Paneflow cannot allow it. Deny it, or decide in the terminal, where the agent shows it in full.";

fn field_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    }
}

pub fn input_text(tool_input: Option<&Value>) -> String {
    match tool_input {
        Some(Value::Object(fields)) => fields
            .iter()
            .map(|(name, value)| {
                let text = field_text(value);
                if text.contains('\n') {
                    format!("{name}:\n{text}")
                } else {
                    format!("{name}: {text}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::Null) | None => String::new(),
        Some(other) => field_text(other),
    }
}

pub fn shows_in_full(tool_input: Option<&Value>, text: &str) -> bool {
    let raw = tool_input.map_or(0, |input| input.to_string().len());
    raw <= MAX_BODY_BYTES && text.len() <= MAX_BODY_BYTES
}

fn is_codex(agent: Option<&str>) -> bool {
    agent.is_some_and(|agent| {
        crate::agent_asks::known_agent_label(agent) == crate::agent_asks::known_agent_label("codex")
    })
}

pub fn permission_choices(mut request: RequestParams, tool_input: Option<&Value>) -> RequestParams {
    let text = input_text(tool_input);
    let deny_or_terminal = [
        item("deny".into(), DENY, None),
        item(TERMINAL_ID.into(), DECIDE_IN_TERMINAL, None),
    ];
    if shows_in_full(tool_input, &text) {
        request.body = (!text.trim().is_empty()).then_some(text);
        let mut items = vec![item("allow".into(), ALLOW_ONCE, None)];
        items.extend(deny_or_terminal);
        request.items = Some(items);
    } else {
        request.body = Some(TOO_LONG_TO_SHOW.to_string());
        request.items = Some(deny_or_terminal.to_vec());
    }
    request
}

struct Deadline(Instant);

impl Deadline {
    fn left(&self) -> Duration {
        self.0.saturating_duration_since(Instant::now())
    }
}

enum Asked {
    Answer(PromptAnswer),
    Declined(&'static str),
}

fn declined_by(status: PromptStatus) -> &'static str {
    match status {
        PromptStatus::Cancelled => "cancelled",
        PromptStatus::Timeout => "timeout",
        PromptStatus::NoClient | PromptStatus::Unsupported => "no_window",
        PromptStatus::Busy | PromptStatus::RateLimited => "busy",
        _ => "failed",
    }
}

impl crate::server::Worker {
    fn agent_prompts_in_paneflow(&self) -> bool {
        paneflow_config::loader::load_config_from_path(&paneflow_home::config_path_in(&self.home))
            .agent_prompts_in_paneflow()
    }

    fn ask_until(&self, mut params: RequestParams, deadline: &Deadline) -> Asked {
        let left = deadline.left();
        if left.is_zero() {
            return Asked::Declined("timeout");
        }
        params.timeout = Some(left.as_secs_f64().max(1.0));
        params.wait_ms = Some(MAX_WAIT_MS.min(left.as_millis() as u64));
        let mut outcome = match self.prompts.request(&params) {
            Ok(outcome) => outcome,
            Err(_) => return Asked::Declined("failed"),
        };
        loop {
            match outcome {
                PromptOutcome {
                    status: PromptStatus::Answered,
                    answer: Some(answer),
                    ..
                } => return Asked::Answer(answer),
                PromptOutcome {
                    status: PromptStatus::Pending,
                    id: Some(id),
                    ..
                } => {
                    let left = deadline.left();
                    if left.is_zero() {
                        self.prompts.withdraw(id);
                        return Asked::Declined("timeout");
                    }
                    outcome = match self
                        .prompts
                        .wait(id, Some(MAX_WAIT_MS.min(left.as_millis() as u64)))
                    {
                        Ok(outcome) => outcome,
                        Err(_) => return Asked::Declined("failed"),
                    };
                }
                other => return Asked::Declined(declined_by(other.status)),
            }
        }
    }

    fn agent_origin(
        &self,
        params: &ResolveParams,
        principal: &Principal,
        connection: ConnectionId,
    ) -> Option<PromptOrigin> {
        self.attribute_prompt(
            Some(PromptOrigin {
                kind: OriginKind::Agent,
                title: None,
                name: None,
                file: None,
                agent: params.agent.clone(),
                surface_id: None,
                workspace_id: None,
                session: params.session.clone(),
            }),
            principal,
            connection,
        )
    }

    pub fn resolve_agent_prompt(
        &self,
        params: ResolveParams,
        principal: &Principal,
        connection: ConnectionId,
    ) -> Resolved {
        self.resolve_agent_prompt_within(
            params,
            principal,
            connection,
            Duration::from_secs(AGENT_PROMPT_DEADLINE_SECS),
        )
    }

    pub(crate) fn resolve_agent_prompt_within(
        &self,
        params: ResolveParams,
        principal: &Principal,
        connection: ConnectionId,
        budget: Duration,
    ) -> Resolved {
        if !self.agent_prompts_in_paneflow() {
            return Resolved::declined("native");
        }
        let deadline = Deadline(Instant::now() + budget);
        let origin = self.agent_origin(&params, principal, connection);
        match params.event {
            AgentPromptEvent::AskUserQuestion => self.resolve_questions(&params, origin, &deadline),
            AgentPromptEvent::PermissionRequest => {
                self.resolve_permission(&params, origin, &deadline)
            }
        }
    }

    fn resolve_questions(
        &self,
        params: &ResolveParams,
        origin: Option<PromptOrigin>,
        deadline: &Deadline,
    ) -> Resolved {
        if is_codex(params.agent.as_deref()) {
            return Resolved::declined("unsupported");
        }
        let Some(questions) = questions_of(&params.payload) else {
            return Resolved::declined("unsupported");
        };
        let mut answers = Map::new();
        for question in &questions {
            let request = RequestParams {
                kind: PromptKind::Pick,
                title: question.text.clone(),
                body: None,
                placeholder: None,
                value: None,
                confirm: None,
                timeout: None,
                items: Some(question_items(question)),
                multiple: question.multiple.then_some(true),
                allow_other: (!question.multiple).then_some(true),
                origin: origin.clone(),
                wait_ms: None,
            };
            match self.ask_until(request, deadline) {
                Asked::Answer(answer) => match chosen_label(question, &answer) {
                    Some(label) => {
                        answers.insert(question.text.clone(), Value::String(label));
                    }
                    None => return Resolved::declined("terminal"),
                },
                Asked::Declined(reason) => return Resolved::declined(reason),
            }
        }
        Resolved::answered(question_output(&params.payload, answers))
    }

    fn resolve_permission(
        &self,
        params: &ResolveParams,
        origin: Option<PromptOrigin>,
        deadline: &Deadline,
    ) -> Resolved {
        let Some(tool) = params
            .payload
            .get("tool_name")
            .and_then(Value::as_str)
            .filter(|tool| !tool.trim().is_empty())
        else {
            return Resolved::declined("unsupported");
        };
        if tool == ASK_USER_QUESTION {
            return Resolved::declined("unsupported");
        }
        let request = RequestParams {
            kind: PromptKind::Pick,
            title: tool.to_string(),
            body: None,
            placeholder: None,
            value: None,
            confirm: None,
            timeout: None,
            items: None,
            multiple: None,
            allow_other: None,
            origin,
            wait_ms: None,
        };
        let request = permission_choices(request, params.payload.get("tool_input"));
        let may_allow = request
            .items
            .as_deref()
            .is_some_and(|items| items.iter().any(|item| item.id() == "allow"));
        match self.ask_until(request, deadline) {
            Asked::Answer(PromptAnswer::Text(choice)) if choice == "allow" && may_allow => {
                Resolved::answered(permission_output(true))
            }
            Asked::Answer(PromptAnswer::Text(choice)) if choice == "deny" => {
                Resolved::answered(permission_output(false))
            }
            Asked::Answer(_) => Resolved::declined("terminal"),
            Asked::Declined(reason) => Resolved::declined(reason),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_api::methods::prompt::OtherAnswer;

    fn ask_payload() -> Value {
        json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "AskUserQuestion",
            "tool_input": {"questions": [
                {"question": "Which color?", "header": "Color", "multiSelect": false,
                 "options": [{"label": "Red", "description": "Warm"}, {"label": "Blue", "description": ""}]},
                {"question": "Which sizes?", "header": "Size", "multiSelect": true,
                 "options": [{"label": "S"}, {"label": "M"}, {"label": "L"}]}
            ]}
        })
    }

    #[test]
    fn each_question_becomes_a_pick_of_its_options_and_a_way_back_to_the_terminal() {
        let questions = questions_of(&ask_payload()).expect("questions");
        assert_eq!(questions.len(), 2);
        let items = question_items(&questions[0]);
        let shown: Vec<(&str, &str, Option<&str>)> = items
            .iter()
            .map(|item| (item.id(), item.title(), item.subtitle()))
            .collect();
        assert_eq!(
            shown,
            [
                ("0", "Red", Some("Warm")),
                ("1", "Blue", None),
                ("terminal", ANSWER_IN_TERMINAL, None)
            ]
        );
        assert!(questions[1].multiple);
        assert!(questions_of(&json!({"tool_input": {"questions": []}})).is_none());
        assert!(questions_of(&json!({"tool_input": {}})).is_none());
    }

    #[test]
    fn an_answer_maps_to_its_label_and_a_multiple_choice_joins_them_with_commas() {
        let questions = questions_of(&ask_payload()).expect("questions");
        assert_eq!(
            chosen_label(&questions[0], &PromptAnswer::Text("1".into())).as_deref(),
            Some("Blue")
        );
        assert_eq!(
            chosen_label(
                &questions[1],
                &PromptAnswer::Ids(vec!["0".into(), "2".into()])
            )
            .as_deref(),
            Some("S, L")
        );
        assert_eq!(
            chosen_label(
                &questions[0],
                &PromptAnswer::Other(OtherAnswer {
                    text: " Green ".into()
                })
            )
            .as_deref(),
            Some("Green")
        );
        assert_eq!(
            chosen_label(&questions[0], &PromptAnswer::Text("terminal".into())),
            None
        );
        assert_eq!(
            chosen_label(
                &questions[1],
                &PromptAnswer::Ids(vec!["0".into(), "terminal".into()])
            ),
            None
        );
        assert_eq!(
            chosen_label(&questions[1], &PromptAnswer::Ids(vec![])),
            None
        );
        assert_eq!(
            chosen_label(&questions[0], &PromptAnswer::Text("9".into())),
            None
        );
    }

    #[test]
    fn the_outputs_are_the_documented_hook_json_and_never_carry_a_rule() {
        let mut answers = Map::new();
        answers.insert("Which color?".into(), json!("Blue"));
        let output = question_output(&ask_payload(), answers);
        assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "allow");
        assert_eq!(
            output["hookSpecificOutput"]["updatedInput"]["questions"],
            ask_payload()["tool_input"]["questions"]
        );
        assert_eq!(
            output["hookSpecificOutput"]["updatedInput"]["answers"],
            json!({"Which color?": "Blue"})
        );
        assert_eq!(
            permission_output(true),
            json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": {"behavior": "allow"}}})
        );
        let denied = permission_output(false);
        assert_eq!(denied["hookSpecificOutput"]["decision"]["behavior"], "deny");
        for output in [permission_output(true), denied] {
            let decision = &output["hookSpecificOutput"]["decision"];
            assert!(decision.get("updatedPermissions").is_none());
            assert!(decision.get("updatedInput").is_none());
            assert!(output["hookSpecificOutput"].get("updatedInput").is_none());
        }
    }

    fn permission_request() -> RequestParams {
        RequestParams {
            kind: PromptKind::Pick,
            title: "Bash".into(),
            body: None,
            placeholder: None,
            value: None,
            confirm: None,
            timeout: None,
            items: None,
            multiple: None,
            allow_other: None,
            origin: None,
            wait_ms: None,
        }
    }

    fn ids(request: &RequestParams) -> Vec<&str> {
        request
            .items
            .as_deref()
            .unwrap()
            .iter()
            .map(PickItem::id)
            .collect()
    }

    #[test]
    fn a_permission_shows_every_field_of_the_input_by_name_with_its_line_breaks() {
        let input = json!({
            "command": "git status\nrm -rf ~/work",
            "description": "Check the tree",
            "timeout": 60000,
            "file_path": "/a/b.rs"
        });
        let request = permission_choices(permission_request(), Some(&input));
        let body = request.body.clone().unwrap();
        assert_eq!(
            body,
            "command:\ngit status\nrm -rf ~/work\ndescription: Check the tree\ntimeout: 60000\nfile_path: /a/b.rs"
        );
        assert_eq!(ids(&request), ["allow", "deny", "terminal"]);
        assert_eq!(request.title, "Bash", "the title stays the tool name");
        let nested = input_text(Some(&json!({"edits": [{"old": "a"}]})));
        assert!(nested.starts_with("edits:\n["), "{nested}");
        assert_eq!(input_text(Some(&json!("plain"))), "plain");
        assert_eq!(input_text(None), "");
    }

    #[test]
    fn an_input_too_long_to_show_in_full_can_only_be_denied_or_decided_in_the_terminal() {
        let long = json!({"command": "x".repeat(MAX_BODY_BYTES)});
        let request = permission_choices(permission_request(), Some(&long));
        assert_eq!(ids(&request), ["deny", "terminal"]);
        assert_eq!(request.body.as_deref(), Some(TOO_LONG_TO_SHOW));
        let fits = json!({"command": "x".repeat(MAX_BODY_BYTES - 32)});
        assert_eq!(
            ids(&permission_choices(permission_request(), Some(&fits))),
            ["allow", "deny", "terminal"]
        );
        let request = permission_choices(permission_request(), None);
        assert_eq!(request.body, None);
        assert_eq!(ids(&request), ["allow", "deny", "terminal"]);
    }
}
