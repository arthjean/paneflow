#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::PathBuf;

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
    }
}
const COMMAND_KINDS: u8 = 7;

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
    }
}
const EVENT_KINDS: u8 = 12;

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
