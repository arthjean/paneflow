use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui::{App, Context, Entity, SharedString};
use paneflow_host::program_status::DeclaredStatus;

use crate::PaneFlowApp;
use crate::app::host_agents::{HostAgentRow, host_observed_agent};
use crate::terminal::TerminalView;
use crate::workspace::Workspace;

pub(crate) const BLOCKED_NOTIFICATION_INTERVAL: Duration = Duration::from_secs(10);

const CHIP_MAX_CHARS: usize = 48;

const LINE_MAX_CHARS: usize = 512;

const UNNAMED_PROGRAM: &str = "A program";

const BLOCKED_WITHOUT_MESSAGE: &str = "Needs input";

const ERROR_WITHOUT_MESSAGE: &str = "Failed";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeclaredState {
    Idle,
    Working,
    Blocked,
    Done,
    Error,
}

impl DeclaredState {
    pub(crate) fn of(status: &DeclaredStatus) -> Option<Self> {
        match status.state.as_str() {
            "idle" => Some(Self::Idle),
            "working" => Some(Self::Working),
            "blocked" => Some(Self::Blocked),
            "done" => Some(Self::Done),
            "error" => Some(Self::Error),
            _ => None,
        }
    }

    fn queues(self) -> bool {
        matches!(self, Self::Blocked | Self::Error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DeclaredChip {
    Fallback,
    Hidden,
    Show { label: SharedString, error: bool },
}

pub(crate) fn declared_chip(status: Option<&DeclaredStatus>) -> DeclaredChip {
    let Some((status, state)) =
        status.and_then(|status| Some((status, DeclaredState::of(status)?)))
    else {
        return DeclaredChip::Fallback;
    };
    let Some(base) = state_label(state, status.progress, status.kind.as_deref()) else {
        return DeclaredChip::Hidden;
    };
    let label = match one_line(&status.message).or_else(|| one_line(&status.title)) {
        Some(detail) => format!("{base} · {detail}"),
        None => base,
    };
    DeclaredChip::Show {
        label: truncate_chars(&label, CHIP_MAX_CHARS).into(),
        error: state == DeclaredState::Error,
    }
}

fn state_label(state: DeclaredState, progress: Option<u8>, kind: Option<&str>) -> Option<String> {
    match state {
        DeclaredState::Idle => None,
        DeclaredState::Working => Some(match progress {
            Some(percent) => format!("working {}%", percent.min(100)),
            None => "working".to_string(),
        }),
        DeclaredState::Blocked => Some(match kind_label(kind) {
            Some(kind) => format!("blocked: {kind}"),
            None => "blocked".to_string(),
        }),
        DeclaredState::Done => Some("done".to_string()),
        DeclaredState::Error => Some("error".to_string()),
    }
}

fn kind_label(kind: Option<&str>) -> Option<&'static str> {
    match kind? {
        "permission" => Some("permission"),
        "question" => Some("question"),
        "auth" => Some("auth"),
        _ => None,
    }
}

fn is_hidden_control(c: char) -> bool {
    matches!(c,
        '\u{202A}'..='\u{202E}'
        | '\u{2066}'..='\u{2069}'
        | '\u{200B}'..='\u{200F}'
        | '\u{2028}'
        | '\u{2029}'
        | '\u{206A}'..='\u{206F}'
    )
}

pub(crate) fn one_line(raw: &str) -> Option<String> {
    let visible: String = raw.chars().filter(|c| !is_hidden_control(*c)).collect();
    let line: String = visible
        .split(['\n', '\r'])
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .take(LINE_MAX_CHARS)
        .collect();
    let line = line.trim();
    (!line.is_empty()).then(|| line.to_string())
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut kept: String = text.chars().take(max.saturating_sub(1)).collect();
    kept.truncate(kept.trim_end().len());
    kept.push('…');
    kept
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeclaredQueueEntry {
    pub(crate) label: String,
    pub(crate) message: Option<String>,
    pub(crate) errored: bool,
}

pub(crate) fn queue_entry(
    status: Option<&DeclaredStatus>,
    pane_title: &str,
) -> Option<DeclaredQueueEntry> {
    let status = status?;
    let state = DeclaredState::of(status).filter(|state| state.queues())?;
    Some(DeclaredQueueEntry {
        label: one_line(&status.app)
            .or_else(|| one_line(pane_title))
            .unwrap_or_else(|| UNNAMED_PROGRAM.to_string()),
        message: one_line(&status.message),
        errored: state == DeclaredState::Error,
    })
}

pub(crate) fn blocked_notification_text(
    status: &DeclaredStatus,
    pane_title: &str,
) -> (String, String) {
    let program = one_line(&status.app).unwrap_or_else(|| UNNAMED_PROGRAM.to_string());
    let summary = match one_line(pane_title) {
        Some(pane) => format!("{program} needs input in {pane}"),
        None => format!("{program} needs input"),
    };
    let body = one_line(&status.message).unwrap_or_else(|| BLOCKED_WITHOUT_MESSAGE.to_string());
    (summary, body)
}

fn error_notification_text(status: &DeclaredStatus, pane_title: &str) -> (String, String) {
    let program = one_line(&status.app).unwrap_or_else(|| UNNAMED_PROGRAM.to_string());
    let summary = match one_line(pane_title) {
        Some(pane) => format!("{program} failed in {pane}"),
        None => format!("{program} failed"),
    };
    let body = one_line(&status.message).unwrap_or_else(|| ERROR_WITHOUT_MESSAGE.to_string());
    (summary, body)
}

fn attention_notification_text(
    status: &DeclaredStatus,
    pane_title: &str,
) -> Option<(String, String)> {
    match DeclaredState::of(status)? {
        DeclaredState::Blocked => Some(blocked_notification_text(status, pane_title)),
        DeclaredState::Error => Some(error_notification_text(status, pane_title)),
        DeclaredState::Idle | DeclaredState::Working | DeclaredState::Done => None,
    }
}

fn declared_attention_seen(visible: Option<&HashSet<u64>>, surface_id: u64, muted: bool) -> bool {
    crate::app::agent_status::completion_was_seen(visible, Some(surface_id)) || muted
}

pub(crate) fn surface_has_agent(
    workspace: &Workspace,
    surface_id: u64,
    row: Option<&HostAgentRow>,
) -> bool {
    host_observed_agent(row).is_some()
        || workspace
            .agent_sessions
            .values()
            .any(|session| session.surface_id == Some(surface_id))
}

#[derive(Debug)]
struct Watched {
    state: Option<DeclaredState>,
    since: Instant,
    notified_at: Option<Instant>,
    acknowledged: bool,
}

impl Watched {
    fn awaits_acknowledgment(&self) -> bool {
        self.state == Some(DeclaredState::Error) && !self.acknowledged
    }
}

#[derive(Debug, Default)]
pub(crate) struct DeclaredWatch {
    surfaces: HashMap<u64, Watched>,
}

impl DeclaredWatch {
    pub(crate) fn observe(
        &mut self,
        surface_id: u64,
        state: Option<DeclaredState>,
        notifiable: bool,
        now: Instant,
    ) -> bool {
        let watched = self.surfaces.entry(surface_id).or_insert(Watched {
            state: None,
            since: now,
            notified_at: None,
            acknowledged: false,
        });
        let entered = watched.state != state;
        if entered {
            watched.state = state;
            watched.since = now;
            watched.acknowledged = false;
        }
        if !(entered && notifiable && state.is_some_and(DeclaredState::queues)) {
            return false;
        }
        if watched
            .notified_at
            .is_some_and(|at| now.saturating_duration_since(at) < BLOCKED_NOTIFICATION_INTERVAL)
        {
            return false;
        }
        watched.notified_at = Some(now);
        true
    }

    pub(crate) fn since(&self, surface_id: u64) -> Option<Instant> {
        self.surfaces
            .get(&surface_id)
            .filter(|watched| watched.state.is_some())
            .map(|watched| watched.since)
    }

    pub(crate) fn queue_entry(
        &self,
        surface_id: u64,
        status: Option<&DeclaredStatus>,
        pane_title: &str,
    ) -> Option<DeclaredQueueEntry> {
        queue_entry(status, pane_title)
            .filter(|entry| !(entry.errored && self.acknowledged(surface_id)))
    }

    fn acknowledged(&self, surface_id: u64) -> bool {
        self.surfaces
            .get(&surface_id)
            .is_some_and(|watched| watched.acknowledged)
    }

    fn awaits_acknowledgment(&self) -> bool {
        self.surfaces.values().any(Watched::awaits_acknowledgment)
    }

    fn acknowledge_seen(&mut self, visible: &HashSet<u64>) -> bool {
        let mut acknowledged = false;
        for (surface_id, watched) in &mut self.surfaces {
            if watched.awaits_acknowledgment() && visible.contains(surface_id) {
                watched.acknowledged = true;
                acknowledged = true;
            }
        }
        acknowledged
    }

    fn retain(&mut self, live: &HashSet<u64>) {
        self.surfaces
            .retain(|surface_id, _| live.contains(surface_id));
    }
}

impl PaneFlowApp {
    pub(crate) fn refresh_declared_status(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let mut live = HashSet::new();
        let mut attention = Vec::new();
        for ws_idx in 0..self.workspaces.len() {
            for pane in self.workspaces[ws_idx].collect_panes() {
                let terminals: Vec<Entity<TerminalView>> =
                    pane.read(cx).terminals().cloned().collect();
                let mut pane_changed = false;
                for terminal in terminals {
                    let surface_id = terminal.entity_id().as_u64();
                    live.insert(surface_id);
                    let row = self.host_agents.row(&terminal.read(cx).terminal.session_id);
                    let declared = row.and_then(|row| row.declared_status.clone());
                    let state = declared.as_ref().and_then(DeclaredState::of);
                    let agentless = !surface_has_agent(&self.workspaces[ws_idx], surface_id, row);
                    if self
                        .host_agents
                        .declared_watch
                        .observe(surface_id, state, agentless, now)
                    {
                        attention.push((ws_idx, surface_id, terminal.clone()));
                    }
                    if terminal.read(cx).terminal.declared_status != declared {
                        terminal.update(cx, |view, _| view.terminal.declared_status = declared);
                        pane_changed = true;
                    }
                }
                if pane_changed {
                    pane.update(cx, |_, cx| cx.notify());
                }
            }
        }
        self.host_agents.declared_watch.retain(&live);
        for (ws_idx, surface_id, terminal) in attention {
            self.notify_declared_attention(ws_idx, surface_id, &terminal, cx);
        }
    }

    pub(crate) fn acknowledge_seen_declared_errors(&mut self, cx: &App) -> bool {
        if !self.host_agents.declared_watch.awaits_acknowledgment() {
            return false;
        }
        let visible: HashSet<u64> = self
            .workspaces
            .iter()
            .filter_map(|workspace| self.surfaces_under_user_eye(workspace.id, cx))
            .flatten()
            .collect();
        self.host_agents.declared_watch.acknowledge_seen(&visible)
    }

    fn notify_declared_attention(
        &self,
        ws_idx: usize,
        surface_id: u64,
        terminal: &Entity<TerminalView>,
        cx: &mut Context<Self>,
    ) {
        let Some(status) = terminal.read(cx).terminal.declared_status.clone() else {
            return;
        };
        let Some(workspace) = self.workspaces.get(ws_idx) else {
            return;
        };
        let pane_title = crate::pane::Pane::terminal_surface_title(terminal, cx);
        let Some((summary, body)) = attention_notification_text(&status, &pane_title) else {
            return;
        };
        let seen = declared_attention_seen(
            self.surfaces_under_user_eye(workspace.id, cx).as_ref(),
            surface_id,
            workspace.muted,
        );
        crate::agents::notifications::fire_desktop_notification_for_session(
            crate::agents::notifications::program_notification(summary, body, &pane_title),
            &self.cached_config,
            seen,
            Some(surface_id),
            cx.background_executor().clone(),
        );
    }

    pub(crate) fn declared_since(&self, surface_id: u64) -> Option<Instant> {
        self.host_agents.declared_watch.since(surface_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(state: &str) -> DeclaredStatus {
        DeclaredStatus {
            state: state.to_string(),
            kind: None,
            progress: None,
            app: String::new(),
            title: String::new(),
            message: String::new(),
        }
    }

    type LabelCase = (
        DeclaredState,
        Option<u8>,
        Option<&'static str>,
        Option<&'static str>,
    );

    fn shown(chip: DeclaredChip) -> (String, bool) {
        match chip {
            DeclaredChip::Show { label, error } => (label.to_string(), error),
            other => panic!("expected a shown chip, got {other:?}"),
        }
    }

    #[test]
    fn every_declared_state_maps_to_its_own_label() {
        let table: &[LabelCase] = &[
            (DeclaredState::Working, None, None, Some("working")),
            (DeclaredState::Working, Some(40), None, Some("working 40%")),
            (
                DeclaredState::Working,
                Some(250),
                None,
                Some("working 100%"),
            ),
            (DeclaredState::Blocked, None, None, Some("blocked")),
            (
                DeclaredState::Blocked,
                None,
                Some("permission"),
                Some("blocked: permission"),
            ),
            (
                DeclaredState::Blocked,
                None,
                Some("question"),
                Some("blocked: question"),
            ),
            (
                DeclaredState::Blocked,
                None,
                Some("auth"),
                Some("blocked: auth"),
            ),
            (
                DeclaredState::Blocked,
                None,
                Some("telepathy"),
                Some("blocked"),
            ),
            (DeclaredState::Done, None, None, Some("done")),
            (DeclaredState::Error, None, None, Some("error")),
            (DeclaredState::Idle, None, None, None),
        ];
        for (state, progress, kind, expected) in table {
            assert_eq!(
                state_label(*state, *progress, *kind).as_deref(),
                *expected,
                "{state:?} {progress:?} {kind:?}"
            );
        }
    }

    #[test]
    fn every_wire_state_parses_and_an_unknown_one_does_not() {
        for (wire, state) in [
            ("idle", Some(DeclaredState::Idle)),
            ("working", Some(DeclaredState::Working)),
            ("blocked", Some(DeclaredState::Blocked)),
            ("done", Some(DeclaredState::Done)),
            ("error", Some(DeclaredState::Error)),
            ("clear", None),
            ("", None),
            ("WORKING", None),
        ] {
            assert_eq!(DeclaredState::of(&status(wire)), state, "{wire:?}");
        }
    }

    #[test]
    fn the_chip_names_the_state_and_why_it_is_blocked() {
        let mut blocked = status("blocked");
        blocked.kind = Some("permission".to_string());
        blocked.message = "Apply the plan?".to_string();
        assert_eq!(
            shown(declared_chip(Some(&blocked))),
            ("blocked: permission · Apply the plan?".to_string(), false)
        );

        let mut failed = status("error");
        failed.title = "cargo test".to_string();
        assert_eq!(
            shown(declared_chip(Some(&failed))),
            ("error · cargo test".to_string(), true)
        );

        let mut long = status("working");
        long.message = "x".repeat(200);
        let (label, _) = shown(declared_chip(Some(&long)));
        assert_eq!(label.chars().count(), CHIP_MAX_CHARS);
        assert!(label.ends_with('…'));
    }

    #[test]
    fn a_session_without_a_declared_status_keeps_the_progress_chip() {
        assert_eq!(declared_chip(None), DeclaredChip::Fallback);
    }

    #[test]
    fn an_absent_or_unknown_state_falls_back_to_the_progress_chip() {
        for wire in ["", "paused", "clear"] {
            assert_eq!(
                declared_chip(Some(&status(wire))),
                DeclaredChip::Fallback,
                "{wire:?}"
            );
        }
        let absent: DeclaredStatus =
            serde_json::from_value(serde_json::json!({"message": "no state"})).unwrap();
        assert_eq!(declared_chip(Some(&absent)), DeclaredChip::Fallback);
    }

    #[test]
    fn a_declared_idle_hides_the_progress_chip_instead_of_falling_back() {
        assert_eq!(declared_chip(Some(&status("idle"))), DeclaredChip::Hidden);
    }

    #[test]
    fn displayed_text_drops_every_hidden_control_range_then_keeps_one_line() {
        let ranges: &[(u32, u32)] = &[
            (0x202A, 0x202E),
            (0x2066, 0x2069),
            (0x200B, 0x200F),
            (0x2028, 0x2028),
            (0x2029, 0x2029),
            (0x206A, 0x206F),
        ];
        for (start, end) in ranges {
            for code in *start..=*end {
                let hidden = char::from_u32(code).unwrap();
                assert_eq!(
                    one_line(&format!("Ap{hidden}ply")).as_deref(),
                    Some("Apply"),
                    "U+{code:04X}"
                );
            }
        }
        assert_eq!(
            one_line("first line\nsecond line").as_deref(),
            Some("first line")
        );
        assert_eq!(one_line("first\r\nsecond").as_deref(), Some("first"));
        assert_eq!(one_line("\u{1b}[31mred").as_deref(), Some("[31mred"));
        assert_eq!(one_line("  \u{202E}  "), None);
        assert_eq!(
            one_line(&"é".repeat(LINE_MAX_CHARS + 10)).map(|line| line.chars().count()),
            Some(LINE_MAX_CHARS)
        );
    }

    #[test]
    fn only_blocked_and_error_enter_the_attention_queue() {
        for (wire, queued) in [
            ("working", false),
            ("idle", false),
            ("done", false),
            ("blocked", true),
            ("error", true),
            ("bogus", false),
        ] {
            assert_eq!(
                queue_entry(Some(&status(wire)), "build").is_some(),
                queued,
                "{wire}"
            );
        }
        assert_eq!(queue_entry(None, "build"), None);
    }

    #[test]
    fn a_queue_entry_is_labelled_by_the_app_then_the_pane_with_a_clean_message() {
        let mut blocked = status("blocked");
        blocked.app = "terra\u{202E}form".to_string();
        blocked.message = "Apply\u{2066} the plan?\nyes/no".to_string();
        assert_eq!(
            queue_entry(Some(&blocked), "infra"),
            Some(DeclaredQueueEntry {
                label: "terraform".to_string(),
                message: Some("Apply the plan?".to_string()),
                errored: false,
            })
        );

        let failed = status("error");
        assert_eq!(
            queue_entry(Some(&failed), "backend"),
            Some(DeclaredQueueEntry {
                label: "backend".to_string(),
                message: None,
                errored: true,
            })
        );
        assert_eq!(
            queue_entry(Some(&failed), "").map(|entry| entry.label),
            Some(UNNAMED_PROGRAM.to_string())
        );
    }

    #[test]
    fn the_blocked_notification_names_the_pane_and_carries_the_clean_message() {
        let mut blocked = status("blocked");
        blocked.app = "terraform".to_string();
        blocked.message = "Apply \u{202E}the plan?".to_string();

        assert_eq!(
            blocked_notification_text(&blocked, "infra\u{2066}"),
            (
                "terraform needs input in infra".to_string(),
                "Apply the plan?".to_string()
            )
        );
        assert_eq!(
            blocked_notification_text(&status("blocked"), ""),
            (
                "A program needs input".to_string(),
                BLOCKED_WITHOUT_MESSAGE.to_string()
            )
        );
    }

    #[test]
    fn a_blocked_entry_notifies_once_and_the_watch_forgets_a_removed_record() {
        let mut watch = DeclaredWatch::default();
        let start = Instant::now();
        assert!(!watch.observe(7, Some(DeclaredState::Working), true, start));
        assert!(watch.observe(7, Some(DeclaredState::Blocked), true, start));
        assert!(
            !watch.observe(7, Some(DeclaredState::Blocked), true, start),
            "a status that does not change never notifies again"
        );
        assert_eq!(watch.since(7), Some(start));

        let later = start + Duration::from_secs(1);
        assert!(!watch.observe(7, None, true, later));
        assert_eq!(watch.since(7), None, "a removed record leaves the queue");
    }

    #[test]
    fn at_most_one_notification_per_pane_every_ten_seconds() {
        let mut watch = DeclaredWatch::default();
        let start = Instant::now();
        let mut sent = 0;
        for step in 0..10_u64 {
            let now = start + Duration::from_secs(step);
            sent += usize::from(watch.observe(1, Some(DeclaredState::Working), true, now));
            sent += usize::from(watch.observe(1, Some(DeclaredState::Blocked), true, now));
        }
        assert_eq!(sent, 1);
        let after = start + BLOCKED_NOTIFICATION_INTERVAL;
        watch.observe(1, Some(DeclaredState::Working), true, after);
        assert!(watch.observe(1, Some(DeclaredState::Blocked), true, after));
        assert!(
            watch.observe(2, Some(DeclaredState::Blocked), true, start),
            "another pane keeps its own budget"
        );
    }

    #[test]
    fn a_hundred_alternations_in_one_second_send_one_notification_and_keep_the_final_state() {
        let mut watch = DeclaredWatch::default();
        let start = Instant::now();
        let mut sent = 0;
        let mut last = None;
        for flip in 0..200_u64 {
            let state = if flip % 2 == 0 {
                DeclaredState::Blocked
            } else {
                DeclaredState::Working
            };
            last = Some(state);
            let now = start + Duration::from_millis(flip * 5);
            sent += usize::from(watch.observe(3, Some(state), true, now));
        }
        assert_eq!(sent, 1);
        assert_eq!(last, Some(DeclaredState::Working));
        assert_eq!(
            watch.surfaces.get(&3).and_then(|watched| watched.state),
            last,
            "the queue reads the final state"
        );
        let mut working = status("working");
        working.app = "terraform".to_string();
        assert_eq!(queue_entry(Some(&working), "infra"), None);
    }

    #[test]
    fn an_agent_pane_is_tracked_but_never_notified_by_the_program_path() {
        let mut watch = DeclaredWatch::default();
        let now = Instant::now();
        assert!(!watch.observe(4, Some(DeclaredState::Blocked), false, now));
        assert_eq!(watch.since(4), Some(now));
        for state in [
            DeclaredState::Idle,
            DeclaredState::Working,
            DeclaredState::Blocked,
            DeclaredState::Done,
            DeclaredState::Error,
        ] {
            let mut watch = DeclaredWatch::default();
            assert!(!watch.observe(5, Some(state), false, now), "{state:?}");
        }
    }

    #[test]
    fn only_blocked_and_error_notify_when_a_program_enters_them() {
        for (state, notifies) in [
            (DeclaredState::Idle, false),
            (DeclaredState::Working, false),
            (DeclaredState::Blocked, true),
            (DeclaredState::Done, false),
            (DeclaredState::Error, true),
        ] {
            let mut watch = DeclaredWatch::default();
            assert_eq!(
                watch.observe(1, Some(state), true, Instant::now()),
                notifies,
                "{state:?}"
            );
        }
    }

    #[test]
    fn blocked_and_error_share_one_notification_every_ten_seconds() {
        let mut watch = DeclaredWatch::default();
        let start = Instant::now();
        assert!(watch.observe(1, Some(DeclaredState::Blocked), true, start));
        assert!(
            !watch.observe(
                1,
                Some(DeclaredState::Error),
                true,
                start + Duration::from_secs(1)
            ),
            "an error inside the blocked window stays silent"
        );
        let after = start + BLOCKED_NOTIFICATION_INTERVAL;
        watch.observe(1, Some(DeclaredState::Working), true, after);
        assert!(watch.observe(1, Some(DeclaredState::Error), true, after));
        assert!(
            !watch.observe(1, Some(DeclaredState::Blocked), true, after),
            "a blocked inside the error window stays silent"
        );
    }

    #[test]
    fn the_error_notification_says_the_program_failed_and_names_the_pane() {
        let mut failed = status("error");
        failed.app = "car\u{202E}go".to_string();
        failed.message = "3 tests \u{2066}failed\nsee log".to_string();
        assert_eq!(
            attention_notification_text(&failed, "build"),
            Some((
                "cargo failed in build".to_string(),
                "3 tests failed".to_string()
            ))
        );
        assert_eq!(
            attention_notification_text(&status("error"), ""),
            Some((
                "A program failed".to_string(),
                ERROR_WITHOUT_MESSAGE.to_string()
            ))
        );

        let mut blocked = status("blocked");
        blocked.app = "terraform".to_string();
        blocked.message = "Apply the plan?".to_string();
        assert_eq!(
            attention_notification_text(&blocked, "infra"),
            Some((
                "terraform needs input in infra".to_string(),
                "Apply the plan?".to_string()
            ))
        );
        assert_eq!(
            attention_notification_text(&blocked, "infra"),
            Some(blocked_notification_text(&blocked, "infra"))
        );
        for wire in ["working", "idle", "done", "bogus"] {
            assert_eq!(attention_notification_text(&status(wire), "build"), None);
        }
    }

    #[test]
    fn a_declared_notification_is_held_when_its_pane_is_seen_muted_or_disabled() {
        use crate::agents::notifications::should_fire_desktop_notification;
        use paneflow_config::schema::NotifyWhenAgentWaiting;

        let watching_it = HashSet::from([9_u64]);
        let watching_another = HashSet::from([8_u64]);
        let cases: &[(Option<&HashSet<u64>>, bool, bool)] = &[
            (Some(&watching_it), false, false),
            (Some(&watching_another), true, false),
            (Some(&watching_another), false, true),
            (None, false, true),
            (None, true, false),
        ];
        for (visible, muted, fires) in cases {
            let seen = declared_attention_seen(*visible, 9, *muted);
            assert_eq!(
                should_fire_desktop_notification(NotifyWhenAgentWaiting::PrimaryScreen, seen),
                *fires,
                "{visible:?} muted={muted}"
            );
            assert!(
                !should_fire_desktop_notification(NotifyWhenAgentWaiting::Never, seen),
                "notify_when_agent_waiting = Never holds every declared notification"
            );
        }
    }

    #[test]
    fn fifty_errors_in_one_second_send_one_notification_and_queue_the_last_message() {
        let mut watch = DeclaredWatch::default();
        let start = Instant::now();
        let mut sent = 0;
        let mut last = status("error");
        for step in 0..50_u64 {
            last = status("error");
            last.app = "cargo".to_string();
            last.message = format!("failure {step}");
            let state = DeclaredState::of(&last);
            let now = start + Duration::from_millis(step * 20);
            sent += usize::from(watch.observe(6, state, true, now));
        }
        assert_eq!(sent, 1);
        assert_eq!(
            watch
                .queue_entry(6, Some(&last), "build")
                .and_then(|entry| entry.message),
            Some("failure 49".to_string())
        );
    }

    #[test]
    fn a_seen_error_leaves_the_queue_without_a_new_host_event() {
        let mut watch = DeclaredWatch::default();
        let failed = status("error");
        watch.observe(7, Some(DeclaredState::Error), true, Instant::now());
        assert!(watch.queue_entry(7, Some(&failed), "build").is_some());

        assert!(watch.acknowledge_seen(&HashSet::from([7])));
        assert_eq!(watch.queue_entry(7, Some(&failed), "build"), None);
        assert!(
            !watch.acknowledge_seen(&HashSet::from([7])),
            "an acknowledged entry is not acknowledged twice"
        );
        assert!(!watch.awaits_acknowledgment());
    }

    #[test]
    fn a_blocked_entry_is_never_acknowledged_by_sight() {
        let mut watch = DeclaredWatch::default();
        let blocked = status("blocked");
        watch.observe(7, Some(DeclaredState::Blocked), true, Instant::now());
        assert!(!watch.acknowledge_seen(&HashSet::from([7])));
        assert!(watch.queue_entry(7, Some(&blocked), "infra").is_some());
    }

    #[test]
    fn an_acknowledgment_falls_as_soon_as_the_declared_state_changes() {
        let mut watch = DeclaredWatch::default();
        let failed = status("error");
        let start = Instant::now();
        watch.observe(7, Some(DeclaredState::Error), true, start);
        watch.acknowledge_seen(&HashSet::from([7]));
        assert_eq!(watch.queue_entry(7, Some(&failed), "build"), None);

        watch.observe(7, Some(DeclaredState::Working), true, start);
        watch.observe(7, Some(DeclaredState::Error), true, start);
        assert!(
            watch.queue_entry(7, Some(&failed), "build").is_some(),
            "a new error after a working comes back to the queue"
        );
    }

    #[test]
    fn acknowledging_an_error_keeps_the_chip_on_error() {
        let mut watch = DeclaredWatch::default();
        let mut failed = status("error");
        failed.title = "cargo test".to_string();
        let before = failed.clone();
        watch.observe(7, Some(DeclaredState::Error), true, Instant::now());
        watch.acknowledge_seen(&HashSet::from([7]));
        assert_eq!(failed, before);
        assert_eq!(
            declared_chip(Some(&failed)),
            DeclaredChip::Show {
                label: "error · cargo test".into(),
                error: true,
            }
        );
    }

    #[test]
    fn an_error_never_on_screen_stays_queued() {
        let mut watch = DeclaredWatch::default();
        let failed = status("error");
        watch.observe(7, Some(DeclaredState::Error), true, Instant::now());
        assert!(!watch.acknowledge_seen(&HashSet::from([8])));
        assert!(!watch.acknowledge_seen(&HashSet::new()));
        assert!(watch.awaits_acknowledgment());
        assert!(watch.queue_entry(7, Some(&failed), "build").is_some());
    }

    #[test]
    fn an_error_in_a_detached_pane_stays_queued_while_the_main_window_has_focus() {
        use crate::app::agent_status::pane_on_screen;

        let mut watch = DeclaredWatch::default();
        let failed = status("error");
        watch.observe(7, Some(DeclaredState::Error), true, Instant::now());
        let panes = [(7_u64, Some(false), true), (8, None, true)];
        let visible: HashSet<u64> = panes
            .iter()
            .filter(|(_, detached, in_active_tab)| pane_on_screen(*detached, true, *in_active_tab))
            .map(|(surface_id, _, _)| *surface_id)
            .collect();
        assert_eq!(visible, HashSet::from([8]));
        assert!(!watch.acknowledge_seen(&visible));
        assert!(watch.queue_entry(7, Some(&failed), "build").is_some());
    }

    #[test]
    fn the_declared_status_path_never_writes_to_a_pty() {
        let source = include_str!("declared_status.rs");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("the module has a production part");
        for writer in [
            "inject_text",
            "write_injected_text",
            "write_program_injected_text",
            "send_text",
            "write_text",
            "write_program_text",
            "send_command",
            "send_keystroke",
            "write_to_pty",
            "write_program_input",
            "write_conversation_resume",
            "submit",
            "paste",
            "input(",
        ] {
            assert!(
                !production.contains(writer),
                "the declared status path must never write to a PTY: found {writer}"
            );
        }
    }
}
