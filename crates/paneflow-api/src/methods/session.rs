use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{EnvMap, NO_FALLBACK, NoParams, WriteScope};
use crate::descriptor::{Audience, Gate, Owner, Reply, ScopeRule};

const HOST: Owner = Owner::Host;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Fields(pub Map<String, Value>);

impl JsonSchema for Fields {
    fn schema_name() -> Cow<'static, str> {
        "Fields".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({"type": "object"})
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HelloParams {
    #[schemars(description = "Name of the connecting client")]
    pub client: String,
    #[schemars(description = "Host protocol version the client speaks")]
    pub protocol: u32,
    #[schemars(
        description = "Terminal engine the client decodes; only the Paneflow window declares one"
    )]
    pub engine: Option<Fields>,
    pub build: Option<String>,
}

method! {
    HostHello {
        name: "host.hello",
        summary: "Open a host connection: the first request on every connection to the terminal host.",
        owner: HOST,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::Anyone,
        reply: Reply::Ready,
        params: HelloParams,
        result: Fields,
        examples: [({"client": "paneflow-cli", "protocol": 1} => {"pid": 4242, "host_instance": "h-1"})],
    }
}

method! {
    HostStatus {
        name: "host.status",
        summary: "Report the terminal host's resources and work counters.",
        owner: HOST,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::Anyone,
        reply: Reply::Ready,
        params: NoParams,
        result: Fields,
        examples: [({} => {"resources": {"live_runtimes": 2}, "counters": {}})],
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ShutdownParams {
    #[schemars(
        description = "Stop the live sessions instead of refusing while some run",
        extend("default" = false)
    )]
    pub force: Option<bool>,
}

