use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use paneflow_config::schema::{SessionGeneration, SessionId};
use paneflow_terminal_ghostty::ProgramStatusReport;

use crate::host::{ScanTarget, SessionHost};
use crate::manifest::{HostedSessionRuntime, now_ms};
use crate::process::ProcessIdentity;
use crate::program_status::DeclaredStatus;
use crate::runtime::ViewportScan;
use crate::runtime_observer::{ForegroundCache, ForegroundRuntime, RuntimeObservation};

pub const VIEWPORT_SCAN_INTERVAL: Duration = Duration::from_millis(500);

const VIEWPORT_SCAN_BUDGET: Duration = Duration::from_millis(250);
const OBSERVATION_SETTLE: Duration = Duration::from_millis(1_500);

const SCREEN_STAMP_COALESCE_MS: u64 = 1_000;

#[derive(Debug, Default)]
struct ScreenChangeTracker {
    last_hash: Option<u64>,
    changed_at_ms: Option<u64>,
    written_at_ms: Option<u64>,
    last_write_ms: u64,
}

impl ScreenChangeTracker {
    fn write_pending(&self) -> bool {
        self.changed_at_ms
            .is_some_and(|changed| Some(changed) != self.written_at_ms)
    }

    fn observe(&mut self, hash: u64, now_ms: u64) -> Option<u64> {
        if self.last_hash != Some(hash) {
            self.last_hash = Some(hash);
            self.changed_at_ms = Some(now_ms);
        }
        let pending = self
            .changed_at_ms
            .filter(|changed| Some(*changed) != self.written_at_ms)?;
        if self.written_at_ms.is_some()
            && now_ms.saturating_sub(self.last_write_ms) < SCREEN_STAMP_COALESCE_MS
        {
            return None;
        }
        self.written_at_ms = Some(pending);
        self.last_write_ms = now_ms;
        Some(pending)
    }
}

#[derive(Debug, Default)]
pub struct ViewportTracker {
    screen: ScreenChangeTracker,
    declared_status: Option<DeclaredStatus>,
    scanned_input_at_ms: Option<u64>,
    scanned_resets: u64,
    observation: Option<RuntimeObservation>,
    foreground: ForegroundCache,
    scanned_output_end: Option<u64>,
    terminal_signals: Option<[Option<u64>; 3]>,
    observation_settles_at: Option<Instant>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ScreenView<'a> {
    pub screen: &'a str,
    pub title: Option<&'a str>,
    pub program_status: Option<&'a ProgramStatusReport>,
}

