use paneflow_config::schema::SessionGeneration;
use paneflow_host::host::SessionReconnection;
use paneflow_program_resume::CwdSource;
use serde_json::json;

use super::*;

const RECORD: u64 = 7;
const ATTEMPT: &str = "0123456789abcdef0123456789abcdef";

fn argv(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_string()).collect()
}

fn candidate(items: &[&str]) -> ResumeCandidate {
    ResumeCandidate {
        record: RecordId::from(RECORD),
        argv: argv(items),
        cwd: "/home/user/project".to_string(),
        cwd_source: CwdSource::Report,
        owner_pid: 4242,
        accepted_at_ms: 1,
        settled: true,
        blocked: None,
    }
}

fn lost_with(candidate: ResumeCandidate) -> HostLinkEnd {
    HostLinkEnd::from_reconnection(SessionReconnection::Lost, SessionGeneration::FIRST)
        .with_resume_candidate(Some(candidate))
}

fn policy(settings: serde_json::Value, restore_conversations: bool) -> ResumePolicy {
    let config: PaneFlowConfig = serde_json::from_value(json!({
        "terminal": {"program_resume": settings},
        "agents": {"restore_conversations": restore_conversations},
    }))
    .unwrap();
    ResumePolicy::from_config(&config)
}

fn ask() -> ResumePolicy {
    policy(json!({"mode": "ask"}), true)
}

fn requested_record(state: &ResumeState) -> Option<RecordId> {
    state.restart_request().and_then(|request| request.record)
}

#[test]
fn auto_resumes_a_settled_record_of_a_lost_session_through_the_restart() {
    let state = restore_state(
        &policy(json!({"mode": "auto"}), true),
        &lost_with(candidate(&["htop"])),
    );
    let request = state.restart_request().expect("the restart carries resume");
    assert_eq!(request.record, Some(RecordId::from(RECORD)));
    assert!(!request.confirmed, "an automatic resume never confirms");
    assert!(state.banner_message().is_none());
}

#[test]
fn ask_resumes_a_trusted_bare_program_without_a_banner() {
    let state = restore_state(
        &policy(json!({"mode": "ask", "trusted_programs": ["nvim"]}), true),
        &lost_with(candidate(&["nvim", "-S", "session.vim"])),
    );
    assert_eq!(requested_record(&state), Some(RecordId::from(RECORD)));
    assert!(state.banner_message().is_none());
}

#[test]
fn ask_offers_an_untrusted_record_and_starts_a_plain_shell() {
    let state = restore_state(&ask(), &lost_with(candidate(&["htop"])));
    assert!(matches!(
        state,
        ResumeState::Offered {
            reason: OfferReason::Untrusted,
            ..
        }
    ));
    assert_eq!(state.restart_request(), None);
    assert!(state.offers_resume());
}

#[test]
fn an_unsettled_record_is_offered_and_never_resumed_even_in_auto() {
    let mut unsettled = candidate(&["htop"]);
    unsettled.settled = false;
    let state = restore_state(
        &policy(json!({"mode": "auto"}), true),
        &lost_with(unsettled),
    );
    assert!(matches!(
        state,
        ResumeState::Offered {
            reason: OfferReason::Unsettled,
            ..
        }
    ));
    assert_eq!(state.restart_request(), None);
    assert_eq!(
        state.banner_message().unwrap(),
        "Resume did not complete last time: htop in /home/user/project"
    );
}

#[test]
fn a_session_the_user_exited_with_code_0_resumes_nothing() {
    let end = HostLinkEnd::exited(0, None).with_resume_candidate(Some(candidate(&["htop"])));
    assert_eq!(
        restore_state(&policy(json!({"mode": "auto"}), true), &end),
        ResumeState::None
    );
}

#[test]
fn mode_off_neither_resumes_nor_offers() {
    for record in [
        candidate(&["htop"]),
        candidate(&["pi", "--session", "abc_1"]),
    ] {
        assert_eq!(
            restore_state(&policy(json!({"mode": "off"}), true), &lost_with(record)),
            ResumeState::None
        );
    }
}

#[test]
fn a_blocked_summary_blocks_the_pane_and_requests_nothing() {
    for refusal in [
        ResumeRefusal::CwdMissing,
        ResumeRefusal::ShellUnsupported,
        ResumeRefusal::SandboxChanged,
    ] {
        let mut blocked = candidate(&["htop"]);
        blocked.blocked = Some(refusal);
        let state = restore_state(&policy(json!({"mode": "auto"}), true), &lost_with(blocked));
        assert!(
            matches!(&state, ResumeState::Blocked { reason, .. } if *reason == refusal),
            "{state:?}"
        );
        assert_eq!(state.restart_request(), None);
        assert!(!state.offers_resume());
    }
}

