use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{NO_FALLBACK, WINDOW};
use crate::descriptor::{Gate, Reply};

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
