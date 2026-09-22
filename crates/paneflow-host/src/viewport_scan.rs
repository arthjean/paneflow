use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Weak};
use std::time::Duration;

use paneflow_agent_config::runtime_catalog;
use paneflow_config::schema::{SessionGeneration, SessionId};

use crate::host::SessionHost;
use crate::manifest::{HostedSessionRuntime, now_ms};
use crate::menu_prompt::viewport_has_menu_prompt;
use crate::runtime_observer::{RuntimeObservation, observe_foreground_runtime};
use crate::screen_activity;

pub const VIEWPORT_SCAN_INTERVAL: Duration = Duration::from_millis(500);

const VIEWPORT_SCAN_BUDGET: Duration = Duration::from_millis(250);

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
    classified_key: Option<(&'static str, u64)>,
    screen_activity: Option<String>,
    menu_prompt_active: bool,
    observation: Option<RuntimeObservation>,
    scanned_output_end: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewportEdges {
    pub screen_changed_at_ms: Option<u64>,
    pub screen_activity: Option<String>,
    pub menu_prompt_active: bool,
    pub observed_runtime: Option<RuntimeObservation>,
    pub write_due: bool,
}

impl ViewportTracker {
    pub fn scan_due(&self, output_end: u64) -> bool {
        self.scanned_output_end != Some(output_end) || self.screen.write_pending()
    }

    pub fn record_scanned_output(&mut self, output_end: u64) {
        self.scanned_output_end = Some(output_end);
    }

    pub fn observe(
        &mut self,
        screen: &str,
        observation: Option<RuntimeObservation>,
        declared_tool: Option<&str>,
        now_ms: u64,
    ) -> ViewportEdges {
        let hash = screen_hash(screen);
        let screen_changed_at_ms = self.screen.observe(hash, now_ms);

        let menu_prompt_active = viewport_has_menu_prompt(screen);

        let runtime = runtime_for(observation.as_ref(), declared_tool);
        let screen_activity = match runtime.and_then(|runtime| {
            screen_activity::rules_for_runtime(runtime).map(|rules| (runtime.id, rules))
        }) {
            None => {
                self.classified_key = None;
                None
            }
            Some((runtime_id, rules)) if self.classified_key != Some((runtime_id, hash)) => {
                self.classified_key = Some((runtime_id, hash));
                screen_activity::classify(screen, &rules)
                    .map(|activity| activity.as_str().to_string())
                    .or_else(|| self.screen_activity.clone())
            }
            Some(_) => self.screen_activity.clone(),
        };

        let write_due = screen_changed_at_ms.is_some()
            || menu_prompt_active != self.menu_prompt_active
            || screen_activity != self.screen_activity
            || observation != self.observation;

        self.menu_prompt_active = menu_prompt_active;
        self.screen_activity.clone_from(&screen_activity);
        self.observation.clone_from(&observation);

        ViewportEdges {
            screen_changed_at_ms,
            screen_activity,
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
        let output_end = target.runtime.stream().end_offset();
        if trackers
            .get(&key)
            .is_some_and(|tracker| !tracker.scan_due(output_end))
        {
            continue;
        }
        let Ok(scan) = target.runtime.viewport_scan(VIEWPORT_SCAN_BUDGET) else {
            continue;
        };
        let observation =
            observe_foreground_runtime(target.runtime.process(), scan.foreground_process_group);
        let declared_tool = target
            .manifest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last_hook
            .as_ref()
            .map(|hook| hook.tool.clone());
        let tracker = trackers.entry(key).or_default();
        let edges = tracker.observe(
            &scan.screen,
            observation,
            declared_tool.as_deref(),
            now_ms(),
        );
        tracker.record_scanned_output(output_end);
        if !edges.write_due {
            continue;
        }
        host.commit_scan(&target, |record| {
            if let Some(stamp) = edges.screen_changed_at_ms {
                record.screen_changed_at_ms = Some(stamp);
            }
            record.screen_activity = edges.screen_activity;
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
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE: &str = "com.anthropic.claude-code";

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
        tracker.observe("$ ", None, None, 1_000);
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
            tracker
                .observe("first", None, None, 1_000)
                .screen_changed_at_ms,
            Some(1_000)
        );
        tracker.record_scanned_output(5);
        assert_eq!(
            tracker
                .observe("second", None, None, 1_500)
                .screen_changed_at_ms,
            None
        );
        tracker.record_scanned_output(9);
        assert!(
            tracker.scan_due(9),
            "the coalesced change is pending, so the quiet session is scanned again"
        );
        assert_eq!(
            tracker
                .observe("second", None, None, 2_000)
                .screen_changed_at_ms,
            Some(1_500)
        );
        tracker.record_scanned_output(9);
        assert!(!tracker.scan_due(9));
    }

    #[test]
    fn an_idle_tui_repainting_identical_text_never_advances_the_stamp() {
        let mut tracker = ViewportTracker::default();
        let screen = "❯\n  ⏵⏵ auto mode on";
        let first = tracker.observe(screen, None, Some("claude"), 1_000);
        assert_eq!(first.screen_changed_at_ms, Some(1_000));
        assert!(first.write_due);

        let mut writes = 0;
        for tick in 1..=20 {
            let edges = tracker.observe(screen, None, Some("claude"), 1_000 + tick * 500);
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
            let edges = tracker.observe(&screen, None, Some("claude"), tick * 100);
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
            tracker
                .observe(working, None, Some("claude"), 1_000)
                .screen_activity,
            Some("working".to_string())
        );
        let idle = "✻ Brewed for 3s · done\n❯";
        assert_eq!(
            tracker
                .observe(idle, None, Some("claude"), 3_000)
                .screen_activity,
            Some("idle".to_string())
        );
        let unknown = "some unrelated shell output";
        let held = tracker.observe(unknown, None, Some("claude"), 5_000);
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
            tracker
                .observe(working, None, Some("claude"), 1_000)
                .screen_activity,
            Some("working".to_string())
        );
        let cleared = tracker.observe(working, None, Some("pi"), 2_500);
        assert_eq!(cleared.screen_activity, None);
        assert!(cleared.write_due);

        let unrecognized = tracker.observe(working, None, None, 4_000);
        assert_eq!(unrecognized.screen_activity, None);
        assert!(!unrecognized.write_due);
    }

    #[test]
    fn the_observed_runtime_supplies_the_rules_when_no_hook_ever_fired() {
        let mut tracker = ViewportTracker::default();
        let working = "✽ Levitating… (1m 52s)\nesc to interrupt\n❯";
        let edges = tracker.observe(working, Some(claude_observation()), None, 1_000);
        assert_eq!(edges.screen_activity, Some("working".to_string()));
        assert_eq!(edges.observed_runtime, Some(claude_observation()));
    }

    #[test]
    fn an_unchanged_observation_is_not_an_edge_and_a_change_writes_once() {
        let mut tracker = ViewportTracker::default();
        let screen = "❯";
        tracker.observe(screen, Some(claude_observation()), None, 1_000);
        let steady = tracker.observe(screen, Some(claude_observation()), None, 2_500);
        assert!(!steady.write_due);

        let mut moved = claude_observation();
        moved.pid = 43;
        let changed = tracker.observe(screen, Some(moved.clone()), None, 4_000);
        assert!(changed.write_due);
        assert_eq!(changed.observed_runtime, Some(moved.clone()));
        let settled = tracker.observe(screen, Some(moved), None, 5_500);
        assert!(!settled.write_due);
    }

    #[test]
    fn changing_runtime_reclassifies_an_unchanged_screen_with_the_new_rules() {
        let mut tracker = ViewportTracker::default();
        let screen = "› Ask Codex to do anything";
        let mut claude = claude_observation();
        assert_eq!(
            tracker
                .observe(screen, Some(claude.clone()), None, 1_000)
                .screen_activity,
            None
        );

        claude.id = "com.openai.codex".to_string();
        let changed = tracker.observe(screen, Some(claude), None, 2_000);
        assert_eq!(changed.screen_activity, Some("idle".to_string()));
        assert!(changed.write_due);
    }

    #[test]
    fn a_menu_flip_is_edge_written_in_both_directions() {
        let mut tracker = ViewportTracker::default();
        let quiet = "❯ waiting for a prompt";
        tracker.observe(quiet, None, Some("claude"), 1_000);
        let menu = "❯ 1. Yes\n  2. No\n\nEnter to select · ↑/↓ to navigate · Esc to cancel";
        let asking = tracker.observe(menu, None, Some("claude"), 2_500);
        assert!(asking.menu_prompt_active);
        assert!(asking.write_due);
        let steady = tracker.observe(menu, None, Some("claude"), 4_000);
        assert!(steady.menu_prompt_active);
        assert!(!steady.write_due);
        let answered = tracker.observe(quiet, None, Some("claude"), 5_500);
        assert!(!answered.menu_prompt_active);
        assert!(answered.write_due);
    }
}
