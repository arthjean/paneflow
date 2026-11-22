use std::collections::HashSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::NO_FALLBACK;
use crate::descriptor::{Audience, Gate, Owner, Reply};

const WORKER: Owner = Owner::Worker;

pub const MAX_PICK_ITEMS: usize = 2_000;
pub const DEFAULT_TIMEOUT_SECS: u64 = 300;
pub const MAX_TIMEOUT_SECS: u64 = 3_600;
pub const MAX_PENDING: usize = 16;
pub const MAX_WAIT_MS: u64 = 25_000;
pub const MAX_SPEC_BYTES: usize = 48 * 1024;
pub const ERR_NOT_PENDING: i64 = -32060;
pub const NOT_PENDING: &str = "not_pending";

pub const ICONS: &[&str] = &[
    "document", "folder", "branch", "terminal", "code", "server", "database", "package", "globe",
    "link", "person", "tag", "clock", "star", "gear", "plus",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PromptKind {
    Input,
    Pick,
}

impl PromptKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Pick => "pick",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PickItemSpec {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "The label shown and filtered on; the id when absent")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "document, folder, branch, terminal, code, server, database, package, globe, link, person, tag, clock, star, gear or plus"
    )]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Text shown at the end of the row")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Starts ticked when multiple is true")]
    pub selected: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum PickItem {
    Plain(String),
    Item(PickItemSpec),
}

impl PickItem {
    pub fn id(&self) -> &str {
        match self {
            Self::Plain(id) => id,
            Self::Item(item) => &item.id,
        }
    }

    pub fn title(&self) -> &str {
        match self {
            Self::Plain(id) => id,
            Self::Item(item) => item.title.as_deref().unwrap_or(&item.id),
        }
    }

    pub fn subtitle(&self) -> Option<&str> {
        match self {
            Self::Plain(_) => None,
            Self::Item(item) => item.subtitle.as_deref(),
        }
    }

    pub fn icon(&self) -> Option<&str> {
        match self {
            Self::Plain(_) => None,
            Self::Item(item) => item.icon.as_deref(),
        }
    }

    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::Plain(_) => None,
            Self::Item(item) => item.detail.as_deref(),
        }
    }

    pub fn selected(&self) -> bool {
        match self {
            Self::Plain(_) => false,
            Self::Item(item) => item.selected.unwrap_or(false),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PromptSpec {
    pub kind: PromptKind,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Text the field starts with")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Label of the confirming row")]
    pub confirm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Seconds before the prompt ends with timeout: 300 by default, at most 3600"
    )]
    pub timeout: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "The choices of a pick prompt, at most 2000: an id string or an item"
    )]
    pub items: Option<Vec<PickItem>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Let the user tick several items; the answer is the list of ids")]
    pub multiple: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Accept typed text that matches no item; the answer is {text}")]
    pub allow_other: Option<bool>,
}

impl PromptSpec {
    pub fn items(&self) -> &[PickItem] {
        self.items.as_deref().unwrap_or_default()
    }

    pub fn multiple(&self) -> bool {
        self.multiple.unwrap_or(false)
    }

    pub fn allow_other(&self) -> bool {
        self.allow_other.unwrap_or(false)
    }

    pub fn timeout_ms(&self) -> u64 {
        self.timeout.map_or(DEFAULT_TIMEOUT_SECS * 1000, |seconds| {
            (seconds * 1000.0).ceil().max(1.0) as u64
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OriginKind {
    Action,
    Script,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PromptOrigin {
    #[schemars(
        description = "action for an action of init.lua, script for a script or a handler, agent for a coding agent"
    )]
    pub kind: OriginKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "The action's title")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "The action's name")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "The script's file name")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "The agent's name")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "The surface the asker runs in or acts on")]
    pub surface_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "The Paneflow session of the agent that asks; the worker replaces it with the caller's own session when the caller runs in a pane"
    )]
    pub session: Option<String>,
}

