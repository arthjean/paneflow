#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use paneflow_browser_proto::surface::{
    Cursor, DialogKind, FileMode, HistoryStep, Input, KeyKind, Modifiers, MouseButton, Rect, View,
};
use paneflow_browser_proto::{Certificate, CertificateName, Connection, SiteSecurity};
use paneflow_browser_proto::{
    Command, CommandError, ConsoleLevel, Event, ExitReason, FrameDescriptor, Hello, HostMessage,
    NavigationRefusal, OriginDecision, Outcome, Permission, PixelFormat, ProfileName, Refusal,
    RequestId, TabId, Welcome, WindowMessage, decode, encode, read_message,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;

fn command_kind(command: &Command) -> u8 {
    match command {
        Command::OpenTab { .. } => 0,
        Command::CloseTab { .. } => 1,
        Command::Navigate { .. } => 2,
        Command::Reload { .. } => 3,
        Command::DevTools { .. } => 4,
        Command::DecideOrigin { .. } => 5,
        Command::Shutdown => 6,
        Command::Present { .. } => 7,
        Command::ReleaseFrame { .. } => 8,
        Command::Input { .. } => 9,
        Command::History { .. } => 10,
        Command::Stop { .. } => 11,
        Command::AnswerDialog { .. } => 12,
        Command::AnswerFiles { .. } => 13,
        Command::Guard { .. } => 14,
        Command::Zoom { .. } => 15,
        Command::Security { .. } => 16,
        Command::Inspect { .. } => 17,
    }
}
const COMMAND_KINDS: u8 = 18;

fn event_kind(event: &Event) -> u8 {
    match event {
        Event::LoadStarted { .. } => 0,
        Event::Loaded { .. } => 1,
        Event::LoadFailed { .. } => 2,
        Event::Console { .. } => 3,
        Event::DevTools { .. } => 4,
        Event::OriginDecisionRequested { .. } => 5,
        Event::NavigationRefused { .. } => 6,
        Event::PermissionDenied { .. } => 7,
        Event::DownloadCancelled { .. } => 8,
        Event::Crashed { .. } => 9,
        Event::Closed { .. } => 10,
        Event::Exiting { .. } => 11,
        Event::Surface { .. } => 12,
        Event::Frame(_) => 13,
        Event::PresentationFailed { .. } => 14,
        Event::Cursor { .. } => 15,
        Event::TitleChanged { .. } => 16,
        Event::AddressChanged { .. } => 17,
        Event::LoadingState { .. } => 18,
        Event::DialogOpened { .. } => 19,
        Event::FileChooserOpened { .. } => 20,
        Event::NewWindowRequested { .. } => 21,
        Event::ImeCaret { .. } => 22,
        Event::LoadProgress { .. } => 23,
    }
}
const EVENT_KINDS: u8 = 24;

fn error_kind(error: &CommandError) -> u8 {
    match error {
        CommandError::TabClosed { .. } => 0,
        CommandError::TabExists { .. } => 1,
        CommandError::TabCrashed { .. } => 2,
        CommandError::Timeout { .. } => 3,
        CommandError::DevTools { .. } => 4,
        CommandError::NavigationRefused { .. } => 5,
        CommandError::NoPendingDecision { .. } => 6,
        CommandError::ProfileUnavailable { .. } => 7,
        CommandError::HostUnavailable { .. } => 8,
        CommandError::Rejected { .. } => 9,
    }
}
const ERROR_KINDS: u8 = 10;

fn refusal_kind(refusal: &Refusal) -> u8 {
    match refusal {
        Refusal::VersionMismatch { .. } => 0,
        Refusal::ProfileRootLocked { .. } => 1,
        Refusal::EngineFailed { .. } => 2,
    }
}
const REFUSAL_KINDS: u8 = 3;

fn frame_kind(frame: &FrameDescriptor) -> u8 {
    match frame {
        FrameDescriptor::SharedMemory { .. } => 0,
        FrameDescriptor::GpuTexture { .. } => 1,
    }
}
const FRAME_KINDS: u8 = 2;

fn tab() -> TabId {
    TabId(42)
}

fn commands() -> Vec<Command> {
    vec![
        Command::OpenTab {
            tab: tab(),
            profile: ProfileName::new("0b6d6713-5706-4af6-8061-06e40f757cb9").unwrap(),
            url: "http://127.0.0.1:5173/".to_string(),
            guarded: true,
        },
        Command::Guard {
            tab: tab(),
            guarded: false,
        },
        Command::Zoom {
            tab: tab(),
            percent: 125,
        },
        Command::Security { tab: tab() },
        Command::Inspect {
            tab: tab(),
            inspector: TabId(9),
        },
        Command::CloseTab { tab: tab() },
        Command::Navigate {
            tab: tab(),
            url: "http://localhost/é?q=\"x\"".to_string(),
        },
        Command::Reload { tab: tab() },
        Command::DevTools {
            tab: tab(),
            method: "Runtime.evaluate".to_string(),
            params: json!({"expression": "1 + 1", "returnByValue": true, "nested": [1, null, {"a": 2.5}]}),
        },
        Command::DecideOrigin {
            tab: tab(),
            origin: "https://example.com".to_string(),
            decision: OriginDecision::AllowOnce,
        },
        Command::DecideOrigin {
            tab: tab(),
            origin: "https://example.com".to_string(),
            decision: OriginDecision::AllowAlways,
        },
        Command::DecideOrigin {
            tab: tab(),
            origin: "https://example.com".to_string(),
            decision: OriginDecision::Deny,
        },
        Command::Present {
            tab: tab(),
            view: Some(View::new(540, 738, 2.0)),
        },
        Command::Present {
            tab: tab(),
            view: None,
        },
        Command::ReleaseFrame {
            tab: tab(),
            generation: 2,
            slot: 1,
        },
        Command::Input {
            tab: tab(),
            input: Input::MouseButton {
                x: 10,
                y: 20,
                button: MouseButton::Left,
                pressed: true,
                clicks: 2,
                modifiers: Modifiers(Modifiers::SHIFT | Modifiers::LEFT_BUTTON),
            },
        },
        Command::Input {
            tab: tab(),
            input: Input::Key {
                kind: KeyKind::Char,
                windows_key_code: 0x41,
                native_key_code: 38,
                character: u16::from(b'a'),
                unmodified_character: u16::from(b'a'),
                modifiers: Modifiers::default(),
            },
        },
        Command::Input {
            tab: tab(),
            input: Input::ImeSetComposition {
                text: "にほん".to_string(),
                selection_start: 1,
                selection_end: 3,
            },
        },
        Command::Input {
            tab: tab(),
            input: Input::Wheel {
                x: 1,
                y: 2,
                delta_x: 0,
                delta_y: -120,
                modifiers: Modifiers::default(),
            },
        },
        Command::Input {
            tab: tab(),
            input: Input::Focus { focused: false },
        },
        Command::History {
            tab: tab(),
            step: HistoryStep::Back,
        },
        Command::Stop { tab: tab() },
        Command::AnswerDialog {
            tab: tab(),
            dialog: 7,
            accept: true,
            text: Some("answer".to_string()),
        },
        Command::AnswerFiles {
            tab: tab(),
            request: 3,
            paths: Some(vec!["/home/u/report.pdf".to_string()]),
        },
        Command::AnswerFiles {
            tab: tab(),
            request: 4,
            paths: None,
        },
        Command::Shutdown,
    ]
}

fn errors() -> Vec<CommandError> {
    vec![
        CommandError::TabClosed { tab: tab() },
        CommandError::TabExists { tab: tab() },
        CommandError::TabCrashed {
            tab: tab(),
            reason: "renderer killed".to_string(),
        },
        CommandError::Timeout {
            method: "Runtime.evaluate".to_string(),
            after_ms: 30_000,
        },
        CommandError::DevTools {
            method: "DOM.nope".to_string(),
            message: "'DOM.nope' wasn't found".to_string(),
        },
        CommandError::NavigationRefused {
            refusal: NavigationRefusal::Scheme {
                scheme: "file".to_string(),
            },
        },
        CommandError::NoPendingDecision {
            tab: tab(),
            origin: "https://example.com".to_string(),
        },
        CommandError::ProfileUnavailable {
            reason: "permission denied".to_string(),
        },
        CommandError::HostUnavailable {
            reason: "the browser engine stopped".to_string(),
        },
        CommandError::Rejected {
            reason: "not ready".to_string(),
        },
    ]
}

fn events() -> Vec<Event> {
    vec![
        Event::LoadStarted {
            tab: tab(),
            url: "http://127.0.0.1/".to_string(),
        },
        Event::Loaded {
            tab: tab(),
            url: "http://127.0.0.1/".to_string(),
            http_status: 200,
        },
        Event::LoadFailed {
            tab: tab(),
            url: "http://127.0.0.1:1/".to_string(),
            error_code: -102,
            error_text: "net::ERR_CONNECTION_REFUSED".to_string(),
        },
        Event::Console {
            tab: tab(),
            level: ConsoleLevel::Error,
            text: "Uncaught TypeError: x is undefined".to_string(),
            source: "http://127.0.0.1/app.js".to_string(),
            line: 14,
        },
        Event::DevTools {
            tab: tab(),
            method: "Network.responseReceived".to_string(),
            params: json!({"response": {"status": 500}}),
        },
        Event::OriginDecisionRequested {
            tab: tab(),
            origin: "https://example.com".to_string(),
            url: "https://example.com/a".to_string(),
        },
        Event::NavigationRefused {
            tab: tab(),
            url: "javascript:alert(1)".to_string(),
            refusal: NavigationRefusal::OriginDenied {
                origin: "https://example.com".to_string(),
            },
        },
        Event::NavigationRefused {
            tab: tab(),
            url: "https://example.com/frame".to_string(),
            refusal: NavigationRefusal::OriginUndecided {
                origin: "https://example.com".to_string(),
            },
        },
        Event::PermissionDenied {
            tab: tab(),
            origin: "http://127.0.0.1".to_string(),
            permissions: vec![
                Permission::Camera,
                Permission::Microphone,
                Permission::Geolocation,
                Permission::Notifications,
                Permission::Other,
            ],
        },
        Event::DownloadCancelled {
            tab: tab(),
            url: "http://127.0.0.1/file.bin".to_string(),
        },
        Event::Crashed {
            tab: tab(),
            reason: "the browser engine stopped".to_string(),
        },
        Event::Closed { tab: tab() },
        Event::Exiting {
            reason: ExitReason::Idle,
        },
        Event::Exiting {
            reason: ExitReason::Shutdown,
        },
        Event::Exiting {
            reason: ExitReason::WindowGone,
        },
        Event::Surface {
            tab: tab(),
            generation: 2,
            width: 1080,
            height: 1476,
            stride: 4320,
            slots: 3,
        },
        Event::Frame(frames().remove(0)),
        Event::PresentationFailed {
            tab: tab(),
            reason: "memfd_create failed".to_string(),
        },
        Event::Cursor {
            tab: tab(),
            cursor: Cursor::Pointer,
        },
        Event::TitleChanged {
            tab: tab(),
            title: "Fixture".to_string(),
        },
        Event::AddressChanged {
            tab: tab(),
            url: "http://127.0.0.1/a".to_string(),
        },
        Event::LoadingState {
            tab: tab(),
            loading: true,
            can_go_back: true,
            can_go_forward: false,
        },
        Event::LoadProgress {
            tab: tab(),
            progress: 0.5,
        },
        Event::DialogOpened {
            tab: tab(),
            dialog: 1,
            kind: DialogKind::Prompt,
            origin: "http://127.0.0.1:5173".to_string(),
            message: "Name?".to_string(),
            default_text: "Ada".to_string(),
        },
        Event::FileChooserOpened {
            tab: tab(),
            request: 2,
            mode: FileMode::OpenMultiple,
            title: String::new(),
            accept: vec![".png".to_string(), "image/*".to_string()],
        },
        Event::NewWindowRequested {
            tab: tab(),
            url: "http://127.0.0.1/popup".to_string(),
            user_gesture: true,
        },
        Event::ImeCaret {
            tab: tab(),
            caret: Rect {
                x: 10,
                y: 20,
                width: 1,
                height: 18,
            },
        },
    ]
}

fn refusals() -> Vec<Refusal> {
    vec![
        Refusal::VersionMismatch { window: 2, host: 1 },
        Refusal::ProfileRootLocked {
            root: PathBuf::from("/r"),
            lock: PathBuf::from("/r/paneflow-host.lock"),
        },
        Refusal::EngineFailed {
            reason: "sandbox".to_string(),
        },
    ]
}

fn frames() -> Vec<FrameDescriptor> {
    vec![
        FrameDescriptor::SharedMemory {
            tab: tab(),
            slot: 1,
            generation: 3,
            width: 540,
            height: 738,
            stride: 2160,
            format: PixelFormat::Bgra8,
        },
        FrameDescriptor::GpuTexture {
            tab: tab(),
            slot: 2,
            generation: 4,
            width: 540,
            height: 738,
            format: PixelFormat::Bgra8,
            modifier: 0x0300_0000_0060_6014,
        },
    ]
}

fn assert_round_trip<T>(value: &T)
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let framed = encode(value).unwrap();
    let (decoded, consumed) = decode::<T>(&framed).unwrap().unwrap();
    assert_eq!(&decoded, value);
    assert_eq!(consumed, framed.len());
    let streamed: T = read_message(&mut framed.as_slice()).unwrap();
    assert_eq!(&streamed, value);
}

