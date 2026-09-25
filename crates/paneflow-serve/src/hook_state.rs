use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use paneflow_config::schema::SessionId;

use crate::hook_assets;

pub const HOOK_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

pub const ATTENTION_DEDUPE_WINDOW: Duration = Duration::from_secs(10);

const LEGACY_GENERATION_STOP_GUARD: Duration = Duration::from_secs(30);

pub const EVENT_START: &str = "Start";
pub const EVENT_HOOK_SEEN: &str = "HookSeen";
pub const EVENT_USER_PROMPT_SUBMIT: &str = "UserPromptSubmit";
pub const EVENT_STOP: &str = "Stop";
pub const EVENT_STOP_FAILURE: &str = "StopFailure";
pub const EVENT_STOP_CANCELLED: &str = "StopCancelled";
pub const EVENT_IDLE: &str = "Idle";
pub const EVENT_PERMISSION_REQUEST: &str = "PermissionRequest";
pub const EVENT_NOTIFICATION: &str = "Notification";
pub const EVENT_SESSION_END: &str = "SessionEnd";

pub const NOTIFICATION_PERMISSION_PROMPT: &str = "permission_prompt";
pub const NOTIFICATION_ELICITATION_DIALOG: &str = "elicitation_dialog";
#[cfg(test)]
pub const NOTIFICATION_IDLE_PROMPT: &str = "idle_prompt";

pub const ASK_USER_QUESTION: &str = "AskUserQuestion";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookState {
    Busy,
    Idle,
    Attention,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Completed,
    Failed(Option<String>),
    Expired,
    Cancelled,
}

impl Outcome {
    pub fn wire_string(&self) -> String {
        match self {
            Self::Completed => "completed".to_string(),
            Self::Failed(None) => "failed".to_string(),
            Self::Failed(Some(reason)) => format!("failed:{reason}"),
            Self::Expired => "expired".to_string(),
            Self::Cancelled => "cancelled".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    Finished,
    NeedsInput,
}

#[derive(Debug, Clone, Default)]
pub struct HookEventInput<'a> {
    pub raw_name: &'a str,
    pub tool_name: Option<&'a str>,
    pub notification_type: Option<&'a str>,
    pub failure_reason: Option<&'a str>,
    pub background_tasks_pending: bool,
    pub event_generation: Option<u64>,
    pub current_generation: u64,
    pub generation_started_at_ms: Option<u64>,
}

#[derive(Debug, Clone)]
struct BackgroundActivity {
    started_at: SystemTime,
    deadline_at: SystemTime,
    expired: bool,
}

#[derive(Debug, Default)]
struct Entry {
    background: BTreeMap<String, BackgroundActivity>,
    background_changed_at: Option<SystemTime>,
    background_signal: Option<SystemTime>,
    background_expired: bool,
    background_tasks_pending: bool,
    native_cancelled: bool,
    last_cancelled_at: Option<u64>,
    cancelled_at: Option<u64>,
    submitted_at: Option<u64>,
    pending_opener: Option<(String, Option<String>, SystemTime)>,
    last_turn_started_at: Option<u64>,
    completed: bool,
    runtime_launch_generation: u64,
    hook_seen: bool,
    state: Option<HookState>,
    deadline_at: Option<SystemTime>,
    last_signal: Option<u64>,
    last_transition_at: Option<SystemTime>,
    session_dir: Option<PathBuf>,
    last_hook_generation: Option<u64>,
    confirmed_turn_generation: Option<u64>,
    legacy_turn_started_at: Option<SystemTime>,
    foreground_identity: Option<String>,
    foreground_identity_recorded: bool,
    last_permission_request_at: Option<SystemTime>,
    last_needs_input_at: Option<SystemTime>,
    menu_prompt_active: bool,
    menu_prompt_recorded: bool,
    outcome: Option<Outcome>,
    pending_notice: Option<Notice>,
}

impl Entry {
    fn fresh_for(generation: u64) -> Self {
        Self {
            runtime_launch_generation: generation,
            ..Self::default()
        }
    }
}

pub fn normalize_event_name(raw: &str) -> String {
    let key: String = raw
        .trim()
        .to_lowercase()
        .chars()
        .filter(|character| !matches!(character, '-' | '_' | ' '))
        .collect();
    match key.as_str() {
        "start" => EVENT_START.to_string(),
        "sessionstart" | "subagentstart" | "subagentstop" | "hookseen" | "ai.sessionstart"
        | "ai.tooluse" => EVENT_HOOK_SEEN.to_string(),
        "userpromptsubmit" | "userpromptsubmitted" | "beforesubmitprompt" | "ai.promptsubmit" => {
            EVENT_USER_PROMPT_SUBMIT.to_string()
        }
        "stop" | "ai.stop" => EVENT_STOP.to_string(),
        "stopfailure" => EVENT_STOP_FAILURE.to_string(),
        "stopcancelled" | "stopcanceled" | "interrupt" => EVENT_STOP_CANCELLED.to_string(),
        "idle" | "exit" | "ai.exit" => EVENT_IDLE.to_string(),
        "permissionrequest" | "ai.notification" => EVENT_PERMISSION_REQUEST.to_string(),
        "notification" => EVENT_NOTIFICATION.to_string(),
        "sessionend" | "ai.sessionend" => EVENT_SESSION_END.to_string(),
        _ => raw.trim().to_string(),
    }
}

fn starts_turn(canonical: &str) -> bool {
    matches!(canonical, EVENT_START | EVENT_USER_PROMPT_SUBMIT)
}

fn settles_turn(canonical: &str) -> bool {
    matches!(
        canonical,
        EVENT_STOP | EVENT_STOP_FAILURE | EVENT_STOP_CANCELLED | EVENT_IDLE
    )
}

pub fn is_latch_only(canonical: &str, tool_name: Option<&str>, asks_for_input: bool) -> bool {
    match canonical {
        EVENT_START
        | EVENT_USER_PROMPT_SUBMIT
        | EVENT_STOP
        | EVENT_STOP_FAILURE
        | EVENT_STOP_CANCELLED
        | EVENT_IDLE => false,
        EVENT_PERMISSION_REQUEST => tool_name == Some(ASK_USER_QUESTION),
        EVENT_NOTIFICATION => !asks_for_input,
        _ => true,
    }
}

fn notification_asks_for_input(notification_type: Option<&str>) -> bool {
    matches!(
        notification_type,
        Some(NOTIFICATION_PERMISSION_PROMPT | NOTIFICATION_ELICITATION_DIALOG)
    )
}

pub fn unix_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

pub fn at_unix_ms(milliseconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(milliseconds)
}

#[derive(Debug, Default)]
pub struct ActivityEngine {
    entries: BTreeMap<SessionId, Entry>,
}

impl ActivityEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply_hook_event(
        &mut self,
        session: &SessionId,
        input: HookEventInput<'_>,
        now: SystemTime,
    ) -> bool {
        self.observe_runtime_launch(
            session,
            input.current_generation,
            input.generation_started_at_ms,
        );
        if input
            .event_generation
            .is_some_and(|generation| generation < input.current_generation)
        {
            return false;
        }

        let canonical = normalize_event_name(input.raw_name);
        let launched_at = input.generation_started_at_ms.map(at_unix_ms);
        if input.event_generation.is_none() && launched_at.is_some_and(|launch| now < launch) {
            return false;
        }

        let entry = self.entries.entry(session.clone()).or_default();
        if starts_turn(&canonical) {
            entry.confirmed_turn_generation =
                Some(input.event_generation.unwrap_or(input.current_generation));
            if input.event_generation.is_none() {
                entry.legacy_turn_started_at = Some(now);
            }
        } else if settles_turn(&canonical)
            && input.event_generation.is_none()
            && input.current_generation > 1
            && entry.confirmed_turn_generation != Some(input.current_generation)
            && launched_at.is_some_and(|launch| {
                now.duration_since(launch).unwrap_or_default() < LEGACY_GENERATION_STOP_GUARD
            })
        {
            return false;
        }

        if !self.apply_canonical(session, &canonical, &input, now) {
            return false;
        }
        if let Some(entry) = self.entries.get_mut(session) {
            entry.last_hook_generation = input.event_generation;
        }
        true
    }

