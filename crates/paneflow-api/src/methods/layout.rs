use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::NO_FALLBACK;
use super::events::PublishedEvent;
use crate::descriptor::{Audience, Gate, Owner, Reply};

const WORKER: Owner = Owner::Worker;

pub const ERR_LAYOUT_REFUSED: i64 = -32070;
pub const WINDOW_OWNS_LAYOUT: &str = "window_owns_layout";
pub const NOT_ATTACHED: &str = "not_attached";
pub const LAUNCH_FAILED: &str = "launch_failed";
pub const HOST_UNREACHABLE: &str = "host_unreachable";
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
    #[schemars(
        description = "The session of the pane that asked the window for this op; the worker records it as the spawner of the terminals the op creates. Only the attached window may send it"
    )]
    pub spawned_by: Option<String>,
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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SweepParams {
    #[serde(default)]
    #[schemars(
        description = "Also stop the live sessions this host started that nothing holds, as the first sweep after a restore does"
    )]
    pub live: bool,
    #[serde(default)]
    #[schemars(
        description = "Sessions the client holds outside the layout, such as dock terminals, which the sweep must keep"
    )]
    pub keep: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Swept {
    #[schemars(description = "Ended sessions whose record was removed")]
    pub forgotten: Vec<String>,
    #[schemars(description = "Live sessions that were stopped")]
    pub stopped: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchParams {
    #[schemars(description = "The terminal surface to start")]
    pub surface_id: u64,
    #[schemars(description = "The session id the client chose for the new terminal")]
    pub session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "The working directory the client asks for")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "A worktree the working directory must stay inside; a cwd outside it starts at the worktree"
    )]
    pub confine_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "The directory used when cwd no longer exists")]
    pub fallback: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Variables for this terminal, above the global and space ones")]
    pub env: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Initial width in cells; 120 when absent")]
    pub cols: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "Initial height in cells; 40 when absent")]
    pub rows: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "The colors the terminal answers color queries with: {foreground, background, cursor, palette of 256 [r, g, b], dark}"
    )]
    pub appearance: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "The cell size in pixels: {width, height}")]
    pub cell: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Launched {
    pub revision: u64,
    pub events: Vec<PublishedEvent>,
    #[schemars(
        description = "Lines to show in the new terminal: a missing default_shell, a config directory that could not be created"
    )]
    pub notices: Vec<String>,
}

method! {
    ViewMethod {
        name: "layout.view",
        summary: "Return the layout model the worker holds, with its revision and the worker's epoch.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::Anyone,
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
        audience: Audience::NotPane,
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
        audience: Audience::Anyone,
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
        gate: Gate::OrchestrationWhenSpawning,
        audience: Audience::Anyone,
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
        audience: Audience::Anyone,
        reply: Reply::Ready,
        params: AdoptParams,
        result: Adopted,
        examples: [({"view": {"revision": 14, "state": {"version": 3, "workspaces": [], "active_workspace": 0, "active_space": null}}, "acknowledged": 12} => {"accepted": true, "revision": 14, "view": null})],
    }
}

method! {
    Launch {
        name: "layout.launch",
        summary: "Start the terminal of a surface for the attached layout client that created it: the worker builds the launch from its configuration and the layout and creates the session under the id the client chose, which the client binds with bind_session. The surface must have no session or this one. Fails with -32070 and data {reason}: not_attached, not_found, invalid, host_unreachable or launch_failed with the host's message.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::OrchestrationWhenSpawning,
        audience: Audience::PaneflowOnly,
        reply: Reply::Ready,
        params: LaunchParams,
        result: Launched,
        examples: [({"surface_id": 7, "session": "0190e4d2-7b2a-7c3d-8e4f-a1b2c3d4e5f6", "cwd": "/home/me/project"} => {"revision": 15, "events": [{"type": "layout.changed", "workspace_id": 1, "tab_id": 2, "data": {"revision": 15}}], "notices": []})],
    }
}

method! {
    Sweep {
        name: "layout.sweep",
        summary: "Remove the records of ended sessions that no surface of the layout, no undo hold and no pending launch names, and with live, also stop the live sessions this host started that nothing holds. The attached layout client calls it after it restores; the worker runs the host requests. Fails with -32070 and data {reason}: not_attached or host_unreachable.",
        owner: WORKER,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::PaneflowOnly,
        reply: Reply::Ready,
        params: SweepParams,
        result: Swept,
        examples: [({"live": true, "keep": []} => {"forgotten": ["0190e4d2-7b2a-7c3d-8e4f-a1b2c3d4e5f6"], "stopped": []})],
    }
}
