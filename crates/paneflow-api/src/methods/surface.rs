use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    EnvMap, HOST_FALLBACK, ManagedWorktreeSpec, NO_FALLBACK, NoParams, SpaceRef, SurfaceProfile,
    WINDOW, WriteScope,
};
use crate::descriptor::{Gate, Reply};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListParams {
    #[schemars(description = "List this workspace instead of the shown one")]
    pub workspace_id: Option<u64>,
    #[schemars(description = "List the workspace that holds this session")]
    pub scope_session: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SurfaceRow {
    pub surface_id: u64,
    pub name: String,
    pub title: String,
    pub cwd: Option<String>,
    pub cmd: Option<String>,
    pub workspace_id: Option<u64>,
    pub workspace: Option<usize>,
    pub scope: String,
    pub tab_id: Option<u64>,
    pub tab_title: Option<String>,
    pub space: Option<SpaceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SurfaceList {
    pub pane_count: usize,
    pub workspace: usize,
    pub scope_workspace_id: Option<u64>,
    pub surfaces: Vec<SurfaceRow>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadParams {
    #[schemars(
        description = "The surface to read; the first surface of the shown tab when absent"
    )]
    pub surface_id: Option<u64>,
    #[schemars(description = "Pick the surface by its unique name instead of its id")]
    pub name: Option<String>,
    #[schemars(
        description = "Trailing lines to return, clamped to 1..4000",
        extend("default" = 200)
    )]
    pub lines: Option<usize>,
    #[schemars(
        description = "Lines to skip back from the end of the buffer",
        extend("default" = 0)
    )]
    pub offset: Option<usize>,
    #[schemars(
        description = "Wrap the text in an untrusted-output fence; the ai_injection_fence setting decides when absent"
    )]
    pub fenced: Option<bool>,
    pub workspace_id: Option<u64>,
    pub scope_session: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SurfaceText {
    pub text: String,
    pub lines: usize,
    pub total_lines: usize,
    pub eof: bool,
    pub output_generation: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SearchParams {
    pub surface_id: Option<u64>,
    pub name: Option<String>,
    #[schemars(description = "Substring or pattern to find, at most 512 bytes")]
    pub pattern: String,
    #[schemars(description = "Matches to return, clamped to 1..1000", extend("default" = 50))]
    pub max_matches: Option<usize>,
    pub workspace_id: Option<u64>,
    pub scope_session: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchMatch {
    pub line: usize,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchResult {
    pub matches: Vec<SearchMatch>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TargetParams {
    pub surface_id: Option<u64>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SurfaceStatus {
    pub surface_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_seq: Option<u64>,
    pub hooked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_ms: Option<u64>,
    pub output_generation: u64,
    pub session: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FleetRow {
    pub pid: Option<u32>,
    pub tool: String,
    pub state: String,
    pub hooked: bool,
    pub reason: Option<String>,
    pub surface_id: Option<u64>,
    pub surface_name: Option<String>,
    pub workspace: usize,
    pub active_tool_name: Option<String>,
    pub message: Option<String>,
    pub last_result: Option<String>,
    pub waiting_ms: Option<u64>,
    pub idle_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Fleet {
    pub agents: Vec<FleetRow>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RenameParams {
    pub surface_id: Option<u64>,
    #[schemars(description = "The new name; an absent or empty name clears the custom name")]
    pub name: Option<String>,
    #[schemars(description = "Alias of name")]
    pub new_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Renamed {
    pub renamed: bool,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FocusParams {
    #[schemars(required)]
    pub surface_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Focused {
    pub focused: bool,
    pub surface_id: u64,
    pub workspace: usize,
    pub scope: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SendTextParams {
    pub surface_id: Option<u64>,
    #[schemars(description = "Text to write; required unless submit is true. At most 64 KiB")]
    pub text: Option<String>,
    #[schemars(
        description = "Submit with a carriage return after the text",
        extend("default" = false)
    )]
    pub submit: Option<bool>,
    #[schemars(
        description = "Force or refuse bracketed paste; decided from the target when absent"
    )]
    pub paste: Option<bool>,
    #[schemars(
        description = "Write even when the agent waits for a decision; the write is logged"
    )]
    pub force: Option<bool>,
    pub scope: Option<WriteScope>,
    #[schemars(description = "The calling pane's session; writes stay in its workspace")]
    pub scope_session: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubmitMode {
    DeferredPasteCr,
    InlineCr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SentText {
    pub sent: bool,
    pub length: usize,
    pub submitted: bool,
    pub paste: bool,
    pub submit_mode: Option<SubmitMode>,
    pub agent_target: bool,
    pub agent_tool: Option<String>,
    pub terminal_bracketed_paste: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SendKeystrokeParams {
    pub surface_id: Option<u64>,
    #[schemars(
        description = "Dash-separated key such as escape, ctrl-c or alt-f; required, and never one that submits a line"
    )]
    pub keystroke: Option<String>,
    pub scope: Option<WriteScope>,
    pub scope_session: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Sent {
    pub sent: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SplitParams {
    #[schemars(description = "horizontal stacks the panes, vertical puts them side by side")]
    pub direction: SplitDirection,
    #[schemars(
        description = "Split the pane holding this surface; the shown tab's first pane when absent"
    )]
    pub surface_id: Option<u64>,
    pub cwd: Option<String>,
    pub command: Option<String>,
    pub prompt: Option<String>,
    pub env: Option<EnvMap>,
    pub profile: Option<SurfaceProfile>,
    pub label: Option<String>,
    #[schemars(description = "Alias of label")]
    pub name: Option<String>,
    pub managed_worktree: Option<ManagedWorktreeSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SplitDone {
    pub split: bool,
    pub direction: String,
    pub panes: usize,
    pub surface_id: u64,
}

method! {
    List {
        name: "surface.list",
        summary: "List the surfaces of the shown workspace, or of one workspace, with their identity.",
        owner: WINDOW,
        fallback: HOST_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Ready,
        capabilities: true,
        params: ListParams,
        result: SurfaceList,
        examples: [({} => {
            "pane_count": 1,
            "workspace": 0,
            "scope_workspace_id": null,
            "surfaces": [{
                "surface_id": 7,
                "name": "shell",
                "title": "~/dev",
                "cwd": "/home/me/dev",
                "cmd": "zsh",
                "workspace_id": 1,
                "workspace": 0,
                "scope": "workspace",
                "tab_id": 1,
                "tab_title": "Terminal",
                "space": null
            }]
        })],
    }
}

method! {
    Read {
        name: "surface.read",
        summary: "Read a surface's trailing scrollback lines.",
        owner: WINDOW,
        fallback: HOST_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Deferred,
        capabilities: true,
        params: ReadParams,
        result: SurfaceText,
        examples: [({"surface_id": 7, "lines": 2, "fenced": false} => {
            "text": "cargo test\nok",
            "lines": 2,
            "total_lines": 40,
            "eof": false,
            "output_generation": 12,
            "truncated": false
        })],
    }
}

method! {
    Search {
        name: "surface.search",
        summary: "Find the scrollback lines of a surface that contain a pattern.",
        owner: WINDOW,
        fallback: HOST_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Deferred,
        capabilities: true,
        params: SearchParams,
        result: SearchResult,
        examples: [({"surface_id": 7, "pattern": "error"} => {
            "matches": [{"line": 31, "text": "error[E0308]: mismatched types"}],
            "truncated": false
        })],
    }
}

method! {
    Status {
        name: "surface.status",
        summary: "Read a surface's agent state, hook status and output generation.",
        owner: WINDOW,
        fallback: HOST_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Ready,
        capabilities: true,
        params: TargetParams,
        result: SurfaceStatus,
        examples: [({"surface_id": 7} => {
            "surface_id": 7,
            "state": "thinking",
            "state_seq": 4,
            "hooked": true,
            "tool": "claude",
            "active_tool_name": "Bash",
            "message": null,
            "last_result": null,
            "waiting_ms": null,
            "idle_ms": 1200,
            "output_generation": 12,
            "session": "123e4567-e89b-42d3-a456-426614174000"
        })],
    }
}

method! {
    FleetList {
        name: "fleet.list",
        summary: "List the agents running in every workspace.",
        owner: WINDOW,
        fallback: HOST_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Ready,
        capabilities: true,
        params: NoParams,
        result: Fleet,
        examples: [({} => {"agents": [{
            "pid": 4242,
            "tool": "claude",
            "state": "waiting_for_input",
            "hooked": true,
            "reason": null,
            "surface_id": 7,
            "surface_name": "claude",
            "workspace": 0,
            "active_tool_name": null,
            "message": "needs a decision",
            "last_result": null,
            "waiting_ms": 3000,
            "idle_ms": 3000
        }]})],
    }
}

method! {
    Rename {
        name: "surface.rename",
        summary: "Give a surface a custom name, or clear it.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Ready,
        capabilities: true,
        params: RenameParams,
        result: Renamed,
        examples: [({"surface_id": 7, "name": "tests"} => {"renamed": true, "name": "tests"})],
    }
}

method! {
    Focus {
        name: "surface.focus",
        summary: "Reveal a surface and give it the keyboard focus.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        reply: Reply::Ready,
        capabilities: true,
        params: FocusParams,
        result: Focused,
        examples: [({"surface_id": 7} => {"focused": true, "surface_id": 7, "workspace": 0, "scope": "workspace"})],
    }
}

method! {
    SendText {
        name: "surface.send_text",
        summary: "Write text into a surface, submitting it only when asked.",
        owner: WINDOW,
        fallback: HOST_FALLBACK,
        gate: Gate::Scripting {
            refusal: "surface.send_text disabled; set PANEFLOW_IPC_SCRIPTING=1 to enable",
        },
        reply: Reply::Ready,
        capabilities: true,
        params: SendTextParams,
        result: SentText,
        examples: [({"surface_id": 7, "text": "cargo test"} => {
            "sent": true,
            "length": 10,
            "submitted": false,
            "paste": false,
            "submit_mode": null,
            "agent_target": false,
            "agent_tool": null,
            "terminal_bracketed_paste": false
        })],
    }
}

method! {
    SendKeystroke {
        name: "surface.send_keystroke",
        summary: "Send a named key such as escape or ctrl-c to a surface.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: Gate::Scripting {
            refusal: "surface.send_keystroke disabled; set PANEFLOW_IPC_SCRIPTING=1 or enable ai_unrestricted to use",
        },
        reply: Reply::Ready,
        capabilities: true,
        params: SendKeystrokeParams,
        result: Sent,
        examples: [({"surface_id": 7, "keystroke": "ctrl-c"} => {"sent": true})],
    }
}

method! {
    Split {
        name: "surface.split",
        summary: "Split the pane holding a surface and start a terminal in the new pane.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: Gate::OrchestrationWhenSpawning,
        reply: Reply::Async,
        capabilities: true,
        params: SplitParams,
        result: SplitDone,
        examples: [({"surface_id": 7, "direction": "vertical"} => {
            "split": true,
            "direction": "vertical",
            "panes": 2,
            "surface_id": 9
        })],
    }
}