#[test]
fn a_lost_session_without_a_record_has_no_resume_state() {
    let end = HostLinkEnd::from_reconnection(SessionReconnection::Lost, SessionGeneration::FIRST);
    assert_eq!(
        restore_state(&policy(json!({"mode": "auto"}), true), &end),
        ResumeState::None
    );
}

#[test]
fn resumes_are_paced_in_reservation_order_at_least_the_spacing_apart() {
    let pacer = ResumePacer::default();
    let now = Instant::now();
    let slots: Vec<Instant> = (0..4).map(|_| pacer.reserve(now)).collect();
    assert_eq!(slots[0], now);
    for pair in slots.windows(2) {
        assert_eq!(pair[1] - pair[0], RESUME_SPACING);
    }
    let later = slots[3] + RESUME_SPACING * 3;
    assert_eq!(pacer.reserve(later), later);
}

#[test]
fn each_blocked_reason_shows_its_edge_case_message_without_a_resume_button() {
    let messages = [
        (
            ResumeRefusal::CwdMissing,
            "Not resumed: `/home/user/project` no longer exists",
        ),
        (
            ResumeRefusal::ShellUnsupported,
            "This shell cannot resume programs",
        ),
        (
            ResumeRefusal::SandboxChanged,
            "Sandbox changed; not resumed",
        ),
    ];
    for (reason, message) in messages {
        let state = ResumeState::Blocked {
            command: ResumeCommand::of(&candidate(&["htop"])),
            reason,
        };
        assert_eq!(state.banner_message().as_deref(), Some(message));
        assert!(!state.offers_resume());
        assert_eq!(state.dismissed_record(), Some(RecordId::from(RECORD)));
    }
}

#[test]
fn an_offer_quotes_the_argv_element_by_element_and_names_the_cwd() {
    let state = restore_state(
        &ask(),
        &lost_with(candidate(&["nvim", "-S", "two words", "", "it's"])),
    );
    assert_eq!(
        state.banner_message().unwrap(),
        "Resume this program?: nvim -S 'two words' '' 'it'\\''s' in /home/user/project"
    );
}

#[test]
fn a_bidi_override_is_shown_escaped_never_as_the_character() {
    let mut record = candidate(&["nvim", "evil\u{202E}txt"]);
    record.cwd = "/tmp/\u{202E}x".to_string();
    let message = restore_state(&ask(), &lost_with(record))
        .banner_message()
        .unwrap();
    assert!(!message.contains('\u{202E}'), "{message}");
    assert!(message.contains("evil\\u{202E}txt"), "{message}");
    assert!(message.contains("/tmp/\\u{202E}x"), "{message}");
}

#[test]
fn a_failed_launch_names_the_command_and_the_reason() {
    let resuming = ResumeState::Resuming {
        command: ResumeCommand::of(&candidate(&["htop", "-d", "5"])),
        attempt: Some(AttemptId::parse(ATTEMPT).unwrap()),
    };
    let report = ResumeAttemptReport {
        attempt: Some(AttemptId::parse(ATTEMPT).unwrap()),
        record: Some(RecordId::from(RECORD)),
        outcome: AttemptOutcome::Failed,
        reason: Some("not_found".to_string()),
        program: Some("htop".to_string()),
        pid: None,
    };
    assert!(resuming.settled_outcome(&report));
    let failed = resuming.after_attempt(Some(&report));
    assert_eq!(
        failed.banner_message().as_deref(),
        Some("Could not resume `htop`: not found")
    );
    assert!(!failed.offers_resume());
    assert_eq!(failed.dismissed_record(), None);
}

#[test]
fn an_attempt_moves_from_prepared_to_cleared_once_started() {
    let requested = ResumeState::Resuming {
        command: ResumeCommand::of(&candidate(&["htop"])),
        attempt: None,
    };
    let mut report = ResumeAttemptReport {
        attempt: Some(AttemptId::parse(ATTEMPT).unwrap()),
        record: Some(RecordId::from(RECORD)),
        outcome: AttemptOutcome::Prepared,
        reason: None,
        program: None,
        pid: None,
    };
    let prepared = requested.after_attempt(Some(&report));
    assert_eq!(
        prepared.awaited_attempt(),
        Some(&AttemptId::parse(ATTEMPT).unwrap())
    );
    assert!(!prepared.settled_outcome(&report));
    report.outcome = AttemptOutcome::Started;
    assert!(prepared.settled_outcome(&report));
    assert_eq!(prepared.after_attempt(Some(&report)), ResumeState::None);
}