    fn apply_canonical(
        &mut self,
        session: &SessionId,
        canonical: &str,
        input: &HookEventInput<'_>,
        now: SystemTime,
    ) -> bool {
        let asks_for_input = canonical == EVENT_NOTIFICATION
            && notification_asks_for_input(input.notification_type)
            && self
                .entries
                .get(session)
                .and_then(|entry| entry.last_permission_request_at)
                .is_none_or(|at| {
                    now.duration_since(at).unwrap_or_default() >= ATTENTION_DEDUPE_WINDOW
                });
        let latch_only = is_latch_only(canonical, input.tool_name, asks_for_input);
        let entry = self.entries.entry(session.clone()).or_default();
        if entry.native_cancelled {
            if starts_turn(canonical) {
                entry.native_cancelled = false;
            } else {
                return false;
            }
        }
        if let Some(cancelled_at) = entry.cancelled_at {
            let event_at = unix_ms(now);
            let resumed = starts_turn(canonical)
                && entry
                    .submitted_at
                    .is_some_and(|submitted| submitted > cancelled_at && event_at >= submitted);
            if resumed {
                entry.cancelled_at = None;
                entry.pending_opener = None;
            } else {
                if starts_turn(canonical) && event_at >= cancelled_at {
                    entry.pending_opener = Some((
                        canonical.to_string(),
                        input.tool_name.map(str::to_owned),
                        now,
                    ));
                }
                return false;
            }
        }
        entry.hook_seen = true;
        if canonical == EVENT_PERMISSION_REQUEST {
            entry.last_permission_request_at = Some(now);
        }
        if latch_only {
            return true;
        }
        if canonical == EVENT_IDLE && entry.state == Some(HookState::Idle) {
            return true;
        }
        let completed_edge = canonical == EVENT_STOP
            && matches!(entry.state, Some(HookState::Busy | HookState::Attention));
        entry.last_transition_at = Some(now);
        match canonical {
            EVENT_START | EVENT_USER_PROMPT_SUBMIT => {
                entry.completed = false;
                entry.background_expired = false;
                entry.background_tasks_pending = false;
                entry.outcome = None;
                entry.last_needs_input_at = None;
                entry.last_turn_started_at = Some(unix_ms(now));
                entry.state = Some(HookState::Busy);
                entry.deadline_at = Some(now + HOOK_IDLE_TIMEOUT);
            }
            EVENT_STOP if input.background_tasks_pending => {
                entry.completed = false;
                entry.background_tasks_pending = true;
                entry.state = Some(HookState::Busy);
                entry.deadline_at = Some(now + HOOK_IDLE_TIMEOUT);
            }
            EVENT_STOP | EVENT_STOP_FAILURE => {
                entry.completed = canonical == EVENT_STOP;
                entry.background_tasks_pending = false;
                entry.state = Some(HookState::Idle);
                entry.deadline_at = None;
                entry.outcome = Some(if canonical == EVENT_STOP {
                    Outcome::Completed
                } else {
                    Outcome::Failed(input.failure_reason.map(str::to_owned))
                });
            }
            EVENT_STOP_CANCELLED => {
                entry.native_cancelled = true;
                entry.completed = false;
                entry.background_tasks_pending = false;
                entry.state = Some(HookState::Idle);
                entry.deadline_at = None;
                entry.outcome = Some(Outcome::Cancelled);
            }
            EVENT_IDLE => {
                entry.completed = false;
                entry.background_tasks_pending = false;
                entry.state = Some(HookState::Idle);
                entry.deadline_at = None;
            }
            EVENT_PERMISSION_REQUEST | EVENT_NOTIFICATION => {
                entry.completed = false;
                entry.state = Some(HookState::Attention);
                entry.deadline_at = None;
                entry.last_signal = None;
            }
            _ => {}
        }
        self.arm_notice(session, canonical, completed_edge, now);
        true
    }