impl<'a> ScreenView<'a> {
    pub fn of(scan: &'a ViewportScan) -> Self {
        Self {
            screen: &scan.screen,
            title: scan.title.as_deref(),
            program_status: scan.program_status.as_ref(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewportEdges {
    pub screen_changed_at_ms: Option<u64>,
    pub declared_status: Option<DeclaredStatus>,
    pub observed_runtime: Option<RuntimeObservation>,
    pub write_due: bool,
}

impl ViewportTracker {
    pub fn terminal_signals_changed(&mut self, signals: [Option<u64>; 3]) -> bool {
        let changed = self.terminal_signals.is_some_and(|held| held != signals);
        self.terminal_signals = Some(signals);
        changed
    }

    pub fn scan_due(&self, output_end: u64) -> bool {
        self.scanned_output_end != Some(output_end) || self.screen.write_pending()
    }

    pub fn observe_foreground(
        &mut self,
        session_leader: ProcessIdentity,
        foreground_process_group: Option<i32>,
    ) -> Option<RuntimeObservation> {
        match self
            .foreground
            .observe(session_leader, foreground_process_group)
        {
            ForegroundRuntime::Observed(observation) => observation,
            ForegroundRuntime::Unobservable => None,
        }
    }

    pub fn observation_settle_due(&self, now: Instant) -> bool {
        self.observation_settles_at.is_some_and(|at| now >= at)
    }

    pub fn schedule_observation_settle(&mut self, output_moved: bool, now: Instant) {
        self.observation_settles_at =
            (output_moved && self.observation.is_none()).then(|| now + OBSERVATION_SETTLE);
    }

    pub fn record_scanned_output(&mut self, output_end: u64) {
        self.scanned_output_end = Some(output_end);
    }

    pub fn declared_rescan_due(&self, input_at_ms: Option<u64>, resets: u64) -> bool {
        let awaits_acknowledgement = self
            .declared_status
            .as_ref()
            .is_some_and(DeclaredStatus::awaits_acknowledgement);
        self.scanned_resets != resets
            || (awaits_acknowledgement && self.scanned_input_at_ms != input_at_ms)
    }

    pub fn record_scanned_terminal(&mut self, input_at_ms: Option<u64>, resets: u64) {
        self.scanned_input_at_ms = input_at_ms;
        self.scanned_resets = resets;
    }

    pub fn observe(
        &mut self,
        view: ScreenView<'_>,
        observation: Option<RuntimeObservation>,
        now_ms: u64,
    ) -> ViewportEdges {
        let screen_changed_at_ms = self.screen.observe(screen_hash(view.screen), now_ms);
        let observation = observation.filter(|observed| observed.confirmed_by_title(view.title));
        let declared_status = view.program_status.map(DeclaredStatus::of);

        let write_due = screen_changed_at_ms.is_some()
            || declared_status != self.declared_status
            || observation != self.observation;

        self.declared_status.clone_from(&declared_status);
        self.observation.clone_from(&observation);

        ViewportEdges {
            screen_changed_at_ms,
            declared_status,
            observed_runtime: observation,
            write_due,
        }
    }
}

fn screen_hash(screen: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    screen.hash(&mut hasher);
    hasher.finish()
}

pub fn spawn(host: &Arc<SessionHost>) {
    let weak = Arc::downgrade(host);
    let spawned = std::thread::Builder::new()
        .name("paneflow-host-viewport".into())
        .spawn(move || scan_loop(weak));
    if let Err(error) = spawned {
        log::warn!("paneflow-host: cannot start the viewport scan: {error}");
    }
}

type TrackerKey = (SessionId, SessionGeneration);

fn scan_loop(host: Weak<SessionHost>) {
    let mut trackers: BTreeMap<TrackerKey, ViewportTracker> = BTreeMap::new();
    loop {
        std::thread::sleep(VIEWPORT_SCAN_INTERVAL);
        let Some(host) = host.upgrade() else {
            return;
        };
        scan_once(&host, &mut trackers);
    }
}

fn scan_once(host: &Arc<SessionHost>, trackers: &mut BTreeMap<TrackerKey, ViewportTracker>) {
    let targets = host.live_scan_targets();
    trackers.retain(|(session, generation), _| {
        targets
            .iter()
            .any(|target| target.session == *session && target.generation == *generation)
    });
    for target in targets {
        let key = (target.session.clone(), target.generation);
        let signals = [
            target.runtime.output_changed_at_ms(),
            target.runtime.bell_at_ms(),
            target.runtime.input_at_ms(),
        ];
        let signals_changed = trackers
            .entry(key.clone())
            .or_default()
            .terminal_signals_changed(signals);
        let announced = scan_target(host, trackers, &target, key);
        if signals_changed && !announced {
            host.announce_session_change(&target.session);
        }
    }
}

fn scan_target(
    host: &Arc<SessionHost>,
    trackers: &mut BTreeMap<TrackerKey, ViewportTracker>,
    target: &ScanTarget,
    key: TrackerKey,
) -> bool {
    let output_end = target.runtime.stream().end_offset();
    let input_at_ms = target.runtime.input_at_ms();
    let resets = target.runtime.resets();
    let now = Instant::now();
    if trackers.get(&key).is_some_and(|tracker| {
        !tracker.scan_due(output_end)
            && !tracker.declared_rescan_due(input_at_ms, resets)
            && !tracker.observation_settle_due(now)
    }) {
        return false;
    }
    let Ok(scan) = target.runtime.viewport_scan(VIEWPORT_SCAN_BUDGET) else {
        return false;
    };
    let tracker = trackers.entry(key).or_default();
    let output_moved = tracker.scan_due(output_end);
    let observation =
        tracker.observe_foreground(target.runtime.process(), scan.foreground_process_group);
    let edges = tracker.observe(ScreenView::of(&scan), observation, now_ms());
    tracker.schedule_observation_settle(output_moved, now);
    tracker.record_scanned_output(output_end);
    tracker.record_scanned_terminal(input_at_ms, resets);
    if !edges.write_due {
        return false;
    }
    host.commit_scan(target, |record| {
        if let Some(stamp) = edges.screen_changed_at_ms {
            record.screen_changed_at_ms = Some(stamp);
        }
        record.declared_status = edges.declared_status;
        let launch_binding = record
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.launch_binding.clone());
        record.runtime = match (edges.observed_runtime, launch_binding) {
            (None, None) => None,
            (current_observation, launch_binding) => Some(HostedSessionRuntime {
                current_observation,
                launch_binding,
            }),
        };
    }) == Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    use paneflow_terminal_ghostty::ProgramStatusState;

    fn observe(
        tracker: &mut ViewportTracker,
        screen: &str,
        observation: Option<RuntimeObservation>,
        now_ms: u64,
    ) -> ViewportEdges {
        tracker.observe(
            ScreenView {
                screen,
                ..ScreenView::default()
            },
            observation,
            now_ms,
        )
    }

    const CLAUDE: &str = "com.anthropic.claude-code";

    fn declared(state: ProgramStatusState, message: &str) -> ProgramStatusReport {
        ProgramStatusReport {
            state,
            kind: None,
            progress: None,
            id: String::new(),
            app: String::new(),
            title: String::new(),
            message: message.to_string(),
        }
    }

    fn observe_declared(
        tracker: &mut ViewportTracker,
        screen: &str,
        report: Option<&ProgramStatusReport>,
        now_ms: u64,
    ) -> ViewportEdges {
        tracker.observe(
            ScreenView {
                screen,
                program_status: report,
                ..ScreenView::default()
            },
            None,
            now_ms,
        )
    }

    #[test]
    fn a_declared_status_is_published_and_written_only_when_it_changes() {
        let mut tracker = ViewportTracker::default();
        let mut blocked = declared(ProgramStatusState::Blocked, "Apply the plan?");
        blocked.kind = Some(paneflow_terminal_ghostty::ProgramStatusKind::Permission);
        blocked.app = "terraform".to_string();

        let edges = observe_declared(&mut tracker, "$ terraform apply", Some(&blocked), 1_000);

        assert_eq!(
            edges.declared_status,
            Some(DeclaredStatus {
                state: "blocked".to_string(),
                kind: Some("permission".to_string()),
                progress: None,
                app: "terraform".to_string(),
                title: String::new(),
                message: "Apply the plan?".to_string(),
            })
        );
        assert!(edges.write_due);

        let edges = observe_declared(&mut tracker, "$ terraform apply", Some(&blocked), 1_200);
        assert!(!edges.write_due, "an unchanged status is not written again");

        let working = declared(ProgramStatusState::Working, "");
        let edges = observe_declared(&mut tracker, "$ terraform apply", Some(&working), 1_400);
        assert_eq!(
            edges.declared_status.map(|status| status.state),
            Some("working".to_string())
        );
        assert!(edges.write_due, "a status change alone is written");

        let edges = observe_declared(&mut tracker, "$ terraform apply", None, 1_600);
        assert_eq!(edges.declared_status, None);
        assert!(edges.write_due, "a removed record is written");
    }

    #[test]
    fn a_held_done_or_error_is_rescanned_after_a_keystroke_without_output() {
        let mut tracker = ViewportTracker::default();
        tracker.record_scanned_terminal(Some(10), 0);
        assert!(
            !tracker.declared_rescan_due(Some(20), 0),
            "nothing to acknowledge"
        );

        let working = declared(ProgramStatusState::Working, "");
        observe_declared(&mut tracker, "$ ", Some(&working), 1_000);
        assert!(
            !tracker.declared_rescan_due(Some(20), 0),
            "a keystroke never releases working"
        );

        for state in [ProgramStatusState::Done, ProgramStatusState::Error] {
            let finished = declared(state, "");
            observe_declared(&mut tracker, "$ ", Some(&finished), 2_000);
            tracker.record_scanned_terminal(Some(10), 0);
            assert!(!tracker.declared_rescan_due(Some(10), 0), "{state:?}");
            assert!(tracker.declared_rescan_due(Some(20), 0), "{state:?}");
            tracker.record_scanned_terminal(Some(20), 0);
            assert!(!tracker.declared_rescan_due(Some(20), 0), "{state:?}");
        }
    }

    #[test]
    fn a_manual_reset_rescans_whatever_the_declared_state() {
        let mut tracker = ViewportTracker::default();
        let blocked = declared(ProgramStatusState::Blocked, "Apply?");
        observe_declared(&mut tracker, "$ ", Some(&blocked), 1_000);
        tracker.record_scanned_terminal(None, 0);
        assert!(!tracker.declared_rescan_due(None, 0));
        assert!(tracker.declared_rescan_due(None, 1));
        tracker.record_scanned_terminal(None, 1);
        assert!(!tracker.declared_rescan_due(None, 1));
    }

    fn claude_observation() -> RuntimeObservation {
        RuntimeObservation {
            id: CLAUDE.to_string(),
            pid: 42,
            pid_started_at: Some(7),
            process_group: 42,
            process_name: "claude".to_string(),
            argv: Some(vec!["claude".to_string()]),
        }
    }

    #[test]
    fn a_session_without_new_output_is_not_rescanned_until_bytes_arrive() {
        let mut tracker = ViewportTracker::default();
        assert!(tracker.scan_due(0));
        observe(&mut tracker, "$ ", None, 1_000);
        tracker.record_scanned_output(0);
        assert!(!tracker.scan_due(0));
        assert!(tracker.scan_due(12));
        tracker.record_scanned_output(12);
        assert!(!tracker.scan_due(12));
    }

    #[test]
    fn a_coalesced_screen_change_is_still_written_after_the_output_stops() {
        let mut tracker = ViewportTracker::default();
        assert_eq!(
            observe(&mut tracker, "first", None, 1_000).screen_changed_at_ms,
            Some(1_000)
        );
        tracker.record_scanned_output(5);
        assert_eq!(
            observe(&mut tracker, "second", None, 1_500).screen_changed_at_ms,
            None
        );
        tracker.record_scanned_output(9);
        assert!(
            tracker.scan_due(9),
            "the coalesced change is pending, so the quiet session is scanned again"
        );
        assert_eq!(
            observe(&mut tracker, "second", None, 2_000).screen_changed_at_ms,
            Some(1_500)
        );
        tracker.record_scanned_output(9);
        assert!(!tracker.scan_due(9));
    }

    #[test]
    fn an_idle_tui_repainting_identical_text_never_advances_the_stamp() {
        let mut tracker = ViewportTracker::default();
        let screen = "❯\n  ⏵⏵ auto mode on";
        let first = observe(&mut tracker, screen, None, 1_000);
        assert_eq!(first.screen_changed_at_ms, Some(1_000));
        assert!(first.write_due);

        let mut writes = 0;
        for tick in 1..=20 {
            let edges = observe(&mut tracker, screen, None, 1_000 + tick * 500);
            assert_eq!(edges.screen_changed_at_ms, None);
            if edges.write_due {
                writes += 1;
            }
        }
        assert_eq!(writes, 0, "a steady screen costs zero manifest writes");
    }

    #[test]
    fn a_changing_screen_stamps_at_most_once_per_second() {
        let mut tracker = ViewportTracker::default();
        let mut stamps = Vec::new();
        for tick in 0..20u64 {
            let screen = format!("❯ frame {tick}");
            let edges = observe(&mut tracker, &screen, None, tick * 100);
            if let Some(stamp) = edges.screen_changed_at_ms {
                stamps.push(stamp);
            }
        }
        assert_eq!(stamps.len(), 2, "{stamps:?}");
        assert!(stamps[1] - stamps[0] >= SCREEN_STAMP_COALESCE_MS);
    }

    fn fx_observation() -> RuntimeObservation {
        RuntimeObservation {
            id: "sh.fx.cli".to_string(),
            pid: 77,
            pid_started_at: Some(9),
            process_group: 77,
            process_name: "fx".to_string(),
            argv: Some(vec!["fx".to_string()]),
        }
    }

    #[test]
    fn an_fx_process_whose_title_is_not_the_agent_title_is_not_an_agent() {
        let mut tracker = ViewportTracker::default();
        let viewer = "{\n  \"name\": \"paneflow\",\n  \"ok\": true\n}\n┃";
        for title in [None, Some("fx data.json"), Some("~/projects/paneflow")] {
            let edges = tracker.observe(
                ScreenView {
                    screen: viewer,
                    title,
                    program_status: None,
                },
                Some(fx_observation()),
                1_000,
            );
            assert_eq!(edges.observed_runtime, None, "{title:?}");
        }

        let agent = tracker.observe(
            ScreenView {
                screen: "𝒇x v0.0.12 · Run /help for commands\n\n┃ hello\n\n• Thinking",
                title: Some("fx v0.0.12 | paneflow"),
                program_status: None,
            },
            Some(fx_observation()),
            2_000,
        );
        assert_eq!(agent.observed_runtime, Some(fx_observation()));
        assert!(agent.write_due);
    }

    #[test]
    fn an_unchanged_observation_is_not_an_edge_and_a_change_writes_once() {
        let mut tracker = ViewportTracker::default();
        let screen = "❯";
        observe(&mut tracker, screen, Some(claude_observation()), 1_000);
        let steady = observe(&mut tracker, screen, Some(claude_observation()), 2_500);
        assert!(!steady.write_due);

        let mut moved = claude_observation();
        moved.pid = 43;
        let changed = observe(&mut tracker, screen, Some(moved.clone()), 4_000);
        assert!(changed.write_due);
        assert_eq!(changed.observed_runtime, Some(moved.clone()));
        let settled = observe(&mut tracker, screen, Some(moved), 5_500);
        assert!(!settled.write_due);
    }

    #[test]
    fn an_unobserved_scan_after_output_rescans_once_the_process_listing_settles() {
        let mut tracker = ViewportTracker::default();
        let scanned = Instant::now();
        tracker.schedule_observation_settle(true, scanned);
        assert!(!tracker.observation_settle_due(scanned));
        let settled = scanned + OBSERVATION_SETTLE;
        assert!(tracker.observation_settle_due(settled));

        tracker.schedule_observation_settle(false, settled);
        assert!(
            !tracker.observation_settle_due(settled + OBSERVATION_SETTLE * 4),
            "the settling rescan is not repeated without new output"
        );
    }
}
