use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{NO_FALLBACK, NoParams, WINDOW};
use crate::descriptor::{Audience, Gate, Reply, ScopeRule};

pub const BROWSER_REFUSAL: &str = "browser.* needs the browser right: ask for it with grant.request {\"rights\": [\"browser\"]}, or set ipc.scripting in paneflow.json";

pub const METHOD_PREFIX: &str = "browser.";

pub const ERR_BROWSER_FAILED: i64 = -32080;
pub const ERR_NOT_OWNER: i64 = -32081;
pub const ERR_STALE_REFERENCE: i64 = -32082;
pub const ERR_TIMED_OUT: i64 = -32083;
pub const ERR_DENIED: i64 = -32084;
pub const ERR_NOT_INSTALLED: i64 = -32085;
pub const ERR_UNKNOWN_TAB: i64 = -32086;

pub const REPLY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(
    super::prompt::AGENT_PROMPT_DEADLINE_SECS + super::config::SLOW_REPLY_DEADLINE.as_secs() * 3,
);

pub const CLIENT_REPLY_DEADLINE: std::time::Duration =
    std::time::Duration::from_secs(REPLY_DEADLINE.as_secs() + 15);

pub const SCREENSHOT_REPLY_BYTES: u64 = 24 * 1024 * 1024;
pub const REPLY_BYTES: u64 = 4 * 1024 * 1024;

pub fn reply_bytes(method: &str) -> Option<u64> {
    match method {
        "browser.screenshot" => Some(SCREENSHOT_REPLY_BYTES),
        method if is_browser_method(method) => Some(REPLY_BYTES),
        _ => None,
    }
}

