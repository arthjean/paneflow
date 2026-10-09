#![cfg(all(
    feature = "native",
    any(
        target_os = "linux",
        all(target_os = "windows", target_arch = "x86_64", target_env = "msvc")
    )
))]

use paneflow_terminal_ghostty::{
    BackendEvent, DisplayTerminal, ProgramStatusKind, ProgramStatusReport, ProgramStatusState,
    PromptKind, SemanticPromptKind, TerminalAppearance, WindowSize,
};

const STATUS_QUERY: &[u8] = b"\x1b]7501;?\x1b\\";

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
fn terminal(program_status: bool) -> DisplayTerminal {
    let mut terminal = DisplayTerminal::new(
        WindowSize::new(80, 24, 8, 16).unwrap(),
        1_000,
        TerminalAppearance::default(),
    )
    .unwrap();
    if program_status {
        terminal.enable_program_status().unwrap();
    }
    terminal
}

#[allow(
    clippy::unwrap_used,
    reason = "test fixture setup must fail immediately"
)]
fn feed(terminal: &mut DisplayTerminal, bytes: &[u8]) -> Vec<BackendEvent> {
    terminal.feed(bytes).unwrap();
    terminal.drain_events()
}

fn reports(events: &[BackendEvent]) -> Vec<ProgramStatusReport> {
    events
        .iter()
        .filter_map(|event| match event {
            BackendEvent::ProgramStatus(report) => Some(report.clone()),
            _ => None,
        })
        .collect()
}

fn replies(events: &[BackendEvent]) -> Vec<Vec<u8>> {
    events
        .iter()
        .filter_map(|event| match event {
            BackendEvent::WritePty(bytes) => Some(bytes.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_full_report_reaches_the_wrapper_with_every_field_copied() {
    let mut terminal = terminal(true);
    let events = feed(
        &mut terminal,
        b"\x1b]7501;state=blocked:kind=permission:progress=42:id=tf/plan:app=terraform:title=UGxhbg==:msg=QXBwbHk/\x1b\\",
    );

    assert_eq!(
        reports(&events),
        [ProgramStatusReport {
            state: ProgramStatusState::Blocked,
            kind: Some(ProgramStatusKind::Permission),
            progress: Some(42),
            id: "tf/plan".into(),
            app: "terraform".into(),
            title: "Plan".into(),
            message: "Apply?".into(),
        }]
    );
}

#[test]
fn a_report_without_progress_or_kind_maps_both_to_none() {
    let mut terminal = terminal(true);
    let events = feed(&mut terminal, b"\x1b]7501;state=done\x1b\\");

    assert_eq!(
        reports(&events),
        [ProgramStatusReport {
            state: ProgramStatusState::Done,
            kind: None,
            progress: None,
            id: String::new(),
            app: String::new(),
            title: String::new(),
            message: String::new(),
        }]
    );
}

#[test]
fn an_unknown_blocked_kind_becomes_none() {
    let mut terminal = terminal(true);
    let events = feed(
        &mut terminal,
        b"\x1b]7501;state=blocked:kind=telepathy\x1b\\",
    );

    let reports = reports(&events);
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].state, ProgramStatusState::Blocked);
    assert_eq!(reports[0].kind, None);
}

#[test]
fn a_message_that_decodes_to_invalid_utf8_produces_no_event() {
    let mut terminal = terminal(true);
    let events = feed(&mut terminal, b"\x1b]7501;state=done:msg=/w==\x1b\\");

    assert!(reports(&events).is_empty(), "{events:?}");
}

#[test]
fn only_a_terminal_with_program_status_answers_the_support_query() {
    let mut desktop = terminal(false);
    let events = feed(&mut desktop, STATUS_QUERY);
    assert!(replies(&events).is_empty(), "{events:?}");

    let mut host = terminal(true);
    let events = feed(&mut host, STATUS_QUERY);
    assert_eq!(replies(&events), [STATUS_QUERY.to_vec()]);
}

#[test]
fn a_full_reset_supersedes_pending_status_events_and_is_reported_last() {
    let mut terminal = terminal(true);
    let events = feed(
        &mut terminal,
        b"\x1b]7501;state=working:app=cargo\x1b\\\x1b]133;A\x1b\\\x1bc",
    );

    assert!(reports(&events).is_empty(), "{events:?}");
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, BackendEvent::SemanticPrompt { .. }))
    );
    assert_eq!(events.last(), Some(&BackendEvent::Reset));
}

#[test]
fn a_full_reset_is_reported_without_program_status() {
    let mut terminal = terminal(false);
    let events = feed(&mut terminal, b"\x1bc");

    assert_eq!(
        events
            .iter()
            .filter(|event| **event == BackendEvent::Reset)
            .count(),
        1
    );
}

#[test]
fn an_embedder_reset_is_reported_like_a_program_reset() {
    let mut terminal = terminal(true);
    let _ = feed(&mut terminal, b"\x1b]7501;state=working\x1b\\");

    assert!(terminal.feed(b"\x1b]7501;state=blocked\x1b\\").is_ok());
    terminal.reset();
    let events = terminal.drain_events();

    assert!(reports(&events).is_empty(), "{events:?}");
    assert_eq!(events, [BackendEvent::Reset]);
}

#[test]
fn semantic_prompts_carry_their_kind_and_exit_code() {
    let mut terminal = terminal(true);
    let events = feed(&mut terminal, b"\x1b]133;A\x1b\\\x1b]133;D;2\x1b\\");

    assert_eq!(
        events,
        [
            BackendEvent::SemanticPrompt {
                kind: SemanticPromptKind::PromptStart,
                prompt_kind: PromptKind::Primary,
                exit_code: None,
            },
            BackendEvent::SemanticPrompt {
                kind: SemanticPromptKind::CommandEnd,
                prompt_kind: PromptKind::Primary,
                exit_code: Some(2),
            },
        ]
    );
}

#[test]
fn semantic_prompts_are_not_reported_without_program_status() {
    let mut terminal = terminal(false);
    let events = feed(&mut terminal, b"\x1b]133;A\x1b\\");

    assert!(events.is_empty(), "{events:?}");
}

#[test]
fn a_status_flood_beyond_sixty_four_pending_events_overflows() {
    let mut terminal = terminal(true);
    let mut flood = Vec::new();
    for index in 0..65 {
        flood.extend_from_slice(format!("\x1b]7501;state=working:id=job{index}\x1b\\").as_bytes());
    }
    let events = feed(&mut terminal, &flood);

    assert_eq!(reports(&events).len(), 64);
    assert!(events.iter().any(|event| matches!(
        event,
        BackendEvent::EffectsOverflow {
            dropped_events: 1,
            ..
        }
    )));
}