impl PromptOrigin {
    pub fn label(&self) -> String {
        match self.kind {
            OriginKind::Action => {
                let action = match (&self.title, &self.name) {
                    (Some(title), _) => format!("Asked by the action {title}"),
                    (None, Some(name)) => format!("Asked by the action {name}"),
                    (None, None) => "Asked by an action".to_string(),
                };
                match &self.agent {
                    Some(agent) => format!("{action}, run by {agent}"),
                    None => action,
                }
            }
            OriginKind::Script => match &self.file {
                Some(file) => format!("Asked by the script {file}"),
                None => "Asked by a script".to_string(),
            },
            OriginKind::Agent => {
                let agent = self.agent.as_deref().unwrap_or("an agent");
                match self.surface_id {
                    Some(surface) => format!("Asked by {agent} in pane {surface}"),
                    None => format!("Asked by {agent}"),
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequestParams {
    #[schemars(description = "input asks for a line of text, pick for a choice among items")]
    pub kind: PromptKind,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Text the field starts with")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Label of the confirming row")]
    pub confirm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Seconds before the prompt ends with timeout: 300 by default, at most 3600"
    )]
    pub timeout: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "The choices of a pick prompt, at most 2000: an id string or an item"
    )]
    pub items: Option<Vec<PickItem>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Let the user tick several items; the answer is the list of ids")]
    pub multiple: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Accept typed text that matches no item; the answer is {text}")]
    pub allow_other: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Who asks, shown under the title")]
    pub origin: Option<PromptOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Milliseconds to wait for the answer before returning pending with the id, 0 by default, at most 25000"
    )]
    pub wait_ms: Option<u64>,
}

