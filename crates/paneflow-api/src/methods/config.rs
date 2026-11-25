use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{NO_FALLBACK, NoParams, WINDOW};
use crate::descriptor::{Audience, Gate, Owner, Reply};

const WORKER: Owner = Owner::Worker;

pub const CONFIG_RELOADED: &str = "config.reloaded";

pub const CONFIG_EVENT_TYPES: &[&str] = &[CONFIG_RELOADED];

pub const SLOW_REPLY_METHODS: &[&str] = &[
    "config.reload",
    "prompt.request",
    "prompt.wait",
    "action.wait",
];

pub const MAX_ACTION_WAIT_MS: u64 = 25_000;

pub const SLOW_REPLY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(45);

pub const ACTION_INVOKE: &str = "action.invoke";

pub const ACTION_REPLY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(
    2 * super::prompt::MAX_TIMEOUT_SECS + SLOW_REPLY_DEADLINE.as_secs(),
);

const GRANT_REQUEST: &str = "grant.request";

pub fn reply_deadline(method: &str, default: std::time::Duration) -> std::time::Duration {
    if method == ACTION_INVOKE {
        ACTION_REPLY_DEADLINE
    } else if super::browser::is_browser_method(method) {
        super::browser::CLIENT_REPLY_DEADLINE
    } else if method == super::prompt::AGENT_PROMPT_RESOLVE || method == GRANT_REQUEST {
        std::time::Duration::from_secs(
            super::prompt::AGENT_PROMPT_DEADLINE_SECS + SLOW_REPLY_DEADLINE.as_secs(),
        )
    } else if SLOW_REPLY_METHODS.contains(&method) {
        SLOW_REPLY_DEADLINE
    } else {
        default
    }
}

pub fn window_reply_deadline(method: &str, default: std::time::Duration) -> std::time::Duration {
    if super::browser::is_browser_method(method) {
        super::browser::REPLY_DEADLINE
    } else {
        default
    }
}

pub const ACTION_ORIGINS: &[&str] = &[
    "key", "palette", "cli", "api", "script", "agent", "timer", "event",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConfigProblem {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "The file the problem is in, such as init.lua or lua/dev.lua")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConfigStatus {
    #[schemars(description = "Absolute path of the init.lua the worker reads")]
    pub path: String,
    #[schemars(description = "Whether the file existed at the last load")]
    pub found: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Milliseconds since the Unix epoch when the active configuration loaded"
    )]
    pub loaded_at_ms: Option<u64>,
    #[schemars(
        description = "False when the last load failed and the previous configuration stayed active"
    )]
    pub ok: bool,
    pub actions: usize,
    pub bindings: usize,
    pub handlers: usize,
    pub timers: usize,
    #[schemars(description = "Errors raised by event handlers of the active configuration")]
    pub handler_errors: u64,
    #[schemars(description = "Errors raised by timers of the active configuration")]
    pub timer_errors: u64,
    pub problems: Vec<ConfigProblem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ActionSummary {
    pub name: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub category: String,
    pub keywords: Vec<String>,
    #[schemars(
        description = "JSON Schema of the arguments, an object with one property per argument"
    )]
    pub args: Value,
    #[schemars(description = "Whether agents may run the action")]
    pub agents: bool,
    #[schemars(description = "Where the action is declared, such as init.lua:12")]
    pub source: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ActionList {
    pub actions: Vec<ActionSummary>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InvokeContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "What started the run: key, palette, cli, api, script, agent, timer or event; api by default"
    )]
    pub origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "An opaque grant the window mints for a run it starts from a key or the palette; the run's writes to that window carry it in the request's grant field and pass the write gate. Never shown to Lua"
    )]
    pub grant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "For origin agent, the Paneflow session of the agent; the worker replaces it with the caller's own session when the caller runs in a pane"
    )]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "For origin agent, the agent's name; the worker keeps it only when it names a known agent"
    )]
    pub agent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InvokeParams {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "The action's arguments, checked against its declared schema")]
    pub args: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctx: Option<InvokeContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Milliseconds to wait for the run to end, at most 25000; past it the reply is {status: running, run_id} and action.wait keeps waiting. Absent waits for the end"
    )]
    pub wait_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Done,
    Running,
    RateLimited,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Invoked {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "What run returned; absent when it returned nothing, nil or true")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Set when the call waited with wait_ms: done, running (then run_id names the run), or rate_limited for an agent running more than 3 actions in a burst or more than 1 per second after it"
    )]
    pub status: Option<RunStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

impl Invoked {
    pub fn done(result: Option<Value>) -> Self {
        Self {
            result,
            status: Some(RunStatus::Done),
            run_id: None,
        }
    }

    pub fn running(run_id: String) -> Self {
        Self {
            result: None,
            status: Some(RunStatus::Running),
            run_id: Some(run_id),
        }
    }

