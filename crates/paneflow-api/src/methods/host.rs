use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{NO_FALLBACK, WriteScope};
use crate::descriptor::{Gate, Owner, Reply};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PaneWriteParams {
    #[schemars(description = "The calling pane's session (PANEFLOW_SESSION_ID)")]
    pub source_session: String,
    #[schemars(description = "The session to write into")]
    pub session: String,
    #[schemars(description = "Text to relay, prefixed with a provenance line")]
    pub text: String,
    pub submit: Option<bool>,
    pub scope: Option<WriteScope>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PaneWritten {
    #[schemars(description = "written, approval_pending or no_window")]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit_mode: Option<String>,
}

method! {
    PaneWrite {
        name: "pane.write",
        summary: "Relay a message from one agent's pane into another's, once a human allows the pair.",
        owner: Owner::Host,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Ready,
        params: PaneWriteParams,
        result: PaneWritten,
        examples: [({
            "source_session": "123e4567-e89b-42d3-a456-426614174000",
            "session": "223e4567-e89b-42d3-a456-426614174000",
            "text": "the tests pass on main"
        } => {"status": "approval_pending", "request": "7", "message": "a human must allow it"})],
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PeerClassifyParams {
    #[schemars(description = "PID of the process to classify")]
    pub pid: u32,
    #[schemars(
        description = "Start time the caller read for that PID, in the host's process clock"
    )]
    pub started_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PeerClass {
    #[schemars(description = "True when the process runs inside one of the host's panes")]
    pub in_pane: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "True when the PID no longer names the process the caller read")]
    pub stale: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Why membership could not be decided")]
    pub unverified: Option<String>,
}

method! {
    PeerClassify {
        name: "peer.classify",
        summary: "Say whether a process runs inside one of the host's panes, and which one.",
        owner: Owner::Host,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Ready,
        params: PeerClassifyParams,
        result: PeerClass,
        examples: [
            ({"pid": 4242, "started_at": 9876543} => {"in_pane": true, "session": "123e4567-e89b-42d3-a456-426614174000", "workspace": "223e4567-e89b-42d3-a456-426614174000"}),
            ({"pid": 4242, "started_at": 9876543} => {"in_pane": false, "stale": true})
        ],
    }
}