impl RequestParams {
    pub fn spec(&self) -> PromptSpec {
        PromptSpec {
            kind: self.kind,
            title: self.title.clone(),
            placeholder: self.placeholder.clone(),
            value: self.value.clone(),
            confirm: self.confirm.clone(),
            timeout: self.timeout,
            items: self.items.clone(),
            multiple: self.multiple,
            allow_other: self.allow_other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OtherAnswer {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum PromptAnswer {
    Text(String),
    Ids(Vec<String>),
    Other(OtherAnswer),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PromptStatus {
    Answered,
    Pending,
    Cancelled,
    Timeout,
    NoClient,
    Unsupported,
    Busy,
    RateLimited,
    Failed,
}

impl PromptStatus {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Answered => "answered",
            Self::Pending => "pending",
            Self::Cancelled => "cancelled",
            Self::Timeout => "timeout",
            Self::NoClient => "no_client",
            Self::Unsupported => "unsupported",
            Self::Busy => "busy",
            Self::RateLimited => "rate_limited",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PromptOutcome {
    pub status: PromptStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "The prompt's id; absent when nothing was queued")]
    pub id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "The text of an input, the id or the list of ids of a pick, or {text} for typed text a pick accepted"
    )]
    pub answer: Option<PromptAnswer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Why the prompt failed, for status failed only")]
    pub detail: Option<String>,
}

impl PromptOutcome {
    pub fn ended(status: PromptStatus, id: Option<u64>) -> Self {
        Self {
            status,
            id,
            answer: None,
            detail: None,
        }
    }

    pub fn failed(id: Option<u64>, detail: impl Into<String>) -> Self {
        Self {
            status: PromptStatus::Failed,
            id,
            answer: None,
            detail: Some(detail.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaitParams {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Milliseconds to wait, 0 by default, at most 25000")]
    pub wait_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplyParams {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "The text, the id, the list of ids or {text}, as the prompt's kind takes"
    )]
    pub answer: Option<PromptAnswer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Dismiss the prompt; the asker receives cancelled")]
    pub cancel: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    Answered,
    Cancelled,
    Timeout,
    NoClient,
    Failed,
}

impl EndReason {
    pub const fn status(self) -> PromptStatus {
        match self {
            Self::Answered => PromptStatus::Answered,
            Self::Cancelled => PromptStatus::Cancelled,
            Self::Timeout => PromptStatus::Timeout,
            Self::NoClient => PromptStatus::NoClient,
            Self::Failed => PromptStatus::Failed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Replied {
    pub id: u64,
    #[schemars(description = "answered or cancelled")]
    pub reason: EndReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FollowParams {
    #[schemars(description = "The prompt kinds this client can show")]
    pub kinds: Vec<PromptKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FollowingKind {
    Following,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FollowHeader {
    #[serde(rename = "type")]
    pub kind: FollowingKind,
    pub kinds: Vec<PromptKind>,
    #[schemars(description = "How many requested frames for already pending prompts follow")]
    pub pending: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FrameKind {
    Requested,
    Ended,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequestedFrame {
    #[serde(rename = "type")]
    pub kind: FrameKind,
    pub id: u64,
    pub spec: PromptSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<PromptOrigin>,
    pub requested_at_ms: u64,
    #[schemars(description = "Milliseconds left before the prompt ends with timeout")]
    pub remaining_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EndedFrame {
    #[serde(rename = "type")]
    pub kind: FrameKind,
    pub id: u64,
    pub reason: EndReason,
}

fn present(text: Option<&str>) -> bool {
    text.is_some_and(|text| !text.trim().is_empty())
}

pub fn check_spec(spec: &PromptSpec) -> Result<(), String> {
    if !present(Some(&spec.title)) {
        return Err("'title' must not be empty".to_string());
    }
    if let Some(timeout) = spec.timeout {
        if !timeout.is_finite() || timeout <= 0.0 {
            return Err("'timeout' must be a positive number of seconds".to_string());
        }
        if timeout > MAX_TIMEOUT_SECS as f64 {
            return Err(format!(
                "'timeout' is {timeout} seconds; a prompt waits at most {MAX_TIMEOUT_SECS} seconds"
            ));
        }
    }
    match spec.kind {
        PromptKind::Input => {
            for (field, set) in [
                ("items", spec.items.is_some()),
                ("multiple", spec.multiple.is_some()),
                ("allow_other", spec.allow_other.is_some()),
            ] {
                if set {
                    return Err(format!("'{field}' is only for a pick prompt"));
                }
            }
        }
        PromptKind::Pick => {
            if spec.multiple() && spec.allow_other() {
                return Err("'allow_other' cannot be combined with 'multiple'".to_string());
            }
            let items = spec.items();
            if items.len() > MAX_PICK_ITEMS {
                return Err(format!(
                    "'items' holds {} items; a pick prompt takes at most {MAX_PICK_ITEMS}",
                    items.len()
                ));
            }
            let mut seen = HashSet::new();
            for (index, item) in items.iter().enumerate() {
                if item.id().is_empty() {
                    return Err(format!("'items[{index}].id' must not be empty"));
                }
                if !seen.insert(item.id()) {
                    return Err(format!(
                        "'items[{index}].id' repeats the id '{}'",
                        item.id()
                    ));
                }
            }
        }
    }
    let size = serde_json::to_vec(spec).map_or(usize::MAX, |bytes| bytes.len());
    if size > MAX_SPEC_BYTES {
        return Err(format!(
            "'items' makes the prompt {size} bytes; a prompt is at most {MAX_SPEC_BYTES} bytes"
        ));
    }
    Ok(())
}

pub fn normalize_answer(spec: &PromptSpec, answer: PromptAnswer) -> Result<PromptAnswer, String> {
    let items = spec.items();
    let known = |id: &str| items.iter().any(|item| item.id() == id);
    match (spec.kind, answer) {
        (PromptKind::Input, PromptAnswer::Text(text)) => Ok(PromptAnswer::Text(text)),
        (PromptKind::Input, _) => Err("'answer' to an input prompt is a string".to_string()),
        (PromptKind::Pick, PromptAnswer::Ids(ids)) if spec.multiple() => {
            if let Some(unknown) = ids.iter().find(|id| !known(id)) {
                return Err(format!(
                    "'answer' names '{unknown}', which is not an item id"
                ));
            }
            Ok(PromptAnswer::Ids(
                items
                    .iter()
                    .map(PickItem::id)
                    .filter(|id| ids.iter().any(|chosen| chosen == id))
                    .map(str::to_string)
                    .collect(),
            ))
        }
        (PromptKind::Pick, _) if spec.multiple() => {
            Err("'answer' to a pick with multiple is a list of item ids".to_string())
        }
        (PromptKind::Pick, PromptAnswer::Text(id)) if known(&id) => Ok(PromptAnswer::Text(id)),
        (PromptKind::Pick, PromptAnswer::Text(id)) => {
            Err(format!("'answer' names '{id}', which is not an item id"))
        }
        (PromptKind::Pick, PromptAnswer::Other(other)) if spec.allow_other() => {
            Ok(PromptAnswer::Other(other))
        }
        (PromptKind::Pick, PromptAnswer::Other(_)) => {
            Err("'answer' {text} needs allow_other on the prompt".to_string())
        }
        (PromptKind::Pick, PromptAnswer::Ids(_)) => {
            Err("'answer' to a pick without multiple is one item id".to_string())
        }
    }
}

method! {
    Request {
        name: "prompt.request",
        summary: "Ask the user through every attached client that can show the prompt; the first answer wins. Returns {status: answered, answer} within wait_ms, {status: pending, id} when wait_ms passes first, or the reason it ended: cancelled, timeout, no_client, unsupported, busy, rate_limited (an agent asking more than 3 times in a burst or more than once a second after it) or failed with a detail.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::Anyone,
        reply: Reply::Ready,
        params: RequestParams,
        result: PromptOutcome,
        examples: [
            ({"kind": "input", "title": "New branch", "value": "feature/", "wait_ms": 20000, "origin": {"kind": "action", "name": "new_branch", "title": "New branch"}} => {"status": "answered", "id": 3, "answer": "feature/login"}),
            ({"kind": "pick", "title": "Project", "items": ["api", {"id": "web", "subtitle": "Next.js", "icon": "globe"}]} => {"status": "pending", "id": 4}),
        ],
    }
}

method! {
    Wait {
        name: "prompt.wait",
        summary: "Keep waiting on a pending prompt by id, with the same outcomes as prompt.request.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::Anyone,
        reply: Reply::Ready,
        params: WaitParams,
        result: PromptOutcome,
        examples: [({"id": 4, "wait_ms": 20000} => {"status": "answered", "id": 4, "answer": "web"})],
    }
}

method! {
    ReplyTo {
        name: "prompt.reply",
        summary: "Answer or cancel a pending prompt. The first reply wins and every follower receives ended; a later one is refused with -32060 and data {reason: not_pending}.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::Anyone,
        reply: Reply::Ready,
        params: ReplyParams,
        result: Replied,
        examples: [
            ({"id": 3, "answer": "feature/login"} => {"id": 3, "reason": "answered"}),
            ({"id": 4, "cancel": true} => {"id": 4, "reason": "cancelled"}),
        ],
    }
}

method! {
    Follow {
        name: "prompt.follow",
        summary: "Stream the prompts this client can show: a following header, a requested frame for each pending prompt oldest first, then requested frames (id, spec, origin, remaining time) and ended frames (id, reason), keepalive frames when quiet.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::Anyone,
        reply: Reply::Stream,
        params: FollowParams,
        result: FollowHeader,
        examples: [({"kinds": ["input", "pick"]} => {"type": "following", "kinds": ["input", "pick"], "pending": 1})],
    }
}

pub const AGENT_PROMPT_RESOLVE: &str = "agent_prompt.resolve";
pub const AGENT_PROMPT_DEADLINE_SECS: u64 = DEFAULT_TIMEOUT_SECS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentPromptEvent {
    AskUserQuestion,
    PermissionRequest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResolveParams {
    pub event: AgentPromptEvent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "The agent's runtime name, such as claude or codex")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "The Paneflow session of the agent; the worker replaces it with the caller's own session when the caller runs in a pane"
    )]
    pub session: Option<String>,
    #[schemars(description = "The hook's input JSON, as the agent wrote it")]
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResolveStatus {
    Answered,
    Declined,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Resolved {
    pub status: ResolveStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "For answered, the exact JSON the hook prints for the agent")]
    pub output: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "For declined: native, terminal, cancelled, timeout, no_window, unsupported or failed"
    )]
    pub reason: Option<String>,
}

impl Resolved {
    pub fn answered(output: serde_json::Value) -> Self {
        Self {
            status: ResolveStatus::Answered,
            output: Some(output),
            reason: None,
        }
    }

    pub fn declined(reason: &str) -> Self {
        Self {
            status: ResolveStatus::Declined,
            output: None,
            reason: Some(reason.to_string()),
        }
    }
}

method! {
    ResolveAgentPrompt {
        name: "agent_prompt.resolve",
        summary: "Ask the user, in the window, the question or the permission request a coding agent's hook received, when agent_prompts is paneflow in paneflow.json. Holds one 300 second deadline for the whole call and replies answered with the JSON the hook prints, or declined, after which the agent asks in its own terminal.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::Anyone,
        reply: Reply::Ready,
        params: ResolveParams,
        result: Resolved,
        examples: [({"event": "permission_request", "agent": "claude", "payload": {"tool_name": "Bash", "tool_input": {"command": "rm -rf build"}}} => {"status": "answered", "output": {"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": {"behavior": "allow"}}}})],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick(items: Vec<PickItem>) -> PromptSpec {
        PromptSpec {
            kind: PromptKind::Pick,
            title: "Project".into(),
            placeholder: None,
            value: None,
            confirm: None,
            timeout: None,
            items: Some(items),
            multiple: None,
            allow_other: None,
        }
    }

    fn plain(ids: &[&str]) -> Vec<PickItem> {
        ids.iter().map(|id| PickItem::Plain((*id).into())).collect()
    }

    #[test]
    fn a_pick_refuses_too_many_items_a_repeated_id_and_other_with_multiple_naming_the_field() {
        let many: Vec<String> = (0..=MAX_PICK_ITEMS)
            .map(|index| index.to_string())
            .collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert!(
            check_spec(&pick(plain(&many)))
                .unwrap_err()
                .starts_with("'items'")
        );
        assert_eq!(
            check_spec(&pick(plain(&["a", "b", "a"]))).unwrap_err(),
            "'items[2].id' repeats the id 'a'"
        );
        let mut both = pick(plain(&["a"]));
        both.multiple = Some(true);
        both.allow_other = Some(true);
        assert!(check_spec(&both).unwrap_err().starts_with("'allow_other'"));
        let mut long = pick(plain(&["a"]));
        long.timeout = Some(3_601.0);
        assert!(check_spec(&long).unwrap_err().starts_with("'timeout'"));
        let mut blank = pick(plain(&["a"]));
        blank.title = " ".into();
        assert!(check_spec(&blank).unwrap_err().starts_with("'title'"));
        assert_eq!(check_spec(&pick(plain(&["a", "b"]))), Ok(()));
    }

    #[test]
    fn a_multiple_answer_comes_back_in_the_order_of_the_items() {
        let mut spec = pick(plain(&["a", "b", "c"]));
        spec.multiple = Some(true);
        assert_eq!(
            normalize_answer(&spec, PromptAnswer::Ids(vec!["c".into(), "a".into()])),
            Ok(PromptAnswer::Ids(vec!["a".into(), "c".into()]))
        );
        assert_eq!(
            normalize_answer(&spec, PromptAnswer::Ids(Vec::new())),
            Ok(PromptAnswer::Ids(Vec::new()))
        );
        assert!(normalize_answer(&spec, PromptAnswer::Text("a".into())).is_err());
    }

    #[test]
    fn a_single_pick_takes_a_known_id_or_typed_text_when_allowed() {
        let mut spec = pick(plain(&["a"]));
        assert!(normalize_answer(&spec, PromptAnswer::Text("z".into())).is_err());
        let other = PromptAnswer::Other(OtherAnswer { text: "z".into() });
        assert!(normalize_answer(&spec, other.clone()).is_err());
        spec.allow_other = Some(true);
        assert_eq!(normalize_answer(&spec, other.clone()), Ok(other));
    }

    #[test]
    fn a_timeout_below_one_millisecond_still_leaves_the_prompt_open_for_one() {
        let mut spec = pick(plain(&["a"]));
        assert_eq!(spec.timeout_ms(), DEFAULT_TIMEOUT_SECS * 1000);
        spec.timeout = Some(0.0004);
        assert_eq!(check_spec(&spec), Ok(()));
        assert_eq!(spec.timeout_ms(), 1);
        spec.timeout = Some(1.5);
        assert_eq!(spec.timeout_ms(), 1_500);
        spec.timeout = Some(MAX_TIMEOUT_SECS as f64);
        assert_eq!(spec.timeout_ms(), MAX_TIMEOUT_SECS * 1000);
    }

    #[test]
    fn a_plain_string_item_is_its_own_id_and_title() {
        let item: PickItem = serde_json::from_str("\"main\"").unwrap();
        assert_eq!((item.id(), item.title()), ("main", "main"));
        let item: PickItem = serde_json::from_str("{\"id\": \"w\", \"title\": \"Web\"}").unwrap();
        assert_eq!((item.id(), item.title()), ("w", "Web"));
    }
}
