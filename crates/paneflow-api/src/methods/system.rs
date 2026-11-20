use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{HOST_FALLBACK, NO_FALLBACK, NoParams, WINDOW};
use crate::descriptor::{Gate, Reply};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Pong {
    pub pong: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub name: String,
    pub version: String,
    pub protocol: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapabilitySet {
    #[schemars(description = "True when surface.send_text and surface.send_keystroke are open")]
    pub scripting: bool,
    #[schemars(description = "True when requests may set a command, a prompt or an env")]
    pub orchestration: bool,
    pub methods: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitSpawns {
    pub total: u64,
    pub probe: u64,
    pub user_action: u64,
    pub by_subcommand: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProcessIdentity {
    pub pid: u64,
    pub started_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CounterSet {
    pub root_renders: u64,
    pub sidebar_renders: u64,
    pub host_agent_snapshots_applied: u64,
    pub session_list_calls: u64,
    pub git_spawns: GitSpawns,
    pub process_spawns: u64,
    pub process_identity: ProcessIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkCounters {
    pub counters: CounterSet,
}

method! {
    Ping {
        name: "system.ping",
        summary: "Check that the endpoint answers.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Inline,
        capabilities: true,
        params: NoParams,
        result: Pong,
        examples: [({} => {"pong": true})],
    }
}

method! {
    Identify {
        name: "system.identify",
        summary: "Name the running app, its version and its wire protocol.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Inline,
        capabilities: true,
        params: NoParams,
        result: Identity,
        examples: [({} => {"name": "PaneFlow", "version": "0.20.0", "protocol": "jsonrpc-2.0"})],
    }
}

method! {
    Counters {
        name: "system.counters",
        summary: "Read the app's work counters: renders, git spawns, process spawns.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Inline,
        capabilities: true,
        params: NoParams,
        result: WorkCounters,
        examples: [({} => {"counters": {
            "root_renders": 120,
            "sidebar_renders": 40,
            "host_agent_snapshots_applied": 3,
            "session_list_calls": 2,
            "git_spawns": {"total": 5, "probe": 4, "user_action": 1, "by_subcommand": {"status": 4, "other": 1}},
            "process_spawns": 7,
            "process_identity": {"pid": 4242, "started_at": 1763600000000_u64}
        }})],
    }
}

method! {
    Capabilities {
        name: "system.capabilities",
        summary: "List the methods this endpoint serves and whether its gates are open.",
        owner: WINDOW,
        fallback: HOST_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Ready,
        capabilities: true,
        params: NoParams,
        result: CapabilitySet,
        examples: [({} => {"scripting": false, "orchestration": false, "methods": ["system.ping", "surface.list"]})],
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DescribeParams {
    #[schemars(description = "Describe only this method")]
    pub method: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExampleDescription {
    pub params: serde_json::Value,
    pub result: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MethodDescription {
    pub name: String,
    pub summary: String,
    #[schemars(description = "The process that serves the method: window, worker or host")]
    pub owner: String,
    #[schemars(description = "The process that answers when the owner is not running")]
    pub fallback: Option<String>,
    #[schemars(description = "none, scripting or orchestration")]
    pub gate: String,
    pub gate_note: Option<String>,
    #[schemars(description = "inline, ready, deferred, async or stream")]
    pub reply: String,
    pub stream: bool,
    #[schemars(description = "Whether system.capabilities lists the method")]
    pub in_capabilities: bool,
    #[schemars(description = "Whether the endpoint that answered serves the method itself")]
    pub callable: bool,
    #[schemars(description = "JSON Schema 2020-12 of the params object")]
    pub params: serde_json::Value,
    #[schemars(description = "JSON Schema 2020-12 of the result")]
    pub result: serde_json::Value,
    pub examples: Vec<ExampleDescription>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MethodList {
    pub methods: Vec<MethodDescription>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Description {
    All(MethodList),
    One(Box<MethodDescription>),
}

method! {
    Describe {
        name: "system.describe",
        summary: "Describe every method, or one: owner, gate, params and result schemas, examples.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Inline,
        params: DescribeParams,
        result: Description,
        examples: [({"method": "system.ping"} => {
            "name": "system.ping",
            "summary": "Check that the endpoint answers.",
            "owner": "window",
            "fallback": null,
            "gate": "none",
            "gate_note": null,
            "reply": "inline",
            "stream": false,
            "in_capabilities": true,
            "callable": true,
            "params": {"type": "object"},
            "result": {"type": "object", "properties": {"pong": {"type": "boolean"}}},
            "examples": [{"params": {}, "result": {"pong": true}}]
        })],
    }
}
