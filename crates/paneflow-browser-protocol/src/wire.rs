use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

use crate::{
    Availability, BrowserError, BrowserId, BrowserPresentation, BrowserSession, Document,
    OperationId, Owner, ProfileId, MAX_ZOOM_PERCENT, MIN_ZOOM_PERCENT,
};

pub const CONTRACT_VERSION: u32 = 3;
pub const MAX_MESSAGE_BYTES: usize = 256 * 1024;
pub const MAX_FRAME_PLANES: usize = 4;
pub const POOL_BUFFERS: u8 = 3;
pub const MAX_PENDING_FRAMES: usize = 2;
pub const RETIRE_DEADLINE_MS: u64 = 1_000;

pub const MODIFIER_SHIFT: u32 = 1 << 1;
pub const MODIFIER_CONTROL: u32 = 1 << 2;
pub const MODIFIER_ALT: u32 = 1 << 3;
pub const MODIFIER_LEFT_MOUSE: u32 = 1 << 4;
pub const MODIFIER_MIDDLE_MOUSE: u32 = 1 << 5;
pub const MODIFIER_RIGHT_MOUSE: u32 = 1 << 6;
pub const MODIFIER_COMMAND: u32 = 1 << 7;
const KNOWN_MODIFIERS: u32 = MODIFIER_SHIFT
    | MODIFIER_CONTROL
    | MODIFIER_ALT
    | MODIFIER_LEFT_MOUSE
    | MODIFIER_MIDDLE_MOUSE
    | MODIFIER_RIGHT_MOUSE
    | MODIFIER_COMMAND;
