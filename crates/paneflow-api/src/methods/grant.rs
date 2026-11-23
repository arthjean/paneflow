use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{NO_FALLBACK, NoParams};
use crate::descriptor::{Audience, Gate, Owner, Reply};
use crate::grants::{GrantScope, GrantSet, Right};

const WORKER: Owner = Owner::Worker;

pub const GRANT_CHANGED: &str = "grant.changed";

pub const DEFAULT_GRANT_SECS: u64 = 3600;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GrantRequestParams {
    #[schemars(description = "Rights asked for, among write, spawn and layout")]
    pub rights: Vec<Right>,
    #[schemars(
        description = "workspace reaches the calling pane's own workspace, instance every workspace",
        extend("default" = "workspace")
    )]
    pub scope: Option<GrantScope>,
    #[schemars(
        description = "Seconds the grant lasts once allowed, 3600 by default; 0 asks for a grant with no expiry"
    )]
    pub duration_secs: Option<u64>,
    #[schemars(description = "Why the agent needs it, shown to the human")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Granted {
    pub granted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Unix time in milliseconds the grant ends at; absent when it has none"
    )]
    pub expires_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "denied, unanswered, or why the grant could not be stored")]
    pub reason: Option<String>,
}

method! {
    Request {
        name: "grant.request",
        summary: "Ask the human for rights the calling pane does not hold, for a scope and a time.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::PaneOnly,
        reply: Reply::Ready,
        params: GrantRequestParams,
        result: Granted,
        examples: [
            ({"rights": ["write"], "scope": "workspace", "duration_secs": 600} => {"granted": true, "id": "g-1", "expires_at_ms": 1_800_000_000_000_u64}),
            ({"rights": ["write"]} => {"granted": false, "reason": "denied"})
        ],
    }
}

method! {
    List {
        name: "grant.list",
        summary: "List the grants panes hold, with the revision of the set; a pane sees the grants it holds or delegated.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::Anyone,
        reply: Reply::Ready,
        params: NoParams,
        result: GrantSet,
        examples: [({} => {"revision": 4, "grants": []})],
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RevokeParams {
    #[schemars(description = "The grant to revoke; every grant delegated from it is revoked too")]
    pub id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Revoked {
    pub revoked: Vec<String>,
    pub revision: u64,
}

method! {
    Revoke {
        name: "grant.revoke",
        summary: "Revoke a grant and every grant delegated from it.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::Anyone,
        reply: Reply::Ready,
        params: RevokeParams,
        result: Revoked,
        examples: [({"id": "g-1"} => {"revoked": ["g-1", "g-2"], "revision": 5})],
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DelegateParams {
    #[schemars(description = "The session of a pane the caller spawned with surface.split")]
    pub session: String,
    pub rights: Vec<Right>,
    #[schemars(extend("default" = "workspace"))]
    pub scope: Option<GrantScope>,
    #[schemars(
        description = "Seconds the delegated grant lasts, until the delegator's grant ends by default"
    )]
    pub duration_secs: Option<u64>,
}

method! {
    Delegate {
        name: "grant.delegate",
        summary: "Give a pane the caller spawned a grant no wider than one the caller holds.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::PaneOnly,
        reply: Reply::Ready,
        params: DelegateParams,
        result: Granted,
        examples: [({"session": "223e4567-e89b-42d3-a456-426614174000", "rights": ["write"]} => {"granted": true, "id": "g-2", "expires_at_ms": 1_800_000_000_000_u64})],
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GrantsInstalled {
    pub revision: u64,
    pub installed: bool,
}

method! {
    HostGrantsSet {
        name: "host.grants.set",
        summary: "Replace the grant set the host applies to callers in a pane.",
        owner: Owner::Host,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::PaneflowOnly,
        reply: Reply::Ready,
        params: GrantSet,
        result: GrantsInstalled,
        examples: [({"revision": 4, "grants": []} => {"revision": 4, "installed": true})],
    }
}