#[test]
fn a_refused_restart_turns_into_the_state_its_reason_names() {
    let refused = |reason: &str| {
        ResumeState::Resuming {
            command: ResumeCommand::of(&candidate(&["htop"])),
            attempt: None,
        }
        .after_attempt(Some(&ResumeAttemptReport {
            attempt: None,
            record: Some(RecordId::from(RECORD)),
            outcome: AttemptOutcome::Refused,
            reason: Some(reason.to_string()),
            program: None,
            pid: None,
        }))
    };
    assert!(matches!(
        refused("cwd_missing"),
        ResumeState::Blocked {
            reason: ResumeRefusal::CwdMissing,
            ..
        }
    ));
    assert!(matches!(
        refused("unsettled"),
        ResumeState::Offered {
            reason: OfferReason::Unsettled,
            ..
        }
    ));
    assert_eq!(refused("unknown_record"), ResumeState::None);
    assert_eq!(
        refused("launcher_missing").banner_message().as_deref(),
        Some(
            "Could not resume `htop`: the resume launcher is not installed beside the host's helpers"
        )
    );
}

#[test]
fn the_banner_types_only_the_quoted_launcher_and_handoff_then_enter() {
    let launcher = "/home/u/.paneflow/cache/bin/0.20.0/paneflow-resume";
    let handoff = "/home/u/my sessions/resume-1.handoff";
    assert_eq!(
        launcher_input(ShellQuoting::Posix, launcher, handoff, false),
        format!("{launcher} '{handoff}'\r").into_bytes()
    );
    assert_eq!(
        launcher_input(ShellQuoting::Posix, launcher, handoff, true),
        format!("\x15{launcher} '{handoff}'\r").into_bytes()
    );
    assert_eq!(
        launcher_input(ShellQuoting::PowerShell, launcher, handoff, false),
        format!("& {launcher} '{handoff}'\r").into_bytes()
    );
}

#[test]
fn a_pi_record_in_a_declared_shape_resumes_without_a_banner_in_ask_mode() {
    for record in [
        candidate(&["pi", "--session", "abc_1"]),
        candidate(&[
            "pi",
            "--session-dir",
            "/home/user/.pi/sessions",
            "--session",
            "abc_1",
        ]),
    ] {
        let state = restore_state(&ask(), &lost_with(record));
        assert_eq!(
            requested_record(&state),
            Some(RecordId::from(RECORD)),
            "{state:?}"
        );
    }
}

#[test]
fn a_pi_record_outside_the_declared_shapes_is_offered() {
    for record in [
        candidate(&["pi", "--extension", "x.js"]),
        candidate(&["pi", "--session", "-rf"]),
        candidate(&["pi", "--session-dir", "relative", "--session", "abc_1"]),
        candidate(&["pi", "--session", "abc_1", "--extra"]),
    ] {
        let state = restore_state(&policy(json!({"mode": "auto"}), true), &lost_with(record));
        assert!(
            matches!(
                state,
                ResumeState::Offered {
                    reason: OfferReason::AgentShape,
                    ..
                }
            ),
            "{state:?}"
        );
    }
}

#[test]
fn turning_conversation_restore_off_offers_a_pi_record() {
    let state = restore_state(
        &policy(json!({"mode": "auto"}), false),
        &lost_with(candidate(&["pi", "--session", "abc_1"])),
    );
    assert!(matches!(
        state,
        ResumeState::Offered {
            reason: OfferReason::AgentsDisabled,
            ..
        }
    ));
}

#[test]
fn an_absolute_pi_is_not_the_pi_runtime_and_is_offered_in_ask_mode() {
    let state = restore_state(
        &ask(),
        &lost_with(candidate(&["/tmp/pi", "--session", "abc"])),
    );
    assert!(matches!(
        state,
        ResumeState::Offered {
            reason: OfferReason::Untrusted,
            ..
        }
    ));
    assert_eq!(agent_of(&argv(&["/tmp/pi"])), None);
    assert!(agent_of(&argv(&["pi"])).is_some());
}

#[test]
fn every_record_state_takes_the_pane_from_a_catalog_conversation() {
    let resumes = restore_state(&ask(), &lost_with(candidate(&["pi", "--session", "abc_1"])));
    let offered = restore_state(&ask(), &lost_with(candidate(&["htop"])));
    assert!(resumes.yields_conversation());
    assert!(offered.yields_conversation());
    assert!(!ResumeState::None.yields_conversation());
}
