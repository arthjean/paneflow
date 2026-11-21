use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::NO_FALLBACK;
use super::events::PublishedEvent;
use crate::descriptor::{Gate, Owner, Reply};

const WORKER: Owner = Owner::Worker;

pub const ERR_LAYOUT_REFUSED: i64 = -32070;
pub const WINDOW_OWNS_LAYOUT: &str = "window_owns_layout";
pub const NOT_ATTACHED: &str = "not_attached";
pub const LEASE_SIZE: u64 = 1_024;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LayoutSnapshot(pub Map<String, Value>);

impl JsonSchema for LayoutSnapshot {
    fn schema_name() -> Cow<'static, str> {
        "LayoutSnapshot".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "object",
            "description": "The layout model as plain data: {revision, state: {version, workspaces, active_workspace, active_space}, undo}, every workspace, tab, pane, split and surface carrying its stable id"
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LayoutOp(pub Map<String, Value>);

impl JsonSchema for LayoutOp {
    fn schema_name() -> Cow<'static, str> {
        "LayoutOp".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "object",
            "description": "One layout operation, named by its op field: split, close_pane, focus_pane, rename_surface, set_ratios, bind_session and the others of paneflow-layout, with the ids it acts on"
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ViewParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "The revision the client holds; when it is still the worker's, view is null"
    )]
    pub unless_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct View {
    #[schemars(
        description = "Names this worker start; a client that sees another epoch fetches the view again"
    )]
    pub epoch: String,
    #[schemars(description = "Increases by one for every change of the model")]
    pub revision: u64,
    #[schemars(description = "Whether a window is attached as a layout client")]
    pub attached: bool,
    #[schemars(description = "The model, or null when unless_revision names the current revision")]
    pub view: Option<LayoutSnapshot>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AttachParams {
    #[schemars(description = "A name for the client in worker.status")]
    pub client: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Attached {
    pub epoch: String,
    pub revision: u64,
    pub view: LayoutSnapshot,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LeaseParams {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Lease {
    #[schemars(description = "First id of the block")]
    pub start: u64,
    #[schemars(description = "One past the last id of the block")]
    pub end: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyParams {
    pub op: LayoutOp,
    #[schemars(
        description = "The client's next id when it applied the op; ids the op allocates are taken from the client's leases from here"
    )]
    pub ids_from: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AppliedOp {
    pub revision: u64,
    pub events: Vec<PublishedEvent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdoptParams {
    pub view: LayoutSnapshot,
    #[schemars(description = "The last revision the worker acknowledged to this client")]
    pub acknowledged: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Adopted {
    #[schemars(
        description = "True when the worker took the client's model; false when its own revision was greater than the acknowledged one"
    )]
    pub accepted: bool,
    pub revision: u64,
    #[schemars(description = "The worker's view when it refused the client's model")]
    pub view: Option<LayoutSnapshot>,
}

method! {
    ViewMethod {
        name: "layout.view",
        summary: "Return the layout model the worker holds, with its revision and the worker's epoch.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Ready,
        params: ViewParams,
        result: View,
        examples: [({} => {"epoch": "1a2b-19a7c3f0e00-0", "revision": 12, "attached": true, "view": {"revision": 12, "state": {"version": 3, "workspaces": [], "active_workspace": 0, "active_space": null}, "undo": {"records": [], "held": []}}})],
    }
}

method! {
    Attach {
        name: "layout.attach",
        summary: "Attach this connection as a layout client until it closes, and return the view, its revision and the worker's epoch.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Ready,
        params: AttachParams,
        result: Attached,
        examples: [({"client": "window"} => {"epoch": "1a2b-19a7c3f0e00-0", "revision": 12, "view": {"revision": 12, "state": {"version": 3, "workspaces": [], "active_workspace": 0, "active_space": null}}})],
    }
}

method! {
    LeaseIds {
        name: "layout.lease_ids",
        summary: "Grant this connection a contiguous block of 1,024 ids for the elements it creates; the worker never grants an id twice, across restarts included.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Ready,
        params: LeaseParams,
        result: Lease,
        examples: [({} => {"start": 4097, "end": 5121})],
    }
}

method! {
    Apply {
        name: "layout.apply",
        summary: "Apply one layout op to the worker's model and return the new revision and its events. A refused op fails with -32070 and data {reason}: not_found, zoomed, pane_limit, tab_limit, surface_limit, ids_exhausted, invalid, or window_owns_layout while a window of an earlier build owns the layout.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Ready,
        params: ApplyParams,
        result: AppliedOp,
        examples: [({"op": {"op": "focus_pane", "pane": 5}} => {"revision": 13, "events": [{"type": "pane.focused", "workspace_id": 1, "tab_id": 2, "pane_id": 5, "data": {}}, {"type": "layout.changed", "workspace_id": 1, "tab_id": 2, "data": {"revision": 13}}]})],
    }
}

method! {
    Adopt {
        name: "layout.adopt",
        summary: "Hand the worker a client's model after an outage. The worker takes it when its own revision is not greater than the revision it last acknowledged to the client, and otherwise returns its own view.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Ready,
        params: AdoptParams,
        result: Adopted,
        examples: [({"view": {"revision": 14, "state": {"version": 3, "workspaces": [], "active_workspace": 0, "active_space": null}}, "acknowledged": 12} => {"accepted": true, "revision": 14, "view": null})],
    }
}