    fn arm_notice(
        &mut self,
        session: &SessionId,
        canonical: &str,
        completed_edge: bool,
        now: SystemTime,
    ) {
        let completed = self.is_completed(session);
        let Some(entry) = self.entries.get_mut(session) else {
            return;
        };
        match entry.state {
            Some(HookState::Attention) => {
                let deduped = entry.last_needs_input_at.is_some_and(|at| {
                    now.duration_since(at).unwrap_or_default() < ATTENTION_DEDUPE_WINDOW
                });
                if !deduped {
                    entry.last_needs_input_at = Some(now);
                    entry.pending_notice = Some(Notice::NeedsInput);
                }
            }
            Some(HookState::Idle) if canonical == EVENT_STOP && completed && completed_edge => {
                entry.pending_notice = Some(Notice::Finished);
            }
            _ => {}
        }
    }

    pub fn observe_menu_prompt(&mut self, session: &SessionId, active: bool, now: SystemTime) {
        let entry = self.entries.entry(session.clone()).or_default();
        let edge = entry.menu_prompt_recorded && active && !entry.menu_prompt_active;
        entry.menu_prompt_active = active;
        entry.menu_prompt_recorded = true;
        if !edge {
            return;
        }
        let deduped = entry
            .last_needs_input_at
            .is_some_and(|at| now.duration_since(at).unwrap_or_default() < ATTENTION_DEDUPE_WINDOW);
        if !deduped {
            entry.last_needs_input_at = Some(now);
            entry.pending_notice = Some(Notice::NeedsInput);
        }
    }

    pub fn take_notice(&mut self, session: &SessionId) -> Option<Notice> {
        self.entries
            .get_mut(session)
            .and_then(|entry| entry.pending_notice.take())
    }

    pub fn outcome(&self, session: &SessionId) -> Option<Outcome> {
        self.entries
            .get(session)
            .and_then(|entry| entry.outcome.clone())
    }

    pub fn is_latched(&self, session: &SessionId) -> bool {
        self.entries
            .get(session)
            .is_some_and(|entry| entry.hook_seen)
    }

    pub fn hook_owned_state(&self, session: &SessionId) -> Option<HookState> {
        let entry = self.entries.get(session)?;
        if !entry.hook_seen {
            return None;
        }
        if entry.state == Some(HookState::Attention) {
            return entry.state;
        }
        let fenced = entry.cancelled_at.is_some() || entry.native_cancelled;
        if !fenced && entry.background.values().any(|activity| !activity.expired) {
            return Some(HookState::Busy);
        }
        entry.state
    }

    pub fn is_cancelled(&self, session: &SessionId) -> bool {
        self.entries
            .get(session)
            .is_some_and(|entry| entry.cancelled_at.is_some() || entry.native_cancelled)
    }

    pub fn is_completed(&self, session: &SessionId) -> bool {
        self.entries.get(session).is_some_and(|entry| {
            entry.hook_seen
                && entry.state == Some(HookState::Idle)
                && entry.completed
                && !entry.background_expired
                && !entry.background_tasks_pending
                && entry.background.values().all(|activity| activity.expired)
        })
    }

    #[cfg(test)]
    pub fn runtime_launch_generation(&self, session: &SessionId) -> Option<u64> {
        self.entries
            .get(session)
            .map(|entry| entry.runtime_launch_generation)
    }

    pub fn observe_runtime_launch(
        &mut self,
        session: &SessionId,
        generation: u64,
        generation_started_at_ms: Option<u64>,
    ) {
        let entry = self.entries.entry(session.clone()).or_default();
        if entry.runtime_launch_generation == generation {
            return;
        }
        let launched_at = generation_started_at_ms.map(at_unix_ms);
        let saw_exact_generation = entry.last_hook_generation == Some(generation);
        let saw_new_legacy_turn = entry.last_hook_generation.is_none()
            && launched_at.is_some_and(|launched_at| {
                entry
                    .legacy_turn_started_at
                    .is_some_and(|started_at| started_at >= launched_at)
            });
        if !(saw_exact_generation || saw_new_legacy_turn) {
            let session_dir = entry.session_dir.take();
            *entry = Entry::default();
            entry.session_dir = session_dir;
        }
        entry.runtime_launch_generation = generation;
        if saw_new_legacy_turn {
            entry.confirmed_turn_generation = Some(generation);
        }
    }

    pub fn observe_foreground_runtime(&mut self, session: &SessionId, observed: Option<&str>) {
        let entry = self.entries.entry(session.clone()).or_default();
        let changed_to_new_agent = entry.foreground_identity_recorded
            && observed.is_some()
            && entry.foreground_identity.as_deref() != observed;
        if changed_to_new_agent {
            let generation = entry.runtime_launch_generation;
            let session_dir = entry.session_dir.take();
            *entry = Entry::fresh_for(generation);
            entry.session_dir = session_dir;
        }
        entry.foreground_identity = observed.map(str::to_owned);
        entry.foreground_identity_recorded = true;
    }