method! {
    HostShutdown {
        name: "host.shutdown",
        summary: "Stop the terminal host, refusing while live sessions remain unless forced.",
        owner: HOST,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::PaneflowOnly,
        reply: Reply::Ready,
        params: ShutdownParams,
        result: Fields,
        examples: [({"force": true} => {"stopping": true, "host_instance": "h-1", "ended_sessions": 2, "unresolved": []})],
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionListParams {
    #[schemars(description = "List only the sessions of this workspace")]
    pub workspace: Option<String>,
}

method! {
    SessionList {
        name: "session.list",
        summary: "List the sessions the terminal host records.",
        owner: HOST,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::Anyone,
        reply: Reply::Ready,
        params: SessionListParams,
        result: Fields,
        examples: [({} => {"sessions": []})],
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CreateParams {
    pub session: Option<String>,
    pub workspace: Option<String>,
    pub cwd: Option<String>,
    #[schemars(description = "Program to run instead of the default shell; needs orchestration")]
    pub shell: Option<String>,
    pub args: Option<Vec<String>>,
    pub env: Option<EnvMap>,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    pub title: Option<String>,
    pub appearance: Option<Fields>,
    pub cell: Option<Fields>,
    #[schemars(
        description = "Start the shell with a resume handoff for this argv and cwd; only the Paneflow window or worker may send it"
    )]
    pub resume: Option<ResumeParams>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ResumeParams {
    #[schemars(description = "The OSC 7577 record to resume")]
    pub record: Option<u64>,
    #[schemars(description = "A catalog command to resume, element by element")]
    pub argv: Option<Vec<String>>,
    #[schemars(description = "The absolute directory of the catalog command")]
    pub cwd: Option<String>,
    #[schemars(
        description = "Resume a record whose previous resume did not complete",
        extend("default" = false)
    )]
    pub confirmed: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RestartParams {
    #[schemars(description = "The session id")]
    pub session: String,
    #[schemars(description = "Refuse the call unless the session runs this launch generation")]
    pub generation: Option<u64>,
    #[schemars(
        description = "Start the shell with a resume handoff; only the Paneflow window or worker may send it"
    )]
    pub resume: Option<ResumeParams>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PrepareResumeParams {
    #[schemars(description = "The session id")]
    pub session: String,
    #[schemars(description = "The OSC 7577 record to resume")]
    pub record: Option<u64>,
    #[schemars(description = "A catalog command to resume, element by element")]
    pub argv: Option<Vec<String>>,
    #[schemars(description = "The absolute directory of the catalog command")]
    pub cwd: Option<String>,
    #[schemars(
        description = "Resume a record whose previous resume did not complete",
        extend("default" = false)
    )]
    pub confirmed: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DiscardResumeParams {
    #[schemars(description = "The session id")]
    pub session: String,
    #[schemars(description = "The OSC 7577 record to forget")]
    pub record: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ResumeAttemptParams {
    #[schemars(description = "The calling pane's session id")]
    pub session: Option<String>,
    #[schemars(description = "The attempt id written in the handoff")]
    pub attempt: String,
    #[schemars(description = "started or failed")]
    pub outcome: String,
    #[schemars(description = "The process the launcher became or started")]
    pub pid: Option<u32>,
    #[schemars(description = "not_found, not_executable, cwd_missing or handoff_invalid")]
    pub reason: Option<String>,
}

method! {
    SessionPrepareResume {
        name: "session.prepare_resume",
        summary: "Write a resume handoff for a live session and return the launcher and handoff paths to type.",
        owner: HOST,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::PaneflowOnly,
        reply: Reply::Ready,
        params: PrepareResumeParams,
        result: Fields,
        examples: [({"session": "123e4567-e89b-42d3-a456-426614174000", "record": 3, "confirmed": true} => {"attempt": "0123456789abcdef0123456789abcdef", "launcher": "/home/u/.paneflow/cache/bin/0.20.0/paneflow-resume", "handoff": "/home/u/.paneflow/sessions/123e4567-e89b-42d3-a456-426614174000/resume-0123456789abcdef0123456789abcdef.handoff", "record": 3})],
    }
}

method! {
    SessionDiscardResume {
        name: "session.discard_resume",
        summary: "Forget one OSC 7577 record of a session.",
        owner: HOST,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::PaneflowOnly,
        reply: Reply::Ready,
        params: DiscardResumeParams,
        result: Fields,
        examples: [({"session": "123e4567-e89b-42d3-a456-426614174000", "record": 3} => {"manifest": {"session": "123e4567-e89b-42d3-a456-426614174000"}})],
    }
}

method! {
    SessionResumeAttempt {
        name: "session.resume_attempt",
        summary: "Report whether the resume launcher of a pane started its program.",
        owner: HOST,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::PaneOnly,
        reply: Reply::Ready,
        params: ResumeAttemptParams,
        result: Fields,
        examples: [({"attempt": "0123456789abcdef0123456789abcdef", "outcome": "started", "pid": 4242} => {"outcome": "started", "pid": 4242})],
    }
}

method! {
    SessionCreate {
        name: "session.create",
        summary: "Start a terminal session in the host.",
        owner: HOST,
        fallback: NO_FALLBACK,
        gate: Gate::OrchestrationWhenSpawning,
        audience: Audience::Anyone,
        reply: Reply::Ready,
        params: CreateParams,
        result: Fields,
        examples: [({"cwd": "/tmp"} => {"manifest": {"session": "123e4567-e89b-42d3-a456-426614174000"}})],
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionParams {
    #[schemars(description = "The session id")]
    pub session: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GenerationParams {
    #[schemars(description = "The session id")]
    pub session: String,
    #[schemars(description = "Refuse the call unless the session runs this launch generation")]
    pub generation: Option<u64>,
}

macro_rules! session_method {
    ($ty:ident, $name:literal, $summary:literal, $params:ty, $audience:expr, $scope:expr, $result:tt) => {
        method! {
            $ty {
                name: $name,
                summary: $summary,
                owner: HOST,
                fallback: NO_FALLBACK,
                gate: Gate::Open,
                audience: $audience,
                scope: $scope,
                reply: Reply::Ready,
                params: $params,
                result: Fields,
                examples: [({"session": "123e4567-e89b-42d3-a456-426614174000"} => $result)],
            }
        }
    };
}

session_method!(
    SessionInspect,
    "session.inspect",
    "Describe one session the host records.",
    SessionParams,
    Audience::Anyone,
    ScopeRule::Unscoped,
    {"manifest": {"session": "123e4567-e89b-42d3-a456-426614174000"}}
);
session_method!(
    SessionStop,
    "session.stop",
    "Stop a live session.",
    GenerationParams,
    Audience::Anyone,
    ScopeRule::Workspace,
    {"manifest": {"session": "123e4567-e89b-42d3-a456-426614174000"}}
);
session_method!(
    SessionRestart,
    "session.restart",
    "Start an ended session again as its next generation.",
    RestartParams,
    Audience::Anyone,
    ScopeRule::Workspace,
    {"manifest": {"session": "123e4567-e89b-42d3-a456-426614174000"}}
);
session_method!(
    SessionRemove,
    "session.remove",
    "Forget an ended session's record.",
    SessionParams,
    Audience::Anyone,
    ScopeRule::Workspace,
    {"removed": {"session": "123e4567-e89b-42d3-a456-426614174000"}}
);
session_method!(
    SessionClearHistory,
    "session.clear_history",
    "Drop a session's scrollback.",
    GenerationParams,
    Audience::Anyone,
    ScopeRule::Workspace,
    {}
);
session_method!(
    SessionReset,
    "session.reset",
    "Reset a session's terminal state.",
    GenerationParams,
    Audience::Anyone,
    ScopeRule::Workspace,
    {}
);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct InputParams {
    pub session: String,
    pub generation: Option<u64>,
    #[schemars(description = "Bytes to write, base64 encoded")]
    pub data: String,
    #[schemars(description = "program when the bytes are not typed by a person")]
    pub origin: Option<String>,
    pub scope: Option<WriteScope>,
    #[schemars(description = "The calling pane's session; writes stay in its workspace")]
    pub scope_session: Option<String>,
}

method! {
    SessionInput {
        name: "session.input",
        summary: "Write bytes into a session's terminal.",
        owner: HOST,
        fallback: NO_FALLBACK,
        gate: Gate::Scripting {
            refusal: "session.input disabled; set PANEFLOW_IPC_SCRIPTING=1 to enable",
        },
        audience: Audience::Anyone,
        scope: ScopeRule::Workspace,
        reply: Reply::Ready,
        params: InputParams,
        result: Fields,
        examples: [({"session": "123e4567-e89b-42d3-a456-426614174000", "data": "bHMK"} => {"accepted_bytes": 3})],
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BindParams {
    pub session: String,
    pub generation: Option<u64>,
    pub runtime_id: Option<String>,
}

method! {
    SessionRuntimeBind {
        name: "session.runtime.bind",
        summary: "Bind the agent runtime that runs in a session.",
        owner: HOST,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::PaneflowOnly,
        reply: Reply::Ready,
        params: BindParams,
        result: Fields,
        examples: [({"session": "123e4567-e89b-42d3-a456-426614174000", "runtime_id": "claude"} => {"bound": true})],
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ResizeParams {
    pub session: String,
    pub generation: Option<u64>,
    pub cols: u16,
    pub rows: u16,
    pub cell: Option<Fields>,
}

method! {
    SessionResize {
        name: "session.resize",
        summary: "Resize a session's terminal grid.",
        owner: HOST,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::Anyone,
        scope: ScopeRule::Workspace,
        reply: Reply::Ready,
        params: ResizeParams,
        result: Fields,
        examples: [({"session": "123e4567-e89b-42d3-a456-426614174000", "cols": 120, "rows": 40} => {"cols": 120, "rows": 40})],
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AppearanceParams {
    pub session: String,
    pub generation: Option<u64>,
    pub appearance: Fields,
}

method! {
    SessionAppearance {
        name: "session.appearance",
        summary: "Set the colors a session's terminal reports.",
        owner: HOST,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::Anyone,
        scope: ScopeRule::Workspace,
        reply: Reply::Ready,
        params: AppearanceParams,
        result: Fields,
        examples: [({"session": "123e4567-e89b-42d3-a456-426614174000", "appearance": {}} => {})],
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WorkspaceParams {
    pub session: String,
    pub workspace: String,
}

method! {
    SessionWorkspace {
        name: "session.workspace",
        summary: "Record the workspace a session belongs to.",
        owner: HOST,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::PaneflowOnly,
        reply: Reply::Ready,
        params: WorkspaceParams,
        result: Fields,
        examples: [({"session": "123e4567-e89b-42d3-a456-426614174000", "workspace": "223e4567-e89b-42d3-a456-426614174000"} => {})],
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TextParams {
    pub session: String,
    #[schemars(description = "Byte offset to resume from", extend("default" = 0))]
    pub from: Option<u64>,
}

method! {
    SessionText {
        name: "session.text",
        summary: "Read a session's text, one frame at a time.",
        owner: HOST,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::Anyone,
        reply: Reply::Ready,
        params: TextParams,
        result: Fields,
        examples: [({"session": "123e4567-e89b-42d3-a456-426614174000"} => {"text": "$ ", "live": true, "next_offset": null})],
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StreamParams {
    pub session: String,
    pub generation: Option<u64>,
    pub from: Option<u64>,
}

method! {
    SessionAttach {
        name: "session.attach",
        summary: "Stream a session's checkpoint then its output to a client that decodes the engine.",
        owner: HOST,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::Anyone,
        reply: Reply::Stream,
        params: StreamParams,
        result: Fields,
        examples: [({"session": "123e4567-e89b-42d3-a456-426614174000"} => {"checkpoint": {}})],
    }
}

method! {
    SessionOutput {
        name: "session.output",
        summary: "Stream a session's output from an offset to a client that decodes the engine.",
        owner: HOST,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::Anyone,
        reply: Reply::Stream,
        params: StreamParams,
        result: Fields,
        examples: [({"session": "123e4567-e89b-42d3-a456-426614174000", "from": 0} => {"offset": 0})],
    }
}

method! {
    ApprovalFollow {
        name: "approval.follow",
        summary: "Stream the write requests that wait for a human decision.",
        owner: HOST,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::PaneflowOnly,
        reply: Reply::Stream,
        params: NoParams,
        result: Fields,
        examples: [({} => {"pending": []})],
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DecideParams {
    #[schemars(description = "The pending request's id")]
    pub id: u64,
    #[schemars(description = "allow or deny")]
    pub decision: String,
}

method! {
    ApprovalDecide {
        name: "approval.decide",
        summary: "Allow or deny a write request that waits for a human decision.",
        owner: HOST,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::PaneflowOnly,
        reply: Reply::Ready,
        params: DecideParams,
        result: Fields,
        examples: [({"id": 7, "decision": "allow"} => {"decided": true, "id": 7})],
    }
}

method! {
    AgentEvent {
        name: "agent.event",
        summary: "Report an agent hook event from inside a pane.",
        owner: HOST,
        fallback: NO_FALLBACK,
        gate: Gate::Open,
        audience: Audience::Anyone,
        reply: Reply::Ready,
        params: Fields,
        result: Fields,
        examples: [({"kind": "ai.stop", "session": "123e4567-e89b-42d3-a456-426614174000"} => {"accepted": true})],
    }
}