fn assert_covers<T>(values: &[T], kind: fn(&T) -> u8, total: u8) {
    let seen: BTreeSet<u8> = values.iter().map(kind).collect();
    assert_eq!(seen, (0..total).collect::<BTreeSet<_>>());
}

#[test]
fn every_command_survives_a_round_trip() {
    let commands = commands();
    assert_covers(&commands, command_kind, COMMAND_KINDS);
    for (index, command) in commands.into_iter().enumerate() {
        assert_round_trip(&WindowMessage::Request {
            id: RequestId(index as u64),
            command,
        });
    }
    assert_round_trip(&WindowMessage::Hello(Hello::current()));
}

#[test]
fn every_event_survives_a_round_trip() {
    let events = events();
    assert_covers(&events, event_kind, EVENT_KINDS);
    for event in events {
        assert_round_trip(&HostMessage::Event(event));
    }
}

#[test]
fn every_reply_and_error_survives_a_round_trip() {
    let errors = errors();
    assert_covers(&errors, error_kind, ERROR_KINDS);
    let mut outcomes = vec![
        Outcome::Done,
        Outcome::DevTools {
            result: json!({"result": {"type": "number", "value": 2}}),
        },
        Outcome::Security {
            security: SiteSecurity {
                url: "https://vercel.com/".to_string(),
                connection: Connection::Secure,
                protocol: Some("TLS 1.3".to_string()),
                certificate: Some(Certificate {
                    subject: CertificateName {
                        common_name: "vercel.com".to_string(),
                        organization: None,
                    },
                    issuer: CertificateName {
                        common_name: "R11".to_string(),
                        organization: Some("Let's Encrypt".to_string()),
                    },
                    valid_from: 1_790_000_000,
                    valid_until: 1_797_776_000,
                }),
            },
        },
        Outcome::Security {
            security: SiteSecurity {
                url: "http://localhost:3000/".to_string(),
                connection: Connection::Plain,
                protocol: None,
                certificate: None,
            },
        },
    ];
    outcomes.extend(errors.into_iter().map(|error| Outcome::Failed { error }));
    for (index, outcome) in outcomes.into_iter().enumerate() {
        assert_round_trip(&HostMessage::Reply {
            id: RequestId(index as u64),
            outcome,
        });
    }
}

#[test]
fn every_handshake_answer_survives_a_round_trip() {
    let refusals = refusals();
    assert_covers(&refusals, refusal_kind, REFUSAL_KINDS);
    for refusal in refusals {
        assert_round_trip(&HostMessage::Refused(refusal));
    }
    assert_round_trip(&HostMessage::Welcome(Welcome {
        protocol_version: 2,
        host_pid: 4242,
        engine: "CEF 156.0.31+ge603d97+chromium-156.0.8078.100".to_string(),
        sandbox: true,
    }));
}

#[test]
fn every_frame_descriptor_survives_a_round_trip() {
    let frames = frames();
    assert_covers(&frames, frame_kind, FRAME_KINDS);
    for frame in frames {
        assert_round_trip(&frame);
    }
}