    pub fn attention_has_new_output(&self, session: &SessionId, activity_signal: u64) -> bool {
        self.entries.get(session).is_some_and(|entry| {
            entry.state == Some(HookState::Attention)
                && entry
                    .last_signal
                    .is_some_and(|previous| previous != activity_signal)
        })
    }

    pub fn note_output_and_sweep(
        &mut self,
        session: &SessionId,
        activity_signal: u64,
        allow_attention_clear: bool,
        now: SystemTime,
    ) {
        let entry = self.entries.entry(session.clone()).or_default();
        let background_grew = entry
            .background_signal
            .is_some_and(|previous| Some(previous) != entry.background_changed_at);
        let grew = background_grew
            || entry
                .last_signal
                .is_some_and(|previous| previous != activity_signal);
        entry.last_signal = Some(activity_signal);
        entry.background_signal = entry.background_changed_at;
        for activity in entry
            .background
            .values_mut()
            .filter(|activity| !activity.expired)
        {
            if grew {
                activity.deadline_at = now + HOOK_IDLE_TIMEOUT;
            } else if activity.deadline_at <= now {
                activity.expired = true;
                entry.background_expired = true;
            }
        }
        if entry.cancelled_at.is_some() || entry.native_cancelled {
            return;
        }
        match entry.state {
            Some(HookState::Attention) if allow_attention_clear && grew => {
                entry.state = Some(HookState::Busy);
                entry.deadline_at = Some(now + HOOK_IDLE_TIMEOUT);
            }
            Some(HookState::Busy) => {
                if entry.deadline_at.is_some_and(|deadline| deadline <= now) {
                    entry.completed = false;
                    entry.background_tasks_pending = false;
                    entry.state = Some(HookState::Idle);
                    entry.deadline_at = None;
                    entry.outcome = Some(Outcome::Expired);
                    if let (Some(dir), Some(through)) =
                        (entry.session_dir.as_deref(), entry.last_transition_at)
                        && let Err(error) = hook_assets::record_hook_expiry(
                            dir,
                            entry.runtime_launch_generation,
                            through,
                        )
                    {
                        log::warn!("paneflow-serve: cannot persist the hook expiry: {error}");
                    }
                } else if grew {
                    entry.deadline_at = Some(now + HOOK_IDLE_TIMEOUT);
                }
            }
            _ => {}
        }
    }

    pub fn clear_output_baseline(&mut self, session: &SessionId) {
        if let Some(entry) = self.entries.get_mut(session) {
            entry.last_signal = None;
        }
    }

    pub fn bind_session_dir(&mut self, session: &SessionId, dir: &Path) {
        self.entries.entry(session.clone()).or_default().session_dir = Some(dir.to_path_buf());
    }

    pub fn sync_cancellation_from_disk(
        &mut self,
        session: &SessionId,
        dir: &Path,
        generation: u64,
    ) {
        if let Some(marker) = hook_assets::read_cancellation(dir) {
            self.observe_cancellation(session, &marker, generation);
        }
    }

    pub fn observe_cancellation(
        &mut self,
        session: &SessionId,
        marker: &hook_assets::Cancellation,
        generation: u64,
    ) {
        if marker.runtime_generation != generation {
            return;
        }
        let entry = self.entries.entry(session.clone()).or_default();
        if entry
            .last_cancelled_at
            .is_none_or(|at| marker.cancelled_at > at)
        {
            entry.last_cancelled_at = Some(marker.cancelled_at);
            entry.pending_opener = None;
            let already_started_next_turn = marker.submitted_at.is_some_and(|submitted| {
                submitted > marker.cancelled_at
                    && entry
                        .last_turn_started_at
                        .is_some_and(|started| started >= submitted)
            });
            if !already_started_next_turn {
                entry.completed = false;
                entry.cancelled_at = Some(marker.cancelled_at);
                if entry.hook_seen {
                    entry.state = Some(HookState::Idle);
                    entry.outcome = Some(Outcome::Cancelled);
                }
                entry.deadline_at = None;
            }
        }
        entry.submitted_at = marker.submitted_at;
        let pending = entry.pending_opener.take();
        if let Some((name, tool, at)) = pending {
            let input = HookEventInput {
                raw_name: &name,
                tool_name: tool.as_deref(),
                current_generation: generation,
                ..HookEventInput::default()
            };
            self.apply_canonical(session, &name, &input, at);
        }
    }

    pub fn sync_background_from_disk(
        &mut self,
        session: &SessionId,
        dir: &Path,
        generation: u64,
        not_before_unix_ms: Option<u64>,
    ) {
        let Ok(activities) = hook_assets::read_background_activity(dir, generation) else {
            return;
        };
        let entry = self.entries.entry(session.clone()).or_default();
        entry.background_changed_at = activities.changed_at;
        let mut current = BTreeSet::new();
        for activity in activities.markers {
            if not_before_unix_ms.is_some_and(|epoch| unix_ms(activity.started_at) < epoch) {
                continue;
            }
            current.insert(activity.id.clone());
            if entry
                .background
                .get(&activity.id)
                .is_some_and(|known| known.started_at == activity.started_at)
            {
                continue;
            }
            entry.hook_seen = true;
            entry.background.insert(
                activity.id,
                BackgroundActivity {
                    started_at: activity.started_at,
                    deadline_at: activity.started_at + HOOK_IDLE_TIMEOUT,
                    expired: false,
                },
            );
        }
        entry.background.retain(|id, _| current.contains(id));
    }

