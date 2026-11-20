use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{NO_FALLBACK, NoParams};
use crate::descriptor::{Gate, Owner, Reply};

const WORKER: Owner = Owner::Worker;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkerIdentity {
    pub name: String,
    pub version: String,
    pub build_id: String,
    pub protocol: u32,
    pub required_core_protocol: u32,
    pub pid: u32,
    pub home: String,
    pub endpoint: String,
    pub started_at_ms: u64,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WorkerStatus {
    pub pid: u32,
    pub protocol: u32,
    pub home: String,
    pub endpoint: String,
    pub version: String,
    pub session_count: usize,
    pub core_connected: bool,
    pub capabilities: Vec<String>,
    pub counters: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Stopping {
    pub stopping: bool,
    pub pid: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentSnapshot {
    pub worker: WorkerIdentity,
    pub capabilities: Vec<String>,
    pub core_connected: bool,
    #[schemars(description = "One reduced row per session: status, activity, unread, runtime")]
    pub sessions: Vec<Value>,
    pub usage: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub following: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActivityLogParams {
    #[schemars(description = "Entries to return, 50 by default")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ActivityLog {
    pub entries: Vec<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AcknowledgeParams {
    pub sessions: Option<Vec<String>>,
    #[schemars(description = "One session, when sessions is absent")]
    pub session: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Acknowledged {
    pub acknowledged: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageAccount {
    #[schemars(description = "The agent runtime slug, such as claude or codex")]
    pub agent: String,
    #[schemars(description = "Absolute path of the agent's configuration directory")]
    pub config_dir: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageParams {
    pub accounts: Option<Vec<UsageAccount>>,
    #[schemars(description = "usage.import only: start again even when an import already ran")]
    pub force: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct UsageSnapshot(pub serde_json::Map<String, Value>);

method! {
    Hello {
        name: "worker.hello",
        summary: "Open a worker connection and read the worker's identity and capabilities.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Ready,
        params: NoParams,
        result: WorkerIdentity,
        examples: [({} => {
            "name": "paneflow-serve",
            "version": "0.20.0",
            "build_id": "9f2c",
            "protocol": 1,
            "required_core_protocol": 1,
            "pid": 4242,
            "home": "/home/me/.paneflow",
            "endpoint": "/home/me/.paneflow/serve/worker.sock",
            "started_at_ms": 1763600000000_u64,
            "capabilities": ["agent.follow"]
        })],
    }
}

method! {
    Status {
        name: "worker.status",
        summary: "Read the worker's pid, version, session count and counters.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Ready,
        params: NoParams,
        result: WorkerStatus,
        examples: [({} => {
            "pid": 4242,
            "protocol": 1,
            "home": "/home/me/.paneflow",
            "endpoint": "/home/me/.paneflow/serve/worker.sock",
            "version": "0.20.0",
            "session_count": 3,
            "core_connected": true,
            "capabilities": ["agent.follow"],
            "counters": {"sweeps": 10}
        })],
    }
}

method! {
    Shutdown {
        name: "worker.shutdown",
        summary: "Ask the worker to stop; terminals keep running in the host.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Ready,
        params: NoParams,
        result: Stopping,
        examples: [({} => {"stopping": true, "pid": 4242})],
    }
}

method! {
    Snapshot {
        name: "agent.snapshot",
        summary: "Read every session's reduced agent row and the usage frame.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Ready,
        params: NoParams,
        result: AgentSnapshot,
        examples: [({} => {
            "worker": {
                "name": "paneflow-serve", "version": "0.20.0", "build_id": "9f2c", "protocol": 1,
                "required_core_protocol": 1, "pid": 4242, "home": "/home/me/.paneflow",
                "endpoint": "/home/me/.paneflow/serve/worker.sock", "started_at_ms": 1763600000000_u64,
                "capabilities": ["agent.follow"]
            },
            "capabilities": ["agent.follow"],
            "core_connected": true,
            "sessions": [],
            "usage": {}
        })],
    }
}

method! {
    Follow {
        name: "agent.follow",
        summary: "Stream the session projection: a snapshot header, then one frame per reduced transition.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Stream,
        params: NoParams,
        result: AgentSnapshot,
        examples: [({} => {
            "worker": {
                "name": "paneflow-serve", "version": "0.20.0", "build_id": "9f2c", "protocol": 1,
                "required_core_protocol": 1, "pid": 4242, "home": "/home/me/.paneflow",
                "endpoint": "/home/me/.paneflow/serve/worker.sock", "started_at_ms": 1763600000000_u64,
                "capabilities": ["agent.follow"]
            },
            "capabilities": ["agent.follow"],
            "core_connected": true,
            "sessions": [],
            "usage": {},
            "following": true
        })],
    }
}

method! {
    ActivityLogMethod {
        name: "agent.activity_log",
        summary: "Read the most recent agent activity entries.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Ready,
        params: ActivityLogParams,
        result: ActivityLog,
        examples: [({"limit": 2} => {"entries": []})],
    }
}

method! {
    Acknowledge {
        name: "agent.acknowledge",
        summary: "Clear the unread mark of sessions and publish the lowered rows.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Ready,
        params: AcknowledgeParams,
        result: Acknowledged,
        examples: [({"sessions": ["123e4567-e89b-42d3-a456-426614174000"]} => {
            "acknowledged": ["123e4567-e89b-42d3-a456-426614174000"]
        })],
    }
}

method! {
    UsageSnapshotMethod {
        name: "usage.snapshot",
        summary: "Read the usage ledger snapshot.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Ready,
        params: NoParams,
        result: UsageSnapshot,
        examples: [({} => {})],
    }
}

method! {
    UsageRefresh {
        name: "usage.refresh",
        summary: "Read fresh usage for the running agents and the listed accounts.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Ready,
        params: UsageParams,
        result: UsageSnapshot,
        examples: [({"accounts": [{"agent": "codex", "config_dir": "/home/me/.codex"}]} => {})],
    }
}

method! {
    UsageImport {
        name: "usage.import",
        summary: "Start importing past activity from the agents' local logs.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Ready,
        params: UsageParams,
        result: UsageSnapshot,
        examples: [({"force": false} => {})],
    }
}