pub fn is_browser_method(method: &str) -> bool {
    method.starts_with(METHOD_PREFIX)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TabOwner {
    Agent {
        session: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
    },
    Human,
    Unclaimed,
}

impl TabOwner {
    pub fn describe(&self) -> String {
        match self {
            Self::Agent {
                agent: Some(agent),
                session,
            } => format!("the agent {agent} (session {session})"),
            Self::Agent {
                agent: None,
                session,
            } => format!("the agent session {session}"),
            Self::Human => "the human".to_string(),
            Self::Unclaimed => "nobody".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TabState {
    Starting,
    Loading,
    Loaded,
    AwaitingDecision,
    Refused,
    Crashed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Presentation {
    None,
    Cpu,
    Gpu,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserTab {
    pub tab: u64,
    #[schemars(description = "The durable id of the workspace whose browser profile the tab uses")]
    pub workspace: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub state: TabState,
    pub owner: TabOwner,
    #[schemars(description = "True when the caller owns the tab and may drive it")]
    pub drivable: bool,
    #[schemars(description = "none while the tab is shown in no pane or dock")]
    pub presentation: Presentation,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StatusParams {
    #[schemars(description = "workspace by default; all needs orchestration")]
    pub scope: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserStatus {
    pub running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_pid: Option<u32>,
    pub installed: bool,
    #[schemars(description = "The browser host binary this window starts")]
    pub binary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "The engine the running host reports, such as CEF and Chromium versions"
    )]
    pub runtime_version: Option<String>,
    #[schemars(
        description = "enabled when the running host starts Chromium with its sandbox, unknown when no host runs"
    )]
    pub sandbox: String,
    pub tabs: Vec<BrowserTab>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstallResult {
    pub installed: bool,
    pub binary: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OpenParams {
    #[schemars(description = "An http or https URL of at most 2048 characters")]
    pub url: String,
    #[schemars(
        description = "The durable id of the workspace whose profile the tab uses, for a caller outside every pane; a pane always opens in its own workspace"
    )]
    pub workspace: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TabParams {
    pub tab: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NavigateParams {
    pub tab: u64,
    #[schemars(description = "An http or https URL of at most 2048 characters")]
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Navigated {
    pub tab: u64,
    #[schemars(description = "The URL the tab shows once the call ends")]
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub state: TabState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<i32>,
    pub owner: TabOwner,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Closed {
    pub tab: u64,
    #[schemars(description = "The URL the tab showed when it closed")]
    pub url: String,
    pub closed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TabList {
    pub tabs: Vec<BrowserTab>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConsoleEntry {
    pub seq: u64,
    #[schemars(description = "debug, info, warning or error")]
    pub level: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NetworkEntry {
    pub seq: u64,
    pub method: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(
        description = "Request headers; the values of Authorization, Cookie and Proxy-Authorization are left out"
    )]
    pub request_headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DialogKind {
    Alert,
    Confirm,
    Prompt,
    Beforeunload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Dialog {
    pub kind: DialogKind,
    pub message: String,
    pub dismissed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub tab: u64,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[schemars(
        description = "True while the page is still loading; the tree is what exists so far"
    )]
    pub loading: bool,
    #[schemars(
        description = "The navigation generation the references belong to; a reference from an older generation is stale"
    )]
    pub generation: u64,
    #[schemars(
        description = "The accessibility tree, one node per line, indented two spaces per depth: role \"name\" [ref=eN]"
    )]
    pub tree: String,
    #[schemars(description = "Bytes of tree left out past the 32 KiB bound")]
    pub dropped_bytes: u64,
    #[schemars(description = "Console entries recorded since the previous snapshot")]
    pub console: Vec<ConsoleEntry>,
    #[schemars(description = "Network entries recorded since the previous snapshot")]
    pub network: Vec<NetworkEntry>,
    #[schemars(
        description = "Entries recorded since the previous snapshot that the buffers no longer hold"
    )]
    pub dropped_entries: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dialogs: Vec<Dialog>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Click,
    Type,
    Press,
    Hover,
    Select,
    Scroll,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActParams {
    pub tab: u64,
    #[serde(rename = "ref")]
    #[schemars(description = "An element reference eN from the latest snapshot")]
    pub reference: String,
    pub action: Action,
    #[schemars(description = "The text to type, for type")]
    pub text: Option<String>,
    #[schemars(description = "A key name such as Enter, Tab, Escape or ArrowDown, for press")]
    pub key: Option<String>,
    #[schemars(description = "The value or visible label of the option to choose, for select")]
    pub value: Option<String>,
    #[schemars(description = "Pixels to scroll down, negative to scroll up, for scroll", extend("default" = 400))]
    pub delta_y: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Acted {
    pub tab: u64,
    pub action: Action,
    #[schemars(description = "True when the action started a navigation of the tab")]
    pub navigated: bool,
    pub url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(description = "JavaScript dialogs the action opened; each one was dismissed")]
    pub dialogs: Vec<Dialog>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WaitParams {
    pub tab: u64,
    #[schemars(description = "Wait until the page shows this text")]
    pub text: Option<String>,
    #[serde(rename = "ref")]
    #[schemars(description = "Wait until this reference of the latest snapshot is visible")]
    pub reference: Option<String>,
    #[schemars(description = "Wait until no request has been in flight for 500 ms")]
    pub network_idle: Option<bool>,
    #[schemars(description = "Milliseconds to wait, at most 60000", extend("default" = 10000))]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Waited {
    pub tab: u64,
    pub condition: String,
    pub waited_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HistoryParams {
    pub tab: u64,
    #[schemars(
        description = "Return the entries after this cursor; from the oldest kept when absent"
    )]
    pub cursor: Option<u64>,
    #[schemars(description = "Entries to return, at most 1000", extend("default" = 200))]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConsoleHistory {
    pub tab: u64,
    pub entries: Vec<ConsoleEntry>,
    #[schemars(description = "Pass it back to read the entries after these")]
    pub cursor: u64,
    #[schemars(description = "Entries after the given cursor that the buffer no longer holds")]
    pub dropped: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NetworkHistory {
    pub tab: u64,
    pub entries: Vec<NetworkEntry>,
    pub cursor: u64,
    pub dropped: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Screenshot {
    pub tab: u64,
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub device_scale_factor: f64,
    #[schemars(description = "The PNG, base64 encoded")]
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EvaluateParams {
    pub tab: u64,
    #[schemars(description = "A JavaScript expression evaluated in the page, at most 65536 bytes")]
    pub script: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Evaluated {
    pub tab: u64,
    pub origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "The result as JSON text, bounded at 32 KiB")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(description = "The message of the exception the script threw")]
    pub exception: Option<String>,
    pub dropped_bytes: u64,
}

const GATE: Gate = Gate::Browser {
    refusal: BROWSER_REFUSAL,
};

method! {
    Status {
        name: "browser.status",
        summary: "Report whether a browser host runs, its engine, its sandbox and the tabs the caller may see.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: GATE,
        audience: Audience::Anyone,
        scope: ScopeRule::WorkspaceUnlessAll,
        reply: Reply::Deferred,
        params: StatusParams,
        result: BrowserStatus,
        examples: [({} => {
            "running": true,
            "host_pid": 4242,
            "installed": true,
            "binary": "/home/u/.paneflow/browser/runtime/current/paneflow-browser-host",
            "runtime_version": "CEF 156.0.31, Chromium 156.0.8078.100",
            "sandbox": "enabled",
            "tabs": [{
                "tab": 1,
                "workspace": "0b6d6713-5706-4af6-8061-06e40f757cb9",
                "url": "http://127.0.0.1:5173/",
                "title": "Shop",
                "state": "loaded",
                "owner": {"kind": "agent", "session": "a9e52660-0000-4000-8000-000000000001"},
                "drivable": true,
                "presentation": "none"
            }]
        })],
    }
}

method! {
    Install {
        name: "browser.install",
        summary: "Report whether the browser engine is installed where this window looks for it.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: GATE,
        audience: Audience::Anyone,
        reply: Reply::Deferred,
        params: NoParams,
        result: InstallResult,
        examples: [({} => {
            "installed": true,
            "binary": "/home/u/dev/paneflow/crates/paneflow-browser-host/target/debug/paneflow-browser-host",
            "message": "The browser engine is installed."
        })],
    }
}

method! {
    Open {
        name: "browser.open",
        summary: "Open a tab owned by the caller on an http or https URL, in its workspace's profile, and wait for the page.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: GATE,
        audience: Audience::Anyone,
        scope: ScopeRule::Workspace,
        reply: Reply::Deferred,
        params: OpenParams,
        result: Navigated,
        examples: [({"url": "http://127.0.0.1:5173/"} => {
            "tab": 1,
            "url": "http://127.0.0.1:5173/",
            "title": "Shop",
            "state": "loaded",
            "http_status": 200,
            "owner": {"kind": "agent", "session": "a9e52660-0000-4000-8000-000000000001"}
        })],
    }
}

method! {
    List {
        name: "browser.list",
        summary: "List the tabs of the caller's workspace with their owner; only the caller's own tabs are drivable.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: GATE,
        audience: Audience::Anyone,
        scope: ScopeRule::WorkspaceUnlessAll,
        reply: Reply::Deferred,
        params: StatusParams,
        result: TabList,
        examples: [({} => {"tabs": []})],
    }
}

method! {
    Claim {
        name: "browser.claim",
        summary: "Take ownership of an unclaimed tab.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: GATE,
        audience: Audience::Anyone,
        scope: ScopeRule::Workspace,
        reply: Reply::Deferred,
        params: TabParams,
        result: BrowserTab,
        examples: [({"tab": 1} => {
            "tab": 1,
            "workspace": "0b6d6713-5706-4af6-8061-06e40f757cb9",
            "url": "http://127.0.0.1:5173/",
            "state": "loaded",
            "owner": {"kind": "agent", "session": "a9e52660-0000-4000-8000-000000000001"},
            "drivable": true,
            "presentation": "none"
        })],
    }
}

method! {
    Close {
        name: "browser.close",
        summary: "Close a tab the caller owns.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: GATE,
        audience: Audience::Anyone,
        scope: ScopeRule::Workspace,
        reply: Reply::Deferred,
        params: TabParams,
        result: Closed,
        examples: [({"tab": 1} => {"tab": 1, "url": "http://127.0.0.1:5173/", "closed": true})],
    }
}

method! {
    Navigate {
        name: "browser.navigate",
        summary: "Load a URL in a tab the caller owns and wait for the page.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: GATE,
        audience: Audience::Anyone,
        scope: ScopeRule::Workspace,
        reply: Reply::Deferred,
        params: NavigateParams,
        result: Navigated,
        examples: [({"tab": 1, "url": "http://127.0.0.1:5173/cart"} => {
            "tab": 1,
            "url": "http://127.0.0.1:5173/cart",
            "state": "loaded",
            "http_status": 200,
            "owner": {"kind": "agent", "session": "a9e52660-0000-4000-8000-000000000001"}
        })],
    }
}

method! {
    Reload {
        name: "browser.reload",
        summary: "Reload a tab the caller owns and wait for the page.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: GATE,
        audience: Audience::Anyone,
        scope: ScopeRule::Workspace,
        reply: Reply::Deferred,
        params: TabParams,
        result: Navigated,
        examples: [({"tab": 1} => {
            "tab": 1,
            "url": "http://127.0.0.1:5173/",
            "state": "loaded",
            "http_status": 200,
            "owner": {"kind": "agent", "session": "a9e52660-0000-4000-8000-000000000001"}
        })],
    }
}

method! {
    Back {
        name: "browser.back",
        summary: "Go back one history entry in a tab the caller owns and wait for the page.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: GATE,
        audience: Audience::Anyone,
        scope: ScopeRule::Workspace,
        reply: Reply::Deferred,
        params: TabParams,
        result: Navigated,
        examples: [({"tab": 1} => {
            "tab": 1,
            "url": "http://127.0.0.1:5173/",
            "state": "loaded",
            "owner": {"kind": "agent", "session": "a9e52660-0000-4000-8000-000000000001"}
        })],
    }
}

method! {
    Forward {
        name: "browser.forward",
        summary: "Go forward one history entry in a tab the caller owns and wait for the page.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: GATE,
        audience: Audience::Anyone,
        scope: ScopeRule::Workspace,
        reply: Reply::Deferred,
        params: TabParams,
        result: Navigated,
        examples: [({"tab": 1} => {
            "tab": 1,
            "url": "http://127.0.0.1:5173/cart",
            "state": "loaded",
            "owner": {"kind": "agent", "session": "a9e52660-0000-4000-8000-000000000001"}
        })],
    }
}

method! {
    SnapshotMethod {
        name: "browser.snapshot",
        summary: "Return a tab's URL, title, accessibility tree with element references, and the console and network entries since the previous snapshot.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: GATE,
        audience: Audience::Anyone,
        scope: ScopeRule::Workspace,
        reply: Reply::Deferred,
        params: TabParams,
        result: Snapshot,
        examples: [({"tab": 1} => {
            "tab": 1,
            "url": "http://127.0.0.1:5173/",
            "title": "Shop",
            "loading": false,
            "generation": 1,
            "tree": "RootWebArea \"Shop\"\n  heading \"Cart\"\n  button \"Checkout\" [ref=e1]",
            "dropped_bytes": 0,
            "console": [{"seq": 1, "level": "error", "text": "TypeError: x is undefined", "source": "http://127.0.0.1:5173/", "line": 14}],
            "network": [{"seq": 1, "method": "GET", "url": "http://127.0.0.1:5173/api/prices", "status": 500, "resource_type": "Fetch"}],
            "dropped_entries": 0
        })],
    }
}

method! {
    Act {
        name: "browser.act",
        summary: "Click, type, press a key, hover, select an option or scroll on an element of the latest snapshot.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: GATE,
        audience: Audience::Anyone,
        scope: ScopeRule::Workspace,
        reply: Reply::Deferred,
        params: ActParams,
        result: Acted,
        examples: [({"tab": 1, "ref": "e1", "action": "click"} => {
            "tab": 1,
            "action": "click",
            "navigated": true,
            "url": "http://127.0.0.1:5173/checkout"
        })],
    }
}

method! {
    WaitFor {
        name: "browser.wait_for",
        summary: "Wait until a tab shows a text, shows an element of the latest snapshot, or has no request in flight.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: GATE,
        audience: Audience::Anyone,
        scope: ScopeRule::Workspace,
        reply: Reply::Deferred,
        params: WaitParams,
        result: Waited,
        examples: [({"tab": 1, "text": "Order placed"} => {"tab": 1, "condition": "text \"Order placed\"", "waited_ms": 120})],
    }
}

method! {
    Console {
        name: "browser.console",
        summary: "Page through a tab's console history with a cursor.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: GATE,
        audience: Audience::Anyone,
        scope: ScopeRule::Workspace,
        reply: Reply::Deferred,
        params: HistoryParams,
        result: ConsoleHistory,
        examples: [({"tab": 1} => {
            "tab": 1,
            "entries": [{"seq": 1, "level": "error", "text": "TypeError: x is undefined", "source": "http://127.0.0.1:5173/", "line": 14}],
            "cursor": 1,
            "dropped": 0
        })],
    }
}

method! {
    Network {
        name: "browser.network",
        summary: "Page through a tab's network history with a cursor: method, URL, status, resource type and failure, never a body.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: GATE,
        audience: Audience::Anyone,
        scope: ScopeRule::Workspace,
        reply: Reply::Deferred,
        params: HistoryParams,
        result: NetworkHistory,
        examples: [({"tab": 1} => {
            "tab": 1,
            "entries": [{"seq": 1, "method": "GET", "url": "http://127.0.0.1:5173/api/prices", "status": 500, "resource_type": "Fetch"}],
            "cursor": 1,
            "dropped": 0
        })],
    }
}

method! {
    ScreenshotMethod {
        name: "browser.screenshot",
        summary: "Return a PNG of a tab's viewport at its device scale factor.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: GATE,
        audience: Audience::Anyone,
        scope: ScopeRule::Workspace,
        reply: Reply::Deferred,
        params: TabParams,
        result: Screenshot,
        examples: [({"tab": 1} => {
            "tab": 1,
            "format": "png",
            "width": 1280,
            "height": 800,
            "device_scale_factor": 1.0,
            "data": "iVBORw0KGgo="
        })],
    }
}

method! {
    Evaluate {
        name: "browser.evaluate",
        summary: "Evaluate a JavaScript expression in a tab the caller owns, once the user has approved evaluation for this agent in this workspace.",
        owner: WINDOW,
        fallback: NO_FALLBACK,
        gate: GATE,
        audience: Audience::Anyone,
        scope: ScopeRule::Workspace,
        reply: Reply::Deferred,
        params: EvaluateParams,
        result: Evaluated,
        examples: [({"tab": 1, "script": "document.title"} => {
            "tab": 1,
            "origin": "http://127.0.0.1:5173",
            "value": "\"Shop\"",
            "dropped_bytes": 0
        })],
    }
}

pub fn declarations() -> Vec<crate::descriptor::Descriptor> {
    use crate::descriptor::Descriptor;
    vec![
        Descriptor::of::<Status>(),
        Descriptor::of::<Install>(),
        Descriptor::of::<Open>(),
        Descriptor::of::<List>(),
        Descriptor::of::<Claim>(),
        Descriptor::of::<Close>(),
        Descriptor::of::<Navigate>(),
        Descriptor::of::<Reload>(),
        Descriptor::of::<Back>(),
        Descriptor::of::<Forward>(),
        Descriptor::of::<SnapshotMethod>(),
        Descriptor::of::<Act>(),
        Descriptor::of::<WaitFor>(),
        Descriptor::of::<Console>(),
        Descriptor::of::<Network>(),
        Descriptor::of::<ScreenshotMethod>(),
        Descriptor::of::<Evaluate>(),
    ]
}

pub const METHODS: [&str; 17] = [
    "browser.status",
    "browser.install",
    "browser.open",
    "browser.list",
    "browser.claim",
    "browser.close",
    "browser.navigate",
    "browser.reload",
    "browser.back",
    "browser.forward",
    "browser.snapshot",
    "browser.act",
    "browser.wait_for",
    "browser.console",
    "browser.network",
    "browser.screenshot",
    "browser.evaluate",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::Owner;

    #[test]
    fn every_browser_method_is_one_window_descriptor_behind_the_browser_gate() {
        let catalog = crate::catalog().expect("the catalog builds");
        let declared: Vec<&str> = catalog
            .names()
            .into_iter()
            .filter(|name| is_browser_method(name))
            .collect();
        assert_eq!(declared, METHODS.to_vec());
        for name in METHODS {
            let descriptor = catalog.get(name).expect(name);
            assert_eq!(descriptor.spec.owner, Owner::Window, "{name}");
            assert_eq!(descriptor.spec.gate, GATE, "{name}");
            assert_eq!(descriptor.spec.reply, Reply::Deferred, "{name}");
        }
    }
}