    pub fn seed_from_disk(
        &mut self,
        session: &SessionId,
        session_dir: &Path,
        anchor_start_to_output: bool,
        not_before_unix_ms: Option<u64>,
        current_generation: u64,
        output_signal_at_ms: Option<u64>,
    ) {
        self.observe_runtime_launch(session, current_generation, not_before_unix_ms);
        self.bind_session_dir(session, session_dir);
        self.sync_cancellation_from_disk(session, session_dir, current_generation);
        self.sync_background_from_disk(
            session,
            session_dir,
            current_generation,
            not_before_unix_ms,
        );
        let Some(seed) = hook_assets::read_last_hook_event(session_dir) else {
            return;
        };
        if self
            .entries
            .get(session)
            .and_then(|entry| entry.last_transition_at)
            .is_some_and(|at| seed.modified_at <= at)
        {
            return;
        }
        if not_before_unix_ms.is_some_and(|not_before| unix_ms(seed.modified_at) < not_before) {
            return;
        }

        let canonical = normalize_event_name(&seed.hook_event_name);
        let seed_at = seed.modified_at;
        let anchor = match canonical.as_str() {
            EVENT_USER_PROMPT_SUBMIT => true,
            EVENT_START => anchor_start_to_output,
            _ => false,
        };
        let mut lease_at = seed_at;
        if starts_turn(&canonical)
            && anchor
            && !self.is_cancelled(session)
            && let Some(output_at) = output_signal_at_ms.map(at_unix_ms)
        {
            lease_at = lease_at.max(output_at);
        }
        let input = HookEventInput {
            raw_name: &canonical,
            tool_name: seed.tool_name.as_deref(),
            notification_type: seed.notification_type.as_deref(),
            event_generation: seed.runtime_generation,
            current_generation,
            generation_started_at_ms: not_before_unix_ms,
            ..HookEventInput::default()
        };
        let accepted = self.apply_hook_event(session, input, seed_at);
        if !accepted || !starts_turn(&canonical) {
            return;
        }
        self.restore_opening_lease(session, session_dir, current_generation, seed_at, lease_at);
    }

    pub fn restore_opening_lease(
        &mut self,
        session: &SessionId,
        session_dir: &Path,
        current_generation: u64,
        event_at: SystemTime,
        lease_at: SystemTime,
    ) {
        let expired = hook_assets::hook_turn_expired(session_dir, current_generation, event_at);
        if let Some(entry) = self.entries.get_mut(session) {
            if expired {
                entry.completed = false;
                entry.background_tasks_pending = false;
                entry.state = Some(HookState::Idle);
                entry.deadline_at = None;
                entry.outcome = Some(Outcome::Expired);
                entry.pending_notice = None;
            } else {
                entry.deadline_at = Some(lease_at + HOOK_IDLE_TIMEOUT);
            }
        }
    }

    pub fn remove_session(&mut self, session: &SessionId) {
        self.entries.remove(session);
    }

    pub fn retain_sessions(&mut self, live: &BTreeSet<SessionId>) {
        self.entries.retain(|session, _| live.contains(session));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(milliseconds: u64) -> SystemTime {
        at_unix_ms(milliseconds)
    }

    fn opened(raw_name: &str) -> HookEventInput<'static> {
        HookEventInput {
            raw_name: Box::leak(raw_name.to_string().into_boxed_str()),
            current_generation: 1,
            ..HookEventInput::default()
        }
    }

    fn engine_with_turn(session: &SessionId) -> ActivityEngine {
        let mut engine = ActivityEngine::new();
        assert!(engine.apply_hook_event(session, opened("UserPromptSubmit"), at(1_000)));
        engine
    }

    #[test]
    fn an_opening_event_arms_a_five_minute_lease_and_a_stop_completes_the_turn() {
        let session = SessionId::new();
        let mut engine = engine_with_turn(&session);
        assert_eq!(engine.hook_owned_state(&session), Some(HookState::Busy));
        assert!(!engine.is_completed(&session));

        assert!(engine.apply_hook_event(&session, opened("Stop"), at(2_000)));
        assert_eq!(engine.hook_owned_state(&session), Some(HookState::Idle));
        assert!(engine.is_completed(&session));
        assert_eq!(engine.outcome(&session), Some(Outcome::Completed));
        assert_eq!(engine.take_notice(&session), Some(Notice::Finished));
        assert_eq!(engine.take_notice(&session), None);

        assert!(engine.apply_hook_event(&session, opened("Stop"), at(2_100)));
        assert_eq!(
            engine.take_notice(&session),
            None,
            "a duplicate stop is not a second busy-to-idle edge"
        );
    }

    #[test]
    fn a_stop_failure_settles_without_completion_and_never_notifies() {
        let session = SessionId::new();
        let mut engine = engine_with_turn(&session);
        let mut failure = opened("StopFailure");
        failure.failure_reason = Some("matcher");
        assert!(engine.apply_hook_event(&session, failure, at(2_000)));
        assert_eq!(engine.hook_owned_state(&session), Some(HookState::Idle));
        assert!(!engine.is_completed(&session));
        assert_eq!(
            engine
                .outcome(&session)
                .map(|outcome| outcome.wire_string()),
            Some("failed:matcher".to_string())
        );
        assert_eq!(engine.take_notice(&session), None);
    }

    #[test]
    fn a_native_cancellation_settles_and_only_a_new_opener_rearms_the_turn() {
        let session = SessionId::new();
        let mut engine = engine_with_turn(&session);
        assert!(engine.apply_hook_event(&session, opened("Interrupt"), at(2_000)));
        assert_eq!(engine.hook_owned_state(&session), Some(HookState::Idle));
        assert_eq!(engine.outcome(&session), Some(Outcome::Cancelled));
        assert!(engine.is_cancelled(&session));
        assert_eq!(engine.take_notice(&session), None);

        assert!(!engine.apply_hook_event(&session, opened("Stop"), at(2_500)));
        assert_eq!(engine.hook_owned_state(&session), Some(HookState::Idle));

        assert!(engine.apply_hook_event(&session, opened("UserPromptSubmit"), at(3_000)));
        assert_eq!(engine.hook_owned_state(&session), Some(HookState::Busy));
        assert!(!engine.is_cancelled(&session));
    }