const MAX_COORDINATE: i32 = 32_768;
const MAX_IME_TEXT_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub version: u32,
    pub operation: OperationId,
    pub command: Command,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Capabilities,
    Create {
        owner: Owner,
        browser: BrowserId,
        profile: ProfileId,
        url: String,
        title: String,
    },
    CreateDevTools {
        document: Document,
        browser: BrowserId,
    },
    State {
        document: Document,
    },
    Start {
        document: Document,
    },
    Sleep {
        document: Document,
    },
    Navigate {
        document: Document,
        url: String,
    },
    AgentNavigate {
        document: Document,
        url: String,
    },
    Screenshot {
        document: Document,
    },
    Present {
        document: Document,
        presentation: BrowserPresentation,
    },
    Input {
        document: Document,
        input: InputEvent,
    },
    History {
        document: Document,
        direction: HistoryDirection,
    },
    Reload {
        document: Document,
        ignore_cache: bool,
    },
    Stop {
        document: Document,
    },
    Find {
        document: Document,
        text: String,
        forward: bool,
        find_next: bool,
    },
    StopFinding {
        document: Document,
    },
    Zoom {
        document: Document,
        percent: u32,
    },
    Mute {
        document: Document,
        muted: bool,
    },
    Close {
        document: Document,
    },
    BeginOperation {
        document: Document,
        mutation: bool,
        text: String,
    },
    CompleteOperation {
        document: Document,
        pending: OperationId,
    },
    Frame {
        document: Document,
        contract_version: u32,
        pool_generation: u64,
        buffer: u8,
        sequence: u64,
    },
    ReleaseFrame {
        document: Document,
        pool_generation: u64,
        buffer: u8,
        sequence: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryDirection {
    Back,
    Forward,
}

pub fn zoom_percent_is_valid(percent: u32) -> bool {
    (MIN_ZOOM_PERCENT..=MAX_ZOOM_PERCENT).contains(&percent)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyKind {
    RawDown,
    Down,
    Up,
    Char,
}

pub const MAX_FIND_TEXT_BYTES: usize = 8 * 1024;

pub fn find_text_is_valid(text: &str) -> bool {
    text.len() <= MAX_FIND_TEXT_BYTES && !text.contains('\0')
}

pub const MAX_CLIPBOARD_TEXT_BYTES: usize = 32 * 1024;

pub fn clipboard_text_is_valid(text: &str) -> bool {
    text.len() <= MAX_CLIPBOARD_TEXT_BYTES && !text.contains('\0')
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum EditAction {
    Copy,
    Cut,
    Paste { text: String },
    SelectAll,
    Undo,
    Redo,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputEvent {
    DropFiles {
        paths: Vec<String>,
        x: i32,
        y: i32,
    },
    TransferResponse {
        request: u64,
        paths: Vec<String>,
    },
    CancelDownload {
        request: u64,
    },
    MouseMove {
        x: i32,
        y: i32,
        modifiers: u32,
    },
    MouseLeave {
        x: i32,
        y: i32,
        modifiers: u32,
    },
    MouseButton {
        x: i32,
        y: i32,
        button: MouseButton,
        down: bool,
        clicks: u8,
        modifiers: u32,
    },
    MouseWheel {
        x: i32,
        y: i32,
        delta_x: i32,
        delta_y: i32,
        modifiers: u32,
    },
    Key {
        kind: KeyKind,
        key_code: i32,
        native_key_code: i32,
        character: u16,
        unmodified_character: u16,
        modifiers: u32,
    },
    Focus {
        focused: bool,
    },
    ImeComposition {
        text: String,
        cursor: u32,
        #[serde(default)]
        selection_start: Option<u32>,
        #[serde(default)]
        replacement: Option<[u32; 2]>,
    },
    ImeCommit {
        text: String,
        #[serde(default)]
        replacement: Option<[u32; 2]>,
    },
    ImeCancel,
    ImeFinish,
    Edit {
        action: EditAction,
        request: u64,
    },
    ContextMenu {
        request: u64,
        command: Option<i32>,
    },
    WebResponse {
        request: u64,
        accept: bool,
        text: String,
    },
    CaptureLost,
    ClipboardWritten {
        request: u64,
    },
}

impl InputEvent {
    pub fn is_valid(&self) -> bool {
        let coordinate = |value: i32| (-MAX_COORDINATE..=MAX_COORDINATE).contains(&value);
        let known = |modifiers: u32| modifiers & !KNOWN_MODIFIERS == 0;
        match self {
            Self::TransferResponse { request, paths } => {
                *request != 0
                    && paths.len() <= 64
                    && paths
                        .iter()
                        .all(|path| !path.is_empty() && path.len() <= 4096 && !path.contains('\0'))
                    && paths.iter().map(String::len).sum::<usize>() <= 128 * 1024
            }
            Self::DropFiles { paths, x, y } => {
                (0..=MAX_COORDINATE).contains(x)
                    && (0..=MAX_COORDINATE).contains(y)
                    && !paths.is_empty()
                    && paths.len() <= 64
                    && paths
                        .iter()
                        .all(|path| !path.is_empty() && path.len() <= 4096 && !path.contains('\0'))
                    && paths.iter().map(String::len).sum::<usize>() <= 128 * 1024
            }
            Self::CancelDownload { request } => *request != 0,
            Self::MouseMove { x, y, modifiers } | Self::MouseLeave { x, y, modifiers } => {
                coordinate(*x) && coordinate(*y) && known(*modifiers)
            }
            Self::MouseButton {
                x,
                y,
                clicks,
                modifiers,
                ..
            } => coordinate(*x) && coordinate(*y) && (1..=3).contains(clicks) && known(*modifiers),
            Self::MouseWheel {
                x,
                y,
                delta_x,
                delta_y,
                modifiers,
            } => {
                coordinate(*x)
                    && coordinate(*y)
                    && coordinate(*delta_x)
                    && coordinate(*delta_y)
                    && known(*modifiers)
            }
            Self::Key { modifiers, .. } => known(*modifiers),
            Self::Focus { .. } | Self::ImeCancel | Self::ImeFinish | Self::CaptureLost => true,
            Self::WebResponse { request, text, .. } => {
                *request != 0 && text.len() <= 8192 && !text.contains('\0')
            }
            Self::ClipboardWritten { request } => *request != 0,
            Self::Edit { action, request } => {
                *request != 0
                    && match action {
                        EditAction::Paste { text } => clipboard_text_is_valid(text),
                        _ => true,
                    }
            }
            Self::ContextMenu { request, command } => {
                *request != 0 && command.is_none_or(|id| id >= 0)
            }
            Self::ImeComposition {
                text,
                cursor,
                selection_start,
                replacement,
            } => {
                replacement.is_none_or(|[start, end]| start <= end && end < u32::MAX)
                    && text.len() <= MAX_IME_TEXT_BYTES
                    && !text.chars().any(char::is_control)
                    && (*cursor as usize) <= text.encode_utf16().count()
                    && selection_start
                        .is_none_or(|start| start as usize <= text.encode_utf16().count())
            }
            Self::ImeCommit { text, replacement } => {
                replacement.is_none_or(|[start, end]| start <= end && end < u32::MAX)
                    && text.len() <= MAX_IME_TEXT_BYTES
                    && !text.chars().any(char::is_control)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameFormat {
    Bgra8,
    Rgba8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaneLayout {
    pub stride: u32,
    pub offset: u64,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BufferLayout {
    pub slot: u8,
    pub planes: Vec<PlaneLayout>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirtyRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameFailure {
    UnsupportedModifier,
    WrongDevice,
    InvalidHandle,
    RetireTimeout,
    CopyFailed,
    UnsupportedFormat,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum FrameMessage {
    PoolCreated {
        document: Document,
        pool_generation: u64,
        width: u32,
        height: u32,
        format: FrameFormat,
        modifier: u64,
        buffers: Vec<BufferLayout>,
    },
    Frame {
        document: Document,
        pool_generation: u64,
        buffer: u8,
        sequence: u64,
        callback_ns: u64,
        ready_ns: u64,
        capture_timestamp_us: u64,
        capture_counter: Option<u64>,
        dirty: Option<DirtyRect>,
    },
    PoolRetired {
        document: Document,
        pool_generation: u64,
    },
    Failed {
        document: Document,
        reason: FrameFailure,
        detail: String,
    },
}

impl FrameMessage {
    pub fn is_valid(&self) -> bool {
        match self {
            Self::PoolCreated {
                width,
                height,
                buffers,
                ..
            } => {
                (1..=16384).contains(width)
                    && (1..=16384).contains(height)
                    && buffers.len() == usize::from(POOL_BUFFERS)
                    && buffers.iter().enumerate().all(|(index, buffer)| {
                        usize::from(buffer.slot) == index
                            && (1..=MAX_FRAME_PLANES).contains(&buffer.planes.len())
                            && buffer
                                .planes
                                .iter()
                                .all(|plane| plane.stride > 0 && plane.size > 0)
                    })
            }
            Self::Frame { buffer, .. } => *buffer < POOL_BUFFERS,
            Self::PoolRetired { .. } => true,
            Self::Failed { detail, .. } => detail.len() <= 1024,
        }
    }

    pub fn expected_fds(&self) -> usize {
        match self {
            Self::PoolCreated { buffers, .. } => {
                buffers.iter().map(|buffer| buffer.planes.len()).sum()
            }
            _ => 0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum FrameAck {
    PoolRejected {
        document: Document,
        pool_generation: u64,
    },
    PoolReady {
        document: Document,
        pool_generation: u64,
    },
    Release {
        document: Document,
        pool_generation: u64,
        buffer: u8,
        sequence: u64,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Capabilities {
        target: String,
        availability: Availability,
        contract_version: u32,
        terminal_available: bool,
    },
    State {
        session: BrowserSession,
    },
    Closed {
        document: Document,
    },
    Accepted {
        operation: OperationId,
    },
    NavigationStarted {
        session: BrowserSession,
    },
    ScreenshotAccepted,
    Completed {
        operation: OperationId,
    },
    Screenshot {
        mime: String,
        width: u32,
        height: u32,
        data: String,
    },
    InputAccepted,
    NavigationAccepted,
    FrameAccepted,
    FrameReleased,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reply {
    pub version: u32,
    pub operation: OperationId,
    pub result: Result<Event, BrowserError>,
}

pub fn read_message(input: &mut impl Read) -> Result<Option<Envelope>, BrowserError> {
    let mut header = [0_u8; 4];
    match input.read(&mut header[..1]) {
        Ok(0) => return Ok(None),
        Ok(_) => (),
        Err(_) => return Err(BrowserError::InvalidMessage),
    }
    input
        .read_exact(&mut header[1..])
        .map_err(|_| BrowserError::InvalidMessage)?;
    let length = u32::from_be_bytes(header) as usize;
    if length > MAX_MESSAGE_BYTES {
        return Err(BrowserError::TooLarge);
    }
    let mut bytes = vec![0; length];
    input
        .read_exact(&mut bytes)
        .map_err(|_| BrowserError::InvalidMessage)?;
    let message: Envelope =
        serde_json::from_slice(&bytes).map_err(|_| BrowserError::InvalidMessage)?;
    if message.version != CONTRACT_VERSION {
        return Err(BrowserError::IncompatibleVersion);
    }
    Ok(Some(message))
}

pub fn read_value(input: &mut impl Read) -> Result<Option<serde_json::Value>, BrowserError> {
    let mut header = [0_u8; 4];
    match input.read(&mut header[..1]) {
        Ok(0) => return Ok(None),
        Ok(_) => (),
        Err(_) => return Err(BrowserError::InvalidMessage),
    }
    input
        .read_exact(&mut header[1..])
        .map_err(|_| BrowserError::InvalidMessage)?;
    let length = u32::from_be_bytes(header) as usize;
    if length > MAX_MESSAGE_BYTES {
        return Err(BrowserError::TooLarge);
    }
    let mut bytes = vec![0; length];
    input
        .read_exact(&mut bytes)
        .map_err(|_| BrowserError::InvalidMessage)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| BrowserError::InvalidMessage)
}

struct BoundedMessage(Vec<u8>);

impl Write for BoundedMessage {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_MESSAGE_BYTES - self.0.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "browser message exceeds 256 KiB",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub fn encode_message(value: &impl Serialize) -> io::Result<Vec<u8>> {
    let mut buffer = BoundedMessage(Vec::new());
    serde_json::to_writer(&mut buffer, value)?;
    let body = buffer.0;
    let mut bytes = Vec::with_capacity(body.len() + 4);
    bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&body);
    Ok(bytes)
}

pub fn write_message(output: &mut impl Write, value: &impl Serialize) -> io::Result<()> {
    let bytes = encode_message(value)?;
    output.write_all(&bytes)?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_accepts_multiline_text_and_rejects_nul_and_oversize() {
        assert!(clipboard_text_is_valid("first\tcolumn\nsecond\r\n"));
        assert!(!clipboard_text_is_valid("before\0after"));
        assert!(clipboard_text_is_valid(
            &"a".repeat(MAX_CLIPBOARD_TEXT_BYTES)
        ));
        assert!(!clipboard_text_is_valid(
            &"a".repeat(MAX_CLIPBOARD_TEXT_BYTES + 1)
        ));
    }

    #[test]
    fn editing_requires_request_identity_and_valid_paste() {
        assert!(!InputEvent::Edit {
            action: EditAction::Copy,
            request: 0
        }
        .is_valid());
        assert!(!InputEvent::Edit {
            action: EditAction::Paste { text: "\0".into() },
            request: 1
        }
        .is_valid());
        assert!(InputEvent::Edit {
            action: EditAction::Paste {
                text: "line one\nline two".into()
            },
            request: 1
        }
        .is_valid());
        assert!(!InputEvent::ContextMenu {
            request: 1,
            command: Some(-1)
        }
        .is_valid());
        assert!(InputEvent::ContextMenu {
            request: 1,
            command: None
        }
        .is_valid());
    }
}
