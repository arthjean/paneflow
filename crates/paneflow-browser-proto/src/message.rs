use serde::{Deserialize, Serialize};

use crate::handshake::{Hello, Refusal, Welcome};
use crate::navigation::NavigationRefusal;
use crate::profile::ProfileName;
use crate::site::SiteSecurity;
use crate::surface::{Cursor, DialogKind, FileMode, HistoryStep, Input, Rect, View};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TabId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(pub u64);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WindowMessage {
    Hello(Hello),
    Request { id: RequestId, command: Command },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    OpenTab {
        tab: TabId,
        profile: ProfileName,
        url: String,
        guarded: bool,
    },
    Guard {
        tab: TabId,
        guarded: bool,
    },
    Zoom {
        tab: TabId,
        percent: u16,
    },
    Security {
        tab: TabId,
    },
    Inspect {
        tab: TabId,
        inspector: TabId,
    },
    CloseTab {
        tab: TabId,
    },
    Navigate {
        tab: TabId,
        url: String,
    },
    Reload {
        tab: TabId,
    },
    DevTools {
        tab: TabId,
        method: String,
        params: serde_json::Value,
    },
    DecideOrigin {
        tab: TabId,
        origin: String,
        decision: OriginDecision,
    },
    Present {
        tab: TabId,
        view: Option<View>,
    },
    ReleaseFrame {
        tab: TabId,
        generation: u64,
        slot: u32,
    },
    Input {
        tab: TabId,
        input: Input,
    },
    History {
        tab: TabId,
        step: HistoryStep,
    },
    Stop {
        tab: TabId,
    },
    AnswerDialog {
        tab: TabId,
        dialog: u64,
        accept: bool,
        text: Option<String>,
    },
    AnswerFiles {
        tab: TabId,
        request: u64,
        paths: Option<Vec<String>>,
    },
    Shutdown,
}