    #[test]
    fn a_pending_opener_rearms_only_after_the_cancellation_marker_records_submission() {
        let session = SessionId::new();
        let mut engine = engine_with_turn(&session);
        let mut marker = hook_assets::Cancellation {
            runtime_generation: 1,
            cancelled_at: 2_000,
            submitted_at: None,
        };
        engine.observe_cancellation(&session, &marker, 1);
        assert_eq!(engine.hook_owned_state(&session), Some(HookState::Idle));
        assert_eq!(engine.outcome(&session), Some(Outcome::Cancelled));

        assert!(!engine.apply_hook_event(&session, opened("UserPromptSubmit"), at(2_500)));
        assert_eq!(engine.hook_owned_state(&session), Some(HookState::Idle));

        marker.submitted_at = Some(2_400);
        engine.observe_cancellation(&session, &marker, 1);
        assert_eq!(engine.hook_owned_state(&session), Some(HookState::Busy));
        assert!(!engine.is_cancelled(&session));
    }

    #[test]
    fn latch_only_events_claim_hook_ownership_without_moving_the_state() {
        let session = SessionId::new();
        let mut engine = ActivityEngine::new();
        for name in ["SessionStart", "SubagentStart", "SubagentStop"] {
            assert!(engine.apply_hook_event(&session, opened(name), at(1_000)));
        }
        assert!(engine.is_latched(&session));
        assert_eq!(engine.hook_owned_state(&session), None);

        let mut busy = engine_with_turn(&session);
        assert!(busy.apply_hook_event(&session, opened("SubagentStop"), at(2_000)));
        assert_eq!(busy.hook_owned_state(&session), Some(HookState::Busy));
        assert!(!busy.is_completed(&session));
    }

    #[test]
    fn a_permission_request_asks_for_input_unless_it_is_an_ask_user_question() {
        let session = SessionId::new();
        let mut engine = engine_with_turn(&session);
        let mut question = opened("PermissionRequest");
        question.tool_name = Some(ASK_USER_QUESTION);
        assert!(engine.apply_hook_event(&session, question, at(2_000)));
        assert_eq!(engine.hook_owned_state(&session), Some(HookState::Busy));
        assert_eq!(engine.take_notice(&session), None);

        assert!(engine.apply_hook_event(&session, opened("PermissionRequest"), at(3_000)));
        assert_eq!(
            engine.hook_owned_state(&session),
            Some(HookState::Attention)
        );
        assert_eq!(engine.take_notice(&session), Some(Notice::NeedsInput));
    }

    #[test]
    fn a_permission_prompt_notification_deduplicates_against_a_recent_permission_request() {
        let session = SessionId::new();
        let mut engine = engine_with_turn(&session);
        assert!(engine.apply_hook_event(&session, opened("PermissionRequest"), at(2_000)));
        assert_eq!(engine.take_notice(&session), Some(Notice::NeedsInput));

        let mut prompt = opened("Notification");
        prompt.notification_type = Some(NOTIFICATION_PERMISSION_PROMPT);
        assert!(engine.apply_hook_event(&session, prompt.clone(), at(2_500)));
        assert_eq!(
            engine.take_notice(&session),
            None,
            "the second signal inside the window is one need for input, not two"
        );

        let mut idle_prompt = opened("Notification");
        idle_prompt.notification_type = Some(NOTIFICATION_IDLE_PROMPT);
        assert!(engine.apply_hook_event(&session, idle_prompt, at(3_000)));
        assert_eq!(
            engine.hook_owned_state(&session),
            Some(HookState::Attention)
        );
        assert_eq!(engine.take_notice(&session), None);
    }

    #[test]
    fn a_menu_edge_asks_for_input_once_and_never_twice_inside_the_window() {
        let session = SessionId::new();
        let mut engine = engine_with_turn(&session);
        engine.observe_menu_prompt(&session, false, at(1_500));
        engine.observe_menu_prompt(&session, true, at(2_000));
        assert_eq!(engine.take_notice(&session), Some(Notice::NeedsInput));

        engine.observe_menu_prompt(&session, false, at(2_200));
        engine.observe_menu_prompt(&session, true, at(2_400));
        assert_eq!(engine.take_notice(&session), None);

        engine.observe_menu_prompt(&session, false, at(2_500));
        engine.observe_menu_prompt(&session, true, at(20_000));
        assert_eq!(engine.take_notice(&session), Some(Notice::NeedsInput));
    }

    #[test]
    fn a_first_menu_sighting_is_recorded_and_is_not_an_edge() {
        let session = SessionId::new();
        let mut engine = engine_with_turn(&session);
        engine.observe_menu_prompt(&session, true, at(2_000));
        assert_eq!(engine.take_notice(&session), None);
    }

    #[test]
    fn a_lost_stop_expires_after_five_unchanged_minutes_and_a_change_rearms_it() {
        let session = SessionId::new();
        let mut engine = engine_with_turn(&session);
        let lease = HOOK_IDLE_TIMEOUT.as_millis() as u64;

        engine.note_output_and_sweep(&session, 10, true, at(1_100));
        engine.note_output_and_sweep(&session, 20, true, at(900 + lease));
        assert_eq!(
            engine.hook_owned_state(&session),
            Some(HookState::Busy),
            "a changed signal rearms the lease"
        );

        engine.note_output_and_sweep(&session, 30, true, at(1_500 + lease));
        assert_eq!(engine.hook_owned_state(&session), Some(HookState::Busy));

        engine.note_output_and_sweep(&session, 30, true, at(1_600 + 2 * lease));
        assert_eq!(engine.hook_owned_state(&session), Some(HookState::Idle));
        assert!(!engine.is_completed(&session));
        assert_eq!(engine.outcome(&session), Some(Outcome::Expired));
        assert_eq!(engine.take_notice(&session), None);
    }

