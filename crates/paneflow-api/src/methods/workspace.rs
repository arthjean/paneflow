use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    EnvMap, LayoutDocument, ManagedWorktreeSpec, NO_FALLBACK, NoParams, SpaceRef, SurfaceProfile,
    WINDOW, WorktreeRequestSpec,
};
use crate::descriptor::{Gate, Reply};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRow {
    pub index: usize,
    pub title: String,
    pub cwd: String,
    pub panes: usize,
    pub active: bool,
    pub space: Option<SpaceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceList {
    pub workspaces: Vec<WorkspaceRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CurrentWorkspace {
    pub index: usize,
    pub title: String,
    pub cwd: String,
    pub panes: usize,
    pub space: Option<SpaceRef>,
    pub layout: Option<LayoutDocument>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CreateParams {
    #[schemars(description = "Workspace title", extend("default" = "Terminal"))]
    pub name: Option<String>,
    #[schemars(description = "Space by name or id; the active space when absent")]
    pub space: Option<String>,
    #[schemars(description = "Directory of the first pane; it must exist")]
    pub cwd: Option<String>,
    pub layout: Option<LayoutDocument>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Created {
    pub index: usize,
    pub title: String,
    pub panes: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct IndexParams {
    #[schemars(required, description = "Zero-based workspace index")]
    pub index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Selected {
    pub selected: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Closed {
    pub closed: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RestoreLayoutParams {
    #[schemars(required, description = "Holds between 1 and 32 panes")]
    pub layout: Option<LayoutDocument>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Restored {
    pub restored: bool,
    pub panes: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PaneSpec {
    pub cwd: Option<String>,
    pub command: Option<String>,
    pub prompt: Option<String>,
    pub env: Option<EnvMap>,
    pub profile: Option<SurfaceProfile>,
    pub focus: Option<bool>,
    pub label: Option<String>,
    #[schemars(description = "Alias of label")]
    pub name: Option<String>,
    pub managed_worktree: Option<ManagedWorktreeSpec>,
    #[schemars(
        description = "Create or reuse a Paneflow worktree for this pane, which then starts in it; exclusive with managed_worktree"
    )]
    pub worktree: Option<WorktreeRequestSpec>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpParams {
    #[schemars(description = "Workspace title", extend("default" = "Workspace"))]
    pub name: Option<String>,
    pub space: Option<String>,
    #[schemars(description = "Layout preset", extend("default" = "even_h"))]
    pub layout: Option<String>,
    #[schemars(length(min = 1))]
    pub panes: Vec<PaneSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Opened {
    pub index: usize,
    pub title: String,
    pub panes: usize,
    pub surface_ids: Vec<u64>,
    pub labels: Vec<Option<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(
        description = "What preparing the worktrees reported: files copied, a setup that failed"
    )]
    pub notices: Vec<String>,
}

method! {
    List {
        name: "workspace.list",
        summary: "List every workspace with its title, directory, pane count and space.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Ready,
        capabilities: true,
        params: NoParams,
        result: WorkspaceList,
        examples: [({} => {"workspaces": [{
            "index": 0,
            "title": "paneflow",
            "cwd": "/home/me/dev/paneflow",
            "panes": 2,
            "active": true,
            "space": null
        }]})],
    }
}

method! {
    Current {
        name: "workspace.current",
        summary: "Describe the shown workspace with its layout document, or null when none is shown.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Ready,
        capabilities: true,
        params: NoParams,
        result: Option<CurrentWorkspace>,
        examples: [({} => {
            "index": 0,
            "title": "paneflow",
            "cwd": "/home/me/dev/paneflow",
            "panes": 1,
            "space": null,
            "layout": {"type": "pane", "surfaces": [{"surface_type": "terminal", "name": "shell"}]}
        })],
    }
}

method! {
    Create {
        name: "workspace.create",
        summary: "Open a workspace with one terminal, or with a layout document.",
        owner: crate::descriptor::Owner::Worker,
        fallback: NO_FALLBACK,
        gate: Gate::OrchestrationWhenSpawning,
        reply: Reply::Async,
        capabilities: true,
        params: CreateParams,
        result: Created,
        examples: [({"name": "review"} => {"index": 1, "title": "review", "panes": 1})],
    }
}

method! {
    Select {
        name: "workspace.select",
        summary: "Show the workspace at an index.",
        owner: crate::descriptor::Owner::Worker,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Ready,
        capabilities: true,
        params: IndexParams,
        result: Selected,
        examples: [({"index": 1} => {"selected": 1})],
    }
}

method! {
    Close {
        name: "workspace.close",
        summary: "Close the workspace at an index; the last workspace cannot be closed.",
        owner: crate::descriptor::Owner::Worker,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Ready,
        capabilities: true,
        params: IndexParams,
        result: Closed,
        examples: [({"index": 1} => {"closed": 1})],
    }
}

method! {
    RestoreLayout {
        name: "workspace.restore_layout",
        summary: "Conform the shown tab to a layout document, reusing its panes.",
        owner: crate::descriptor::Owner::Worker,
        fallback: NO_FALLBACK,
        gate: Gate::OrchestrationWhenSpawning,
        reply: Reply::Ready,
        capabilities: true,
        params: RestoreLayoutParams,
        result: Restored,
        examples: [({"layout": {"type": "split", "direction": "vertical", "children": [
            {"type": "pane", "surfaces": [{}]},
            {"type": "pane", "surfaces": [{}]}
        ]}} => {"restored": true, "panes": 2})],
    }
}

method! {
    Up {
        name: "workspace.up",
        summary: "Open a workspace from pane specs, each with its own directory, command or prompt.",
        owner: crate::descriptor::Owner::Worker,
        fallback: NO_FALLBACK,
        gate: Gate::OrchestrationWhenSpawning,
        reply: Reply::Async,
        capabilities: true,
        params: UpParams,
        result: Opened,
        examples: [({"name": "agents", "panes": [{"label": "api"}, {"label": "web"}]} => {
            "index": 1,
            "title": "agents",
            "panes": 2,
            "surface_ids": [11, 12],
            "labels": ["api", "web"]
        })],
    }
}