    pub fn rate_limited() -> Self {
        Self {
            result: None,
            status: Some(RunStatus::RateLimited),
            run_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaitRunParams {
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Milliseconds to wait, 0 by default, at most 25000")]
    pub wait_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BindingTarget {
    Lua,
    Builtin,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct KeyBindingRow {
    #[schemars(description = "The key as written, in the syntax of paneflow.json shortcuts")]
    pub key: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Map<String, Value>>,
    pub source: String,
    #[schemars(
        description = "lua for an action of init.lua, builtin for a name the window resolves"
    )]
    pub target: BindingTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnboundKey {
    pub key: String,
    pub source: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Keymap {
    pub bindings: Vec<KeyBindingRow>,
    #[schemars(description = "Keys init.lua unbound, for the window to drop their defaults")]
    pub unbound: Vec<UnboundKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BuiltinAction {
    pub name: String,
    pub description: String,
    pub group: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BuiltinActions {
    pub actions: Vec<BuiltinAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DispatchParams {
    #[schemars(description = "A name from client.list_actions, such as toggle_zoom")]
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Dispatched {
    pub dispatched: bool,
    pub name: String,
}

method! {
    Status {
        name: "config.status",
        summary: "Read the state of init.lua: its path, whether it was found, when it loaded, what it declared and its load problems.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::Anyone,
        reply: Reply::Ready,
        params: NoParams,
        result: ConfigStatus,
        examples: [({} => {
            "path": "/home/me/.paneflow/init.lua",
            "found": true,
            "loaded_at_ms": 1763600000000_u64,
            "ok": false,
            "actions": 2,
            "bindings": 1,
            "handlers": 1,
            "timers": 0,
            "handler_errors": 0,
            "timer_errors": 0,
            "problems": [{"file": "init.lua", "line": 7, "severity": "error", "message": "unexpected symbol near 'end'"}]
        })],
    }
}

method! {
    Reload {
        name: "config.reload",
        summary: "Load init.lua again now and return the status; a failed load keeps the previous configuration active.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::Anyone,
        reply: Reply::Ready,
        params: NoParams,
        result: ConfigStatus,
        examples: [({} => {
            "path": "/home/me/.paneflow/init.lua",
            "found": false,
            "ok": true,
            "actions": 0,
            "bindings": 0,
            "handlers": 0,
            "timers": 0,
            "handler_errors": 0,
            "timer_errors": 0,
            "problems": []
        })],
    }
}

method! {
    ListActions {
        name: "action.list",
        summary: "List the actions init.lua declared, with their titles, argument schemas and sources.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::Anyone,
        reply: Reply::Ready,
        params: NoParams,
        result: ActionList,
        examples: [({} => {"actions": [{
            "name": "dev_layout",
            "title": "Dev layout",
            "category": "Actions",
            "keywords": ["split"],
            "args": {"type": "object", "properties": {"file": {"type": "string"}}, "required": ["file"], "additionalProperties": false},
            "agents": false,
            "source": "init.lua:3"
        }]})],
    }
}

method! {
    InvokeAction {
        name: "action.invoke",
        summary: "Run an action of init.lua with arguments and return what it returned. Arguments are checked against its schema first; a run is stopped after 30 seconds. With wait_ms, a run still going past it answers {status: running, run_id} for action.wait. With ctx.origin agent, only an action declared with agents = true runs.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::Anyone,
        reply: Reply::Ready,
        params: InvokeParams,
        result: Invoked,
        examples: [({"name": "dev_layout", "args": {"file": "app.log"}, "ctx": {"origin": "cli"}} => {"result": {"surface_id": 12}})],
    }
}

method! {
    WaitAction {
        name: "action.wait",
        summary: "Keep waiting on an action run that action.invoke returned as {status: running, run_id}: the run's result with status done, the same running status, or the action's error.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::Anyone,
        reply: Reply::Ready,
        params: WaitRunParams,
        result: Invoked,
        examples: [({"run_id": "6f1c0e9a2b7d4c3e8f5a1b2c3d4e5f60", "wait_ms": 20000} => {"result": {"surface_id": 12}, "status": "done"})],
    }
}

method! {
    ListKeymap {
        name: "keymap.list",
        summary: "List the key bindings init.lua declared and the keys it unbound, for the window to apply.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::Anyone,
        reply: Reply::Ready,
        params: NoParams,
        result: Keymap,
        examples: [({} => {
            "bindings": [{"key": "ctrl-alt-d", "action": "dev_layout", "args": {"file": "app.log"}, "source": "init.lua:9", "target": "lua"}],
            "unbound": [{"key": "ctrl-shift-w", "source": "init.lua:10"}]
        })],
    }
}

method! {
    ListBuiltinActions {
        name: "client.list_actions",
        summary: "List the window's built-in actions: the names key bindings and client.dispatch_action accept.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::Anyone,
        reply: Reply::Inline,
        params: NoParams,
        result: BuiltinActions,
        examples: [({} => {"actions": [{"name": "toggle_zoom", "description": "Zoom the focused pane", "group": "Panes & splits"}]})],
    }
}

method! {
    DispatchAction {
        name: "client.dispatch_action",
        summary: "Dispatch a built-in action in the window as the command palette does, and reply once it is dispatched.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::Anyone,
        reply: Reply::Async,
        params: DispatchParams,
        result: Dispatched,
        examples: [({"name": "toggle_zoom"} => {"dispatched": true, "name": "toggle_zoom"})],
    }
}