impl Command {
    pub fn tab(&self) -> Option<TabId> {
        match self {
            Self::OpenTab { tab, .. }
            | Self::CloseTab { tab }
            | Self::Guard { tab, .. }
            | Self::Zoom { tab, .. }
            | Self::Security { tab }
            | Self::Inspect { tab, .. }
            | Self::Navigate { tab, .. }
            | Self::Reload { tab }
            | Self::DevTools { tab, .. }
            | Self::DecideOrigin { tab, .. }
            | Self::Present { tab, .. }
            | Self::ReleaseFrame { tab, .. }
            | Self::Input { tab, .. }
            | Self::History { tab, .. }
            | Self::Stop { tab }
            | Self::AnswerDialog { tab, .. }
            | Self::AnswerFiles { tab, .. } => Some(*tab),
            Self::Shutdown => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginDecision {
    AllowOnce,
    AllowAlways,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostMessage {
    Welcome(Welcome),
    Refused(Refusal),
    Reply { id: RequestId, outcome: Outcome },
    Event(Event),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    Done,
    DevTools { result: serde_json::Value },
    Security { security: SiteSecurity },
    Failed { error: CommandError },
}

impl Outcome {
    pub fn into_result(self) -> Result<Reply, CommandError> {
        match self {
            Self::Done => Ok(Reply::Done),
            Self::DevTools { result } => Ok(Reply::DevTools(result)),
            Self::Security { security } => Ok(Reply::Security(security)),
            Self::Failed { error } => Err(error),
        }
    }
}

impl From<Result<Reply, CommandError>> for Outcome {
    fn from(result: Result<Reply, CommandError>) -> Self {
        match result {
            Ok(Reply::Done) => Self::Done,
            Ok(Reply::DevTools(result)) => Self::DevTools { result },
            Ok(Reply::Security(security)) => Self::Security { security },
            Err(error) => Self::Failed { error },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Done,
    DevTools(serde_json::Value),
    Security(SiteSecurity),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum CommandError {
    #[error("tab {} is closed", tab.0)]
    TabClosed { tab: TabId },
    #[error("tab {} already exists", tab.0)]
    TabExists { tab: TabId },
    #[error("tab {} has crashed: {reason}", tab.0)]
    TabCrashed { tab: TabId, reason: String },
    #[error("the DevTools method {method} did not answer within {after_ms} ms")]
    Timeout { method: String, after_ms: u64 },
    #[error("the DevTools method {method} failed: {message}")]
    DevTools { method: String, message: String },
    #[error("navigation refused: {refusal}")]
    NavigationRefused { refusal: NavigationRefusal },
    #[error("no navigation to {origin} is waiting for a decision on tab {}", tab.0)]
    NoPendingDecision { tab: TabId, origin: String },
    #[error("the browser profile could not be opened: {reason}")]
    ProfileUnavailable { reason: String },
    #[error("the browser engine is unavailable: {reason}")]
    HostUnavailable { reason: String },
    #[error("the browser engine refused the command: {reason}")]
    Rejected { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleLevel {
    Debug,
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    Camera,
    Microphone,
    Geolocation,
    Notifications,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitReason {
    Idle,
    Shutdown,
    WindowGone,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    LoadStarted {
        tab: TabId,
        url: String,
    },
    Loaded {
        tab: TabId,
        url: String,
        http_status: i32,
    },
    LoadFailed {
        tab: TabId,
        url: String,
        error_code: i32,
        error_text: String,
    },
    Console {
        tab: TabId,
        level: ConsoleLevel,
        text: String,
        source: String,
        line: i32,
    },
    DevTools {
        tab: TabId,
        method: String,
        params: serde_json::Value,
    },
    OriginDecisionRequested {
        tab: TabId,
        origin: String,
        url: String,
    },
    NavigationRefused {
        tab: TabId,
        url: String,
        refusal: NavigationRefusal,
    },
    PermissionDenied {
        tab: TabId,
        origin: String,
        permissions: Vec<Permission>,
    },
    DownloadCancelled {
        tab: TabId,
        url: String,
    },
    Crashed {
        tab: TabId,
        reason: String,
    },
    Closed {
        tab: TabId,
    },
    Surface {
        tab: TabId,
        generation: u64,
        width: u32,
        height: u32,
        stride: u32,
        slots: u32,
    },
    Frame(FrameDescriptor),
    PresentationFailed {
        tab: TabId,
        reason: String,
    },
    Cursor {
        tab: TabId,
        cursor: Cursor,
    },
    TitleChanged {
        tab: TabId,
        title: String,
    },
    AddressChanged {
        tab: TabId,
        url: String,
    },
    LoadingState {
        tab: TabId,
        loading: bool,
        can_go_back: bool,
        can_go_forward: bool,
    },
    LoadProgress {
        tab: TabId,
        progress: f64,
    },
    DialogOpened {
        tab: TabId,
        dialog: u64,
        kind: DialogKind,
        origin: String,
        message: String,
        default_text: String,
    },
    FileChooserOpened {
        tab: TabId,
        request: u64,
        mode: FileMode,
        title: String,
        accept: Vec<String>,
    },
    NewWindowRequested {
        tab: TabId,
        url: String,
        user_gesture: bool,
    },
    ImeCaret {
        tab: TabId,
        caret: Rect,
    },
    Exiting {
        reason: ExitReason,
    },
}

impl Event {
    pub fn tab(&self) -> Option<TabId> {
        match self {
            Self::LoadStarted { tab, .. }
            | Self::Loaded { tab, .. }
            | Self::LoadFailed { tab, .. }
            | Self::Console { tab, .. }
            | Self::DevTools { tab, .. }
            | Self::OriginDecisionRequested { tab, .. }
            | Self::NavigationRefused { tab, .. }
            | Self::PermissionDenied { tab, .. }
            | Self::DownloadCancelled { tab, .. }
            | Self::Crashed { tab, .. }
            | Self::Closed { tab }
            | Self::Surface { tab, .. }
            | Self::PresentationFailed { tab, .. }
            | Self::Cursor { tab, .. }
            | Self::TitleChanged { tab, .. }
            | Self::AddressChanged { tab, .. }
            | Self::LoadingState { tab, .. }
            | Self::LoadProgress { tab, .. }
            | Self::DialogOpened { tab, .. }
            | Self::FileChooserOpened { tab, .. }
            | Self::NewWindowRequested { tab, .. }
            | Self::ImeCaret { tab, .. } => Some(*tab),
            Self::Frame(frame) => Some(frame.tab()),
            Self::Exiting { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelFormat {
    Bgra8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "path", rename_all = "snake_case")]
pub enum FrameDescriptor {
    SharedMemory {
        tab: TabId,
        slot: u32,
        generation: u64,
        width: u32,
        height: u32,
        stride: u32,
        format: PixelFormat,
    },
    GpuTexture {
        tab: TabId,
        slot: u32,
        generation: u64,
        width: u32,
        height: u32,
        format: PixelFormat,
        modifier: u64,
    },
}

impl FrameDescriptor {
    pub fn tab(&self) -> TabId {
        match self {
            Self::SharedMemory { tab, .. } | Self::GpuTexture { tab, .. } => *tab,
        }
    }
}