    #[test]
    fn an_unchanged_screen_signal_never_rearms_a_lease_however_much_output_grows() {
        let session = SessionId::new();
        let mut engine = engine_with_turn(&session);
        let lease = HOOK_IDLE_TIMEOUT.as_millis() as u64;
        for tick in 0..10 {
            engine.note_output_and_sweep(&session, 7, true, at(1_100 + tick * 1_000));
        }
        engine.note_output_and_sweep(&session, 7, true, at(1_100 + lease));
        assert_eq!(engine.hook_owned_state(&session), Some(HookState::Idle));
        assert_eq!(engine.outcome(&session), Some(Outcome::Expired));
    }

    #[test]
    fn attention_clears_to_busy_only_when_the_runtime_allows_it() {
        let session = SessionId::new();
        let mut engine = engine_with_turn(&session);
        assert!(engine.apply_hook_event(&session, opened("PermissionRequest"), at(2_000)));
        engine.note_output_and_sweep(&session, 1, false, at(2_100));
        assert!(!engine.attention_has_new_output(&session, 1));
        assert!(engine.attention_has_new_output(&session, 2));
        engine.note_output_and_sweep(&session, 2, false, at(2_200));
        assert_eq!(
            engine.hook_owned_state(&session),
            Some(HookState::Attention)
        );

        engine.note_output_and_sweep(&session, 3, true, at(2_300));
        assert_eq!(engine.hook_owned_state(&session), Some(HookState::Busy));
    }

    #[test]
    fn an_event_from_a_generation_the_session_has_left_is_refused() {
        let session = SessionId::new();
        let mut engine = ActivityEngine::new();
        let mut opener = opened("UserPromptSubmit");
        opener.current_generation = 2;
        opener.event_generation = Some(2);
        assert!(engine.apply_hook_event(&session, opener, at(1_000)));

        let mut stale = opened("Stop");
        stale.current_generation = 2;
        stale.event_generation = Some(1);
        assert!(!engine.apply_hook_event(&session, stale, at(1_100)));
        assert_eq!(engine.hook_owned_state(&session), Some(HookState::Busy));
    }

    #[test]
    fn an_untagged_stop_right_after_a_relaunch_is_quarantined_until_the_replacement_opens_a_turn() {
        let session = SessionId::new();
        let mut engine = ActivityEngine::new();
        let launched_at = 100_000u64;
        let mut stale_stop = opened("Stop");
        stale_stop.current_generation = 2;
        stale_stop.generation_started_at_ms = Some(launched_at);
        assert!(!engine.apply_hook_event(&session, stale_stop.clone(), at(launched_at + 1_000)));

        let mut opener = opened("UserPromptSubmit");
        opener.current_generation = 2;
        opener.generation_started_at_ms = Some(launched_at);
        assert!(engine.apply_hook_event(&session, opener, at(launched_at + 2_000)));
        assert!(engine.apply_hook_event(&session, stale_stop.clone(), at(launched_at + 3_000)));
        assert_eq!(engine.hook_owned_state(&session), Some(HookState::Idle));

        let mut late = opened("Stop");
        late.current_generation = 3;
        late.generation_started_at_ms = Some(launched_at);
        let guard = LEGACY_GENERATION_STOP_GUARD.as_millis() as u64;
        assert!(engine.apply_hook_event(&session, late, at(launched_at + guard + 1)));
    }

    #[test]
    fn a_new_foreground_identity_resets_the_latch_and_keeps_the_generation() {
        let session = SessionId::new();
        let mut engine = engine_with_turn(&session);
        engine.observe_foreground_runtime(&session, Some("com.anthropic.claude-code:10:5"));
        assert_eq!(
            engine.hook_owned_state(&session),
            Some(HookState::Busy),
            "the first identity an observer reports is recorded, never an edge"
        );

        engine.observe_foreground_runtime(&session, Some("com.anthropic.claude-code:10:5"));
        assert_eq!(engine.hook_owned_state(&session), Some(HookState::Busy));

        engine.observe_foreground_runtime(&session, None);
        assert_eq!(engine.hook_owned_state(&session), Some(HookState::Busy));

        engine.observe_foreground_runtime(&session, Some("com.openai.codex:11:6"));
        assert_eq!(engine.hook_owned_state(&session), None);
        assert!(!engine.is_latched(&session));
        assert_eq!(engine.runtime_launch_generation(&session), Some(1));
    }

    #[test]
    fn a_stop_carrying_background_tasks_keeps_the_session_busy_until_the_list_empties() {
        let session = SessionId::new();
        let mut engine = engine_with_turn(&session);
        let mut pending = opened("Stop");
        pending.background_tasks_pending = true;
        assert!(engine.apply_hook_event(&session, pending, at(2_000)));
        assert_eq!(engine.hook_owned_state(&session), Some(HookState::Busy));
        assert!(!engine.is_completed(&session));
        assert_eq!(engine.take_notice(&session), None);

        assert!(engine.apply_hook_event(&session, opened("Stop"), at(3_000)));
        assert_eq!(engine.hook_owned_state(&session), Some(HookState::Idle));
        assert!(engine.is_completed(&session));
        assert_eq!(engine.take_notice(&session), Some(Notice::Finished));
    }

