use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{NO_FALLBACK, WINDOW};
use crate::descriptor::{Gate, Owner, Reply};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubscribeParams {
    #[schemars(description = "Only stream events about these surfaces")]
    pub surfaces: Option<Vec<u64>>,
    #[schemars(
        description = "Only stream these event types: surface_changed and the ai.* lifecycle types"
    )]
    pub types: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubscribedKind {
    Subscribed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Subscribed {
    #[serde(rename = "type")]
    pub kind: SubscribedKind,
    pub id: u64,
}

method! {
    Subscribe {
        name: "events.subscribe",
        summary: "Stream surface and agent events as JSON lines: a subscribed frame, then events, heartbeat and dropped frames.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Stream,
        capabilities: true,
        params: SubscribeParams,
        result: Subscribed,
        examples: [({"types": ["surface_changed"]} => {"type": "subscribed", "id": 1})],
    }
}

pub const LAYOUT_EVENT_TYPES: &[&str] = &[
    "workspace.created",
    "workspace.closed",
    "workspace.renamed",
    "workspace.selected",
    "tab.created",
    "tab.closed",
    "tab.renamed",
    "tab.selected",
    "pane.created",
    "pane.closed",
    "pane.focused",
    "surface.created",
    "surface.closed",
    "surface.renamed",
    "layout.changed",
    "layout.resync",
];

pub const TERMINAL_EVENT_TYPES: &[&str] = &[
    "terminal.cwd_changed",
    "terminal.title_changed",
    "terminal.bell",
    "terminal.exited",
];

pub const AGENT_EVENT_TYPES: &[&str] = &["agent.state_changed"];

pub fn is_layout_event_type(kind: &str) -> bool {
    LAYOUT_EVENT_TYPES.contains(&kind)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EventIds {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "The terminal session id the host assigned")]
    pub session: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Event {
    #[schemars(
        description = "Position in the worker's log: consecutive from 1 for a worker start"
    )]
    pub seq: u64,
    #[schemars(description = "Milliseconds since the Unix epoch when the worker appended it")]
    pub ts: u64,
    #[serde(rename = "type")]
    #[schemars(description = "A dotted event type, such as terminal.exited or pane.created")]
    pub kind: String,
    #[serde(flatten)]
    pub ids: EventIds,
    pub data: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FollowParams {
    #[schemars(
        description = "Replay the events after this seq first; refused with an evicted error when they left the ring"
    )]
    pub since: Option<u64>,
    #[schemars(
        description = "The epoch since belongs to; a different epoch is refused with an evicted error"
    )]
    pub epoch: Option<String>,
    #[schemars(description = "Only send these types: exact names or a family such as terminal.*")]
    pub types: Option<Vec<String>>,
    #[schemars(description = "Only send events about this layout workspace id")]
    pub workspace_id: Option<u64>,
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
    #[schemars(
        description = "The seq of the last event appended before the stream opened, 0 when none"
    )]
    pub seq: u64,
    #[schemars(description = "Names this worker start; a new epoch restarts seq at 1")]
    pub epoch: String,
}

method! {
    Follow {
        name: "events.follow",
        summary: "Stream the worker's numbered event log: a following header, then every event with consecutive seq values, keepalive frames when quiet, and an end frame when the worker closes the stream. A since that left the ring, or an epoch from another worker start, is refused with -32040 and data {reason: evicted, seq, epoch}.",
        owner: Owner::Worker,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Stream,
        params: FollowParams,
        result: FollowHeader,
        examples: [({"types": ["terminal.*", "pane.closed"]} => {"type": "following", "seq": 41, "epoch": "1a2b-19a7c3f0e00-0"})],
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PublishedEvent {
    #[serde(rename = "type")]
    #[schemars(
        description = "A layout event type: workspace.*, tab.*, pane.*, surface.* or layout.*"
    )]
    pub kind: String,
    #[serde(flatten)]
    pub ids: EventIds,
    #[serde(default)]
    pub data: serde_json::Map<String, serde_json::Value>,
}
