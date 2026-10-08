use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Weak};
use std::time::Duration;

use paneflow_agent_config::runtime_catalog;
use paneflow_agent_config::screen_rules::{ScreenInput, ScreenState, evaluate};
use paneflow_config::schema::{SessionGeneration, SessionId};
use paneflow_terminal_ghostty::{ProgramStatusReport, ProgramStatusState};

use crate::host::{HostError, ScanTarget, SessionHost};
use crate::manifest::{HostedSessionRuntime, now_ms};
use crate::process::ProcessIdentity;
use crate::runtime::ViewportScan;
use crate::runtime_observer::{
    ForegroundCache, ForegroundRuntime, RuntimeObservation, observe_foreground_runtime,
};
use crate::screen_rule_registry::ScreenRuleRegistry;

pub const VIEWPORT_SCAN_INTERVAL: Duration = Duration::from_millis(500);

const VIEWPORT_SCAN_BUDGET: Duration = Duration::from_millis(250);

const SCREEN_STAMP_COALESCE_MS: u64 = 1_000;

pub const BLOCKER_RELEASE_MISSES: u8 = 2;

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
    classified_key: Option<(&'static str, u64, u64)>,
    steady_state: Option<ScreenState>,
    screen_state: Option<ScreenState>,
    screen_activity: Option<String>,
    declared_blocker: Option<String>,
    menu_prompt_active: bool,
    blocker_misses: u8,
    observation: Option<RuntimeObservation>,
    foreground: ForegroundCache,
    scanned_output_end: Option<u64>,
    scanned_rules_generation: Option<u64>,
    terminal_signals: Option<[Option<u64>; 3]>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ScreenView<'a> {
    pub screen: &'a str,
    pub title: Option<&'a str>,
    pub progress: Option<&'a str>,
    pub program_status: Option<&'a ProgramStatusReport>,
}

impl<'a> ScreenView<'a> {
    pub fn of(scan: &'a ViewportScan) -> Self {
        Self {
            screen: &scan.screen,
            title: scan.title.as_deref(),
            progress: scan.progress,
            program_status: scan.program_status.as_ref(),
        }
    }

    fn input(&self) -> ScreenInput<'a> {
        ScreenInput {
            screen: self.screen,
            title: self.title,
            progress: self.progress,
            program_status: self.declared_state(),
        }
    }

    fn declared_state(&self) -> Option<ScreenState> {
        self.program_status
            .map(|report| declared_screen_state(report.state))
    }

    fn declared_blocker(&self) -> Option<String> {
        self.program_status
            .filter(|report| report.state == ProgramStatusState::Blocked)
            .map(|report| report.message.clone())
    }

    fn classification_hash(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.screen.hash(&mut hasher);
        self.title.hash(&mut hasher);
        self.progress.hash(&mut hasher);
        self.declared_state().hash(&mut hasher);
        hasher.finish()
    }
}