    #[test]
    fn a_background_lease_expires_without_completing_the_turn() {
        let session = SessionId::new();
        let mut engine = engine_with_turn(&session);
        let mut pending = opened("Stop");
        pending.background_tasks_pending = true;
        assert!(engine.apply_hook_event(&session, pending, at(2_000)));
        let lease = HOOK_IDLE_TIMEOUT.as_millis() as u64;
        engine.note_output_and_sweep(&session, 1, true, at(2_100));
        engine.note_output_and_sweep(&session, 1, true, at(2_100 + lease));
        assert_eq!(engine.hook_owned_state(&session), Some(HookState::Idle));
        assert!(!engine.is_completed(&session));
        assert_eq!(engine.outcome(&session), Some(Outcome::Expired));
    }

    fn write_background_marker(dir: &Path, generation: u64, activity_id: &str) {
        let path =
            paneflow_ipc_client::ai_hook::background_marker_path(dir, generation, activity_id)
                .expect("a usable background identity");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "activity_id": activity_id,
                "runtime_generation": generation,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn a_live_subagent_keeps_the_session_busy_through_the_main_stop_until_its_marker_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut engine = engine_with_turn(&session);
        write_background_marker(dir.path(), 1, "explorer-1");
        engine.sync_background_from_disk(&session, dir.path(), 1, None);

        assert!(engine.apply_hook_event(&session, opened("Stop"), at(2_000)));
        assert_eq!(
            engine.hook_owned_state(&session),
            Some(HookState::Busy),
            "a child still running holds the session busy"
        );
        assert!(!engine.is_completed(&session));
        assert_eq!(engine.take_notice(&session), None);

        std::fs::remove_file(
            paneflow_ipc_client::ai_hook::background_marker_path(dir.path(), 1, "explorer-1")
                .unwrap(),
        )
        .unwrap();
        engine.sync_background_from_disk(&session, dir.path(), 1, None);
        assert_eq!(engine.hook_owned_state(&session), Some(HookState::Idle));
        assert!(engine.is_completed(&session));
    }

    #[test]
    fn markers_of_a_previous_generation_are_ignored_and_attention_outranks_a_busy_child() {
        let dir = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut engine = engine_with_turn(&session);
        write_background_marker(dir.path(), 1, "stale");
        engine.sync_background_from_disk(&session, dir.path(), 2, None);
        assert!(engine.apply_hook_event(&session, opened("Stop"), at(2_000)));
        assert_eq!(
            engine.hook_owned_state(&session),
            Some(HookState::Idle),
            "a new generation never inherits the previous generation's children"
        );

        write_background_marker(dir.path(), 1, "child");
        engine.sync_background_from_disk(&session, dir.path(), 1, None);
        assert!(engine.apply_hook_event(&session, opened("PermissionRequest"), at(2_500)));
        assert_eq!(
            engine.hook_owned_state(&session),
            Some(HookState::Attention),
            "the main turn waiting on the user outranks a busy child"
        );
    }

    #[test]
    fn a_stale_child_expires_on_the_lease_and_the_turn_settles_without_completion() {
        let dir = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut engine = engine_with_turn(&session);
        write_background_marker(dir.path(), 1, "child");
        engine.sync_background_from_disk(&session, dir.path(), 1, None);
        assert!(engine.apply_hook_event(&session, opened("Stop"), at(2_000)));

        let lease = HOOK_IDLE_TIMEOUT.as_millis() as u64;
        let started = unix_ms(SystemTime::now());
        engine.note_output_and_sweep(&session, 1, true, at(started + lease + 1_000));
        assert_eq!(engine.hook_owned_state(&session), Some(HookState::Idle));
        assert!(
            !engine.is_completed(&session),
            "an expired child never completes the turn"
        );
        assert_eq!(engine.take_notice(&session), None);
    }

    #[test]
    fn an_escape_fence_settles_a_turn_whose_child_is_still_marked() {
        let dir = tempfile::tempdir().unwrap();
        let session = SessionId::new();
        let mut engine = engine_with_turn(&session);
        write_background_marker(dir.path(), 1, "child");
        engine.sync_background_from_disk(&session, dir.path(), 1, None);
        assert_eq!(engine.hook_owned_state(&session), Some(HookState::Busy));

        let mut marker = hook_assets::Cancellation {
            runtime_generation: 1,
            cancelled_at: 2_000,
            submitted_at: None,
        };
        engine.observe_cancellation(&session, &marker, 1);
        assert_eq!(
            engine.hook_owned_state(&session),
            Some(HookState::Idle),
            "the interrupt ends the children the turn started, so their markers never hold it busy"
        );
        assert_eq!(engine.outcome(&session), Some(Outcome::Cancelled));
        assert!(!engine.is_completed(&session));
        assert_eq!(engine.take_notice(&session), None);

        assert!(!engine.apply_hook_event(&session, opened("UserPromptSubmit"), at(2_500)));
        marker.submitted_at = Some(2_400);
        engine.observe_cancellation(&session, &marker, 1);
        assert_eq!(engine.hook_owned_state(&session), Some(HookState::Busy));
        assert!(!engine.is_cancelled(&session));
    }

    #[test]
    fn an_idle_heartbeat_never_rewrites_a_known_outcome() {
        let session = SessionId::new();
        let mut engine = engine_with_turn(&session);
        assert!(engine.apply_hook_event(&session, opened("Stop"), at(2_000)));
        assert!(engine.is_completed(&session));
        assert!(engine.apply_hook_event(&session, opened("Idle"), at(2_100)));
        assert!(
            engine.is_completed(&session),
            "a provider heartbeat repairs a missing stop, it does not erase one"
        );
    }
}