fn declared_screen_state(state: ProgramStatusState) -> ScreenState {
    match state {
        ProgramStatusState::Working => ScreenState::Working,
        ProgramStatusState::Blocked => ScreenState::Blocked,
        ProgramStatusState::Idle
        | ProgramStatusState::Done
        | ProgramStatusState::Error
        | ProgramStatusState::Clear => ScreenState::Idle,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewportEdges {
    pub screen_changed_at_ms: Option<u64>,
    pub screen_activity: Option<String>,
    pub declared_blocker: Option<String>,
    pub menu_prompt_active: bool,
    pub observed_runtime: Option<RuntimeObservation>,
    pub write_due: bool,
}

impl ViewportTracker {
    pub fn terminal_signals_changed(&mut self, signals: [Option<u64>; 3]) -> bool {
        let changed = self.terminal_signals.is_some_and(|held| held != signals);
        self.terminal_signals = Some(signals);
        changed
    }

    pub fn scan_due(&self, output_end: u64, rules_generation: u64) -> bool {
        self.scanned_output_end != Some(output_end)
            || self.screen.write_pending()
            || self.blocker_releasing()
            || self
                .scanned_rules_generation
                .is_some_and(|scanned| scanned != rules_generation)
    }

    pub fn observe_foreground(
        &mut self,
        session_leader: ProcessIdentity,
        foreground_process_group: Option<i32>,
    ) -> ForegroundRuntime {
        self.foreground
            .observe(session_leader, foreground_process_group)
    }

    pub fn record_scanned_output(&mut self, output_end: u64, rules_generation: u64) {
        self.scanned_output_end = Some(output_end);
        self.scanned_rules_generation = Some(rules_generation);
    }

    fn blocker_releasing(&self) -> bool {
        self.menu_prompt_active && self.blocker_misses > 0
    }

    fn classify(&mut self, state: Option<ScreenState>, visible_blocker: bool) {
        match state {
            Some(ScreenState::Blocked) => self.screen_state = Some(ScreenState::Blocked),
            Some(steady) => {
                self.steady_state = Some(steady);
                self.screen_state = Some(steady);
            }
            None => self.screen_state = self.steady_state,
        }
        if visible_blocker {
            self.menu_prompt_active = true;
            self.blocker_misses = 0;
        } else if self.menu_prompt_active {
            self.blocker_misses += 1;
            if self.blocker_misses >= BLOCKER_RELEASE_MISSES {
                self.menu_prompt_active = false;
                self.blocker_misses = 0;
            }
        }
    }

    fn forget_classification(&mut self) {
        self.classified_key = None;
        self.steady_state = None;
        self.screen_state = None;
        self.menu_prompt_active = false;
        self.blocker_misses = 0;
    }

    pub fn observe(
        &mut self,
        view: ScreenView<'_>,
        observation: Option<RuntimeObservation>,
        declared_tool: Option<&str>,
        registry: &ScreenRuleRegistry,
        now_ms: u64,
    ) -> ViewportEdges {
        let hash = screen_hash(view.screen);
        let screen_changed_at_ms = self.screen.observe(hash, now_ms);
        let previous_blocker = self.menu_prompt_active;
        let observation = observation.filter(|observed| observed.confirmed_by_title(view.title));

        let runtime = runtime_for(observation.as_ref(), declared_tool);
        match runtime.and_then(|runtime| {
            registry
                .rules_for(runtime.slug)
                .map(|rules| (runtime.id, rules))
        }) {
            None => self.forget_classification(),
            Some((runtime_id, rules)) => {
                let key = (
                    runtime_id,
                    view.classification_hash(),
                    registry.generation(),
                );
                if self.classified_key != Some(key) || self.blocker_releasing() {
                    self.classified_key = Some(key);
                    let evaluation = evaluate(&rules, &view.input());
                    self.classify(
                        evaluation.state(&rules),
                        evaluation.visible_blocker.is_some(),
                    );
                }
            }
        }
        let screen_activity = self.screen_state.map(|state| state.as_str().to_string());
        let declared_blocker = self
            .classified_key
            .is_some()
            .then(|| view.declared_blocker())
            .flatten();
        let menu_prompt_active = self.menu_prompt_active;

        let write_due = screen_changed_at_ms.is_some()
            || menu_prompt_active != previous_blocker
            || screen_activity != self.screen_activity
            || declared_blocker != self.declared_blocker
            || observation != self.observation;

        self.screen_activity.clone_from(&screen_activity);
        self.declared_blocker.clone_from(&declared_blocker);
        self.observation.clone_from(&observation);

        ViewportEdges {
            screen_changed_at_ms,
            screen_activity,
            declared_blocker,
            menu_prompt_active,
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

fn runtime_for(
    observation: Option<&RuntimeObservation>,
    declared_tool: Option<&str>,
) -> Option<&'static paneflow_agent_config::runtime_catalog::Runtime> {
    observation
        .and_then(RuntimeObservation::runtime)
        .or_else(|| declared_tool.and_then(runtime_catalog::runtime_for_tool))
}

fn foreground_evidence(
    foreground: ForegroundRuntime,
    declared_tool: impl FnOnce() -> Option<String>,
) -> (Option<RuntimeObservation>, Option<String>) {
    match foreground {
        ForegroundRuntime::Observed(observation) => (observation, None),
        ForegroundRuntime::Unobservable => (None, declared_tool()),
    }
}

pub struct ViewportCapture {
    pub scan: ViewportScan,
    pub runtime: Option<&'static paneflow_agent_config::runtime_catalog::Runtime>,
}

pub fn capture(host: &SessionHost, session: &SessionId) -> Result<ViewportCapture, HostError> {
    let target = host.scan_target(session)?;
    let scan = target.runtime.viewport_scan(VIEWPORT_SCAN_BUDGET)?;
    let (observation, declared_tool) = foreground_evidence(
        observe_foreground_runtime(target.runtime.process(), scan.foreground_process_group),
        || declared_tool(&target),
    );
    let observation =
        observation.filter(|observed| observed.confirmed_by_title(scan.title.as_deref()));
    let runtime = runtime_for(observation.as_ref(), declared_tool.as_deref());
    Ok(ViewportCapture { scan, runtime })
}

pub fn explain(
    host: &SessionHost,
    session: &SessionId,
    now_ms: u64,
) -> Result<serde_json::Value, HostError> {
    let capture = capture(host, session)?;
    let manifest = host.inspect(session)?.manifest;
    let last_hook = manifest.last_hook.as_ref().map(|hook| {
        serde_json::json!({
            "event": hook.hook_event_name,
            "tool": hook.tool,
            "age_ms": now_ms.saturating_sub(hook.received_at_ms),
            "current_generation": hook.runtime_generation == manifest.generation,
        })
    });
    let mut explained = serde_json::json!({
        "session": session,
        "runtime_id": capture.runtime.map(|runtime| runtime.id),
        "runtime_slug": capture.runtime.map(|runtime| runtime.slug),
        "runtime_label": capture.runtime.map(|runtime| runtime.label),
        "title": capture.scan.title,
        "progress": capture.scan.progress,
        "program_status": crate::program_status::to_json(capture.scan.program_status.as_ref()),
        "tracked_screen_activity": manifest.screen_activity,
        "tracked_visible_blocker": manifest.menu_prompt_active,
        "last_hook": last_hook,
        "rules": [],
        "winner": null,
        "screen_state": null,
        "visible_blocker": null,
    });
    let Some(runtime) = capture.runtime else {
        return Ok(explained);
    };
    let registry = host.screen_rules();
    let status = registry.status(runtime.slug);
    explained["sources"] = serde_json::json!({
        "remote_version": status.remote_version,
        "remote_rejection": status.remote_rejection,
        "local_path": status.local_path,
        "local_error": status.local_error,
    });
    let Some(rules) = registry.rules_for(runtime.slug) else {
        return Ok(explained);
    };
    let evaluation = evaluate(&rules, &ScreenView::of(&capture.scan).input());
    explained["rules"] = rules
        .iter()
        .zip(&evaluation.matched)
        .map(|(rule, matched)| {
            serde_json::json!({
                "id": rule.id,
                "state": rule.state.as_str(),
                "priority": rule.priority,
                "region": rule.region.to_string(),
                "origin": rule.origin.to_string(),
                "visible_blocker": rule.visible_blocker,
                "matched": matched,
            })
        })
        .collect();
    explained["winner"] = serde_json::json!(evaluation.winner.map(|index| &rules[index].id));
    explained["screen_state"] =
        serde_json::json!(evaluation.state(&rules).map(ScreenState::as_str));
    explained["visible_blocker"] =
        serde_json::json!(evaluation.visible_blocker.map(|index| &rules[index].id));
    Ok(explained)
}

fn declared_tool(target: &ScanTarget) -> Option<String> {
    target
        .manifest
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .last_hook
        .as_ref()
        .map(|hook| hook.tool.clone())
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
        host.reload_local_screen_rules();
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
    let rules_generation = host.screen_rules().generation();
    if trackers
        .get(&key)
        .is_some_and(|tracker| !tracker.scan_due(output_end, rules_generation))
    {
        return false;
    }
    let Ok(scan) = target.runtime.viewport_scan(VIEWPORT_SCAN_BUDGET) else {
        return false;
    };
    let tracker = trackers.entry(key).or_default();
    let (observation, declared_tool) = foreground_evidence(
        tracker.observe_foreground(target.runtime.process(), scan.foreground_process_group),
        || declared_tool(target),
    );
    let edges = tracker.observe(
        ScreenView::of(&scan),
        observation,
        declared_tool.as_deref(),
        host.screen_rules(),
        now_ms(),
    );
    tracker.record_scanned_output(output_end, rules_generation);
    if !edges.write_due {
        return false;
    }
    host.commit_scan(target, |record| {
        if let Some(stamp) = edges.screen_changed_at_ms {
            record.screen_changed_at_ms = Some(stamp);
        }
        record.screen_activity = edges.screen_activity;
        record.declared_blocker = edges.declared_blocker;
        record.menu_prompt_active = edges.menu_prompt_active;
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

    use std::sync::LazyLock;

    static REGISTRY: LazyLock<ScreenRuleRegistry> = LazyLock::new(ScreenRuleRegistry::with_builtin);

    fn observe(
        tracker: &mut ViewportTracker,
        screen: &str,
        observation: Option<RuntimeObservation>,
        declared_tool: Option<&str>,
        now_ms: u64,
    ) -> ViewportEdges {
        tracker.observe(
            ScreenView {
                screen,
                ..ScreenView::default()
            },
            observation,
            declared_tool,
            &REGISTRY,
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
        declared_tool: Option<&str>,
        now_ms: u64,
    ) -> ViewportEdges {
        tracker.observe(
            ScreenView {
                screen,
                program_status: report,
                ..ScreenView::default()
            },
            None,
            declared_tool,
            &REGISTRY,
            now_ms,
        )
    }

    #[test]
    fn a_declared_program_status_decides_the_screen_state_before_text_rules() {
        let working_screen = "✽ Levitating… (1m 52s)\nesc to interrupt\n❯";
        let mut tracker = ViewportTracker::default();
        assert_eq!(
            observe_declared(&mut tracker, working_screen, None, Some("claude"), 1_000)
                .screen_activity,
            Some("working".to_string())
        );

        for (state, expected) in [
            (ProgramStatusState::Idle, "idle"),
            (ProgramStatusState::Done, "idle"),
            (ProgramStatusState::Error, "idle"),
            (ProgramStatusState::Blocked, "blocked"),
            (ProgramStatusState::Working, "working"),
        ] {
            let report = declared(state, "");
            let edges = observe_declared(
                &mut tracker,
                working_screen,
                Some(&report),
                Some("claude"),
                2_000,
            );
            assert_eq!(
                edges.screen_activity.as_deref(),
                Some(expected),
                "{state:?}"
            );
        }
    }

    #[test]
    fn a_declared_blocker_carries_its_message_and_clears_with_the_record() {
        let mut tracker = ViewportTracker::default();
        let blocked = declared(ProgramStatusState::Blocked, "Apply?");

        let edges = observe_declared(&mut tracker, "plain", Some(&blocked), Some("claude"), 1_000);
        assert_eq!(edges.screen_activity.as_deref(), Some("blocked"));
        assert_eq!(edges.declared_blocker.as_deref(), Some("Apply?"));
        assert!(!edges.menu_prompt_active);
        assert!(edges.write_due);

        let working = declared(ProgramStatusState::Working, "");
        let edges = observe_declared(&mut tracker, "plain", Some(&working), Some("claude"), 1_500);
        assert_eq!(edges.screen_activity.as_deref(), Some("working"));
        assert_eq!(edges.declared_blocker, None);
        assert!(edges.write_due, "a status change alone is written");
    }

    #[test]
    fn a_declared_program_status_needs_a_recognized_runtime() {
        let mut tracker = ViewportTracker::default();
        let blocked = declared(ProgramStatusState::Blocked, "Apply?");

        let edges = observe_declared(&mut tracker, "plain", Some(&blocked), None, 1_000);

        assert_eq!(edges.screen_activity, None);
        assert_eq!(edges.declared_blocker, None);
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
        assert!(tracker.scan_due(0, 0));
        observe(&mut tracker, "$ ", None, None, 1_000);
        tracker.record_scanned_output(0, 0);
        assert!(!tracker.scan_due(0, 0));
        assert!(tracker.scan_due(12, 0));
        tracker.record_scanned_output(12, 0);
        assert!(!tracker.scan_due(12, 0));
    }

    #[test]
    fn a_coalesced_screen_change_is_still_written_after_the_output_stops() {
        let mut tracker = ViewportTracker::default();
        assert_eq!(
            observe(&mut tracker, "first", None, None, 1_000).screen_changed_at_ms,
            Some(1_000)
        );
        tracker.record_scanned_output(5, 0);
        assert_eq!(
            observe(&mut tracker, "second", None, None, 1_500).screen_changed_at_ms,
            None
        );
        tracker.record_scanned_output(9, 0);
        assert!(
            tracker.scan_due(9, 0),
            "the coalesced change is pending, so the quiet session is scanned again"
        );
        assert_eq!(
            observe(&mut tracker, "second", None, None, 2_000).screen_changed_at_ms,
            Some(1_500)
        );
        tracker.record_scanned_output(9, 0);
        assert!(!tracker.scan_due(9, 0));
    }

    #[test]
    fn an_idle_tui_repainting_identical_text_never_advances_the_stamp() {
        let mut tracker = ViewportTracker::default();
        let screen = "❯\n  ⏵⏵ auto mode on";
        let first = observe(&mut tracker, screen, None, Some("claude"), 1_000);
        assert_eq!(first.screen_changed_at_ms, Some(1_000));
        assert!(first.write_due);

        let mut writes = 0;
        for tick in 1..=20 {
            let edges = observe(
                &mut tracker,
                screen,
                None,
                Some("claude"),
                1_000 + tick * 500,
            );
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
            let edges = observe(&mut tracker, &screen, None, Some("claude"), tick * 100);
            if let Some(stamp) = edges.screen_changed_at_ms {
                stamps.push(stamp);
            }
        }
        assert_eq!(stamps.len(), 2, "{stamps:?}");
        assert!(stamps[1] - stamps[0] >= SCREEN_STAMP_COALESCE_MS);
    }

    #[test]
    fn the_declared_screen_tier_classifies_only_when_the_text_changed() {
        let mut tracker = ViewportTracker::default();
        let working = "✽ Levitating… (1m 52s)\nesc to interrupt\n❯";
        assert_eq!(
            observe(&mut tracker, working, None, Some("claude"), 1_000).screen_activity,
            Some("working".to_string())
        );
        let idle = "✻ Brewed for 3s · done\n❯";
        assert_eq!(
            observe(&mut tracker, idle, None, Some("claude"), 3_000).screen_activity,
            Some("idle".to_string())
        );
        let unknown = "some unrelated shell output";
        let held = observe(&mut tracker, unknown, None, Some("claude"), 5_000);
        assert_eq!(
            held.screen_activity,
            Some("idle".to_string()),
            "an unrecognized screen leaves the previous verdict alone"
        );
    }

    #[test]
    fn a_runtime_without_screen_rules_clears_the_verdict() {
        let mut tracker = ViewportTracker::default();
        let working = "✽ Levitating… (1m 52s)\nesc to interrupt\n❯";
        assert_eq!(
            observe(&mut tracker, working, None, Some("claude"), 1_000).screen_activity,
            Some("working".to_string())
        );
        let cleared = observe(&mut tracker, working, None, Some("amp"), 2_500);
        assert_eq!(cleared.screen_activity, None);
        assert!(cleared.write_due);

        let unrecognized = observe(&mut tracker, working, None, None, 4_000);
        assert_eq!(unrecognized.screen_activity, None);
        assert!(!unrecognized.write_due);
    }

    #[test]
    fn the_observed_runtime_supplies_the_rules_when_no_hook_ever_fired() {
        let mut tracker = ViewportTracker::default();
        let working = "✽ Levitating… (1m 52s)\nesc to interrupt\n❯";
        let edges = observe(
            &mut tracker,
            working,
            Some(claude_observation()),
            None,
            1_000,
        );
        assert_eq!(edges.screen_activity, Some("working".to_string()));
        assert_eq!(edges.observed_runtime, Some(claude_observation()));
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
                    progress: None,
                    program_status: None,
                },
                Some(fx_observation()),
                None,
                &REGISTRY,
                1_000,
            );
            assert_eq!(edges.observed_runtime, None, "{title:?}");
            assert_eq!(edges.screen_activity, None, "{title:?}");
            assert!(!edges.menu_prompt_active, "{title:?}");
        }

        let agent = tracker.observe(
            ScreenView {
                screen: "𝒇x v0.0.12 · Run /help for commands\n\n┃ hello\n\n• Thinking\n\n┃\n\nauto · grok-4.7",
                title: Some("fx v0.0.12 | paneflow"),
                progress: None,
                program_status: None,
            },
            Some(fx_observation()),
            None,
            &REGISTRY,
            2_000,
        );
        assert_eq!(agent.observed_runtime, Some(fx_observation()));
        assert_eq!(agent.screen_activity, Some("working".to_string()));
        assert!(agent.write_due);
    }

    #[test]
    fn an_unchanged_observation_is_not_an_edge_and_a_change_writes_once() {
        let mut tracker = ViewportTracker::default();
        let screen = "❯";
        observe(
            &mut tracker,
            screen,
            Some(claude_observation()),
            None,
            1_000,
        );
        let steady = observe(
            &mut tracker,
            screen,
            Some(claude_observation()),
            None,
            2_500,
        );
        assert!(!steady.write_due);

        let mut moved = claude_observation();
        moved.pid = 43;
        let changed = observe(&mut tracker, screen, Some(moved.clone()), None, 4_000);
        assert!(changed.write_due);
        assert_eq!(changed.observed_runtime, Some(moved.clone()));
        let settled = observe(&mut tracker, screen, Some(moved), None, 5_500);
        assert!(!settled.write_due);
    }

    #[test]
    fn changing_runtime_reclassifies_an_unchanged_screen_with_the_new_rules() {
        let mut tracker = ViewportTracker::default();
        let screen = "› Ask Codex to do anything";
        let mut claude = claude_observation();
        assert_eq!(
            observe(&mut tracker, screen, Some(claude.clone()), None, 1_000).screen_activity,
            None
        );

        claude.id = "com.openai.codex".to_string();
        let changed = observe(&mut tracker, screen, Some(claude), None, 2_000);
        assert_eq!(changed.screen_activity, Some("idle".to_string()));
        assert!(changed.write_due);
    }

    #[test]
    fn a_menu_flip_is_edge_written_in_both_directions() {
        let mut tracker = ViewportTracker::default();
        let quiet = "❯ waiting for a prompt";
        observe(&mut tracker, quiet, None, Some("claude"), 1_000);
        let menu = "❯ 1. Yes\n  2. No\n\nEnter to select · ↑/↓ to navigate · Esc to cancel";
        let asking = observe(&mut tracker, menu, None, Some("claude"), 2_500);
        assert!(asking.menu_prompt_active);
        assert!(asking.write_due);
        let steady = observe(&mut tracker, menu, None, Some("claude"), 4_000);
        assert!(steady.menu_prompt_active);
        assert!(!steady.write_due);
        let answered = observe(&mut tracker, quiet, None, Some("claude"), 5_500);
        assert!(
            answered.menu_prompt_active,
            "one evaluation without the blocker does not release it"
        );
        tracker.record_scanned_output(7, 0);
        assert!(
            tracker.scan_due(7, 0),
            "a releasing blocker is scanned again"
        );
        let released = observe(&mut tracker, quiet, None, Some("claude"), 6_000);
        assert!(!released.menu_prompt_active);
        assert!(released.write_due);
        tracker.record_scanned_output(7, 0);
        assert!(!tracker.scan_due(7, 0));
    }

    #[test]
    fn a_visible_blocker_survives_one_miss_and_is_released_by_the_second() {
        let mut tracker = ViewportTracker::default();
        let menu = "❯ 1. Yes\n  2. No\n\nEnter to select · ↑/↓ to navigate · Esc to cancel";
        assert!(observe(&mut tracker, menu, None, Some("claude"), 1_000).menu_prompt_active);
        let flicker = "redrawing";
        assert!(observe(&mut tracker, flicker, None, Some("claude"), 1_500).menu_prompt_active);
        assert!(
            observe(&mut tracker, menu, None, Some("claude"), 2_000).menu_prompt_active,
            "a returning blocker resets the misses"
        );
        assert!(observe(&mut tracker, flicker, None, Some("claude"), 2_500).menu_prompt_active);
        assert!(!observe(&mut tracker, flicker, None, Some("claude"), 3_000).menu_prompt_active);
    }

    #[test]
    fn a_shell_pane_showing_the_npm_init_menu_raises_no_agent_blocker() {
        let mut tracker = ViewportTracker::default();
        let npm = "$ npm init\n? Select a package manager › - Use arrow-keys. Return to submit.\n\
                   ❯   npm\n    yarn\n    pnpm\n↑/↓ to navigate · enter to select";
        let edges = observe(&mut tracker, npm, None, None, 1_000);
        assert!(!edges.menu_prompt_active);
        assert_eq!(edges.screen_activity, None);
        let claude = observe(&mut tracker, npm, None, Some("claude"), 2_500);
        assert!(
            claude.menu_prompt_active,
            "the same footer under a recognized agent is a blocker, so only the runtime gate keeps the shell quiet"
        );
    }

    #[test]
    fn a_hook_left_by_an_agent_that_exited_arms_no_rule_over_the_observed_shell() {
        let menu = "$ npm init\nPress enter to confirm or esc to cancel";
        let claude_hook = || Some("claude".to_string());
        let mut tracker = ViewportTracker::default();
        let (observation, declared) =
            foreground_evidence(ForegroundRuntime::Observed(None), claude_hook);
        let edges = observe(&mut tracker, menu, observation, declared.as_deref(), 1_000);
        assert!(!edges.menu_prompt_active);
        assert_eq!(edges.screen_activity, None);

        let (observation, declared) =
            foreground_evidence(ForegroundRuntime::Unobservable, claude_hook);
        assert!(
            observe(&mut tracker, menu, observation, declared.as_deref(), 2_500).menu_prompt_active,
            "only an unobservable foreground falls back to the tool the hooks declared"
        );

        let (observation, declared) = foreground_evidence(
            ForegroundRuntime::Observed(Some(claude_observation())),
            claude_hook,
        );
        assert_eq!(declared, None);
        assert_eq!(observation, Some(claude_observation()));
    }

    #[test]
    fn a_blocked_screen_verdict_falls_back_to_the_steady_state_once_unrecognized() {
        let mut tracker = ViewportTracker::default();
        let idle = "✻ Brewed for 3s · done\n❯";
        assert_eq!(
            observe(&mut tracker, idle, None, Some("claude"), 1_000).screen_activity,
            Some("idle".to_string())
        );
        let menu = "Do you want to proceed?\n❯ 1. Yes\n  2. No\n\nEsc to cancel · Tab to amend";
        assert_eq!(
            observe(&mut tracker, menu, None, Some("claude"), 2_000).screen_activity,
            Some("blocked".to_string())
        );
        assert_eq!(
            observe(&mut tracker, "plain output", None, Some("claude"), 3_000).screen_activity,
            Some("idle".to_string())
        );
    }

    #[test]
    fn the_osc_title_and_progress_reach_the_rules() {
        let home = tempfile::tempdir().unwrap();
        let registry = ScreenRuleRegistry::with_builtin();
        let dir = paneflow_home::screen_rule_overrides_dir_in(home.path()).join("claude-code");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(crate::screen_rule_registry::LOCAL_RULES_FILE),
            "engine = 2\n[[rules]]\nid = \"titled\"\nstate = \"working\"\npriority = 40\nregion = \"title\"\nany = ['^✳ ']\nprogress = ['indeterminate']\n",
        )
        .unwrap();
        registry.reload_local(home.path());
        let mut tracker = ViewportTracker::default();
        let view = |title, progress| ScreenView {
            screen: "❯",
            title,
            progress,
            program_status: None,
        };
        let observe_view = |tracker: &mut ViewportTracker, title, progress, now| {
            tracker
                .observe(view(title, progress), None, Some("claude"), &registry, now)
                .screen_activity
        };
        assert_eq!(
            observe_view(
                &mut tracker,
                Some("✳ Fix the build"),
                Some("indeterminate"),
                1_000
            ),
            Some("working".to_string())
        );
        assert_eq!(
            observe_view(&mut tracker, Some("✳ Fix the build"), None, 2_000),
            Some("idle".to_string()),
            "an unchanged screen is reclassified when only the progress changes"
        );
        assert_eq!(
            observe_view(
                &mut tracker,
                Some("Claude Code"),
                Some("indeterminate"),
                3_000
            ),
            Some("idle".to_string())
        );
    }

    #[test]
    fn a_rule_reload_rescans_a_quiet_session() {
        let mut tracker = ViewportTracker::default();
        observe(&mut tracker, "❯", None, Some("claude"), 1_000);
        tracker.record_scanned_output(4, 1);
        assert!(!tracker.scan_due(4, 1));
        assert!(tracker.scan_due(4, 2));
    }
}
