use std::collections::VecDeque;

use paneflow_config::schema::{SessionGeneration, SessionId};
use serde_json::{Value, json};

use crate::hook_state::{Notice, Outcome};

pub const MAX_NOTIFICATION_BODY_CHARS: usize = 200;

pub const ACTIVITY_LOG_CAPACITY: usize = 256;

pub const KIND_FINISHED: &str = "finished";
pub const KIND_NEEDS_INPUT: &str = "needs_input";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub kind: &'static str,
    pub runtime_label: String,
    pub body: Option<String>,
}

impl Notification {
    pub fn to_value(&self) -> Value {
        json!({
            "kind": self.kind,
            "runtime_label": self.runtime_label,
            "body": self.body,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityLogEntry {
    pub session: SessionId,
    pub generation: SessionGeneration,
    pub outcome: String,
    pub at_ms: u64,
}

impl ActivityLogEntry {
    pub fn to_value(&self) -> Value {
        json!({
            "session": self.session,
            "generation": self.generation,
            "outcome": self.outcome,
            "at_ms": self.at_ms,
        })
    }
}

#[derive(Debug, Default)]
pub struct ActivityLog {
    entries: VecDeque<ActivityLogEntry>,
}

impl ActivityLog {
    pub fn record(
        &mut self,
        session: &SessionId,
        generation: SessionGeneration,
        outcome: &Outcome,
        at_ms: u64,
    ) {
        let entry = ActivityLogEntry {
            session: session.clone(),
            generation,
            outcome: outcome.wire_string(),
            at_ms,
        };
        if self.entries.len() == ACTIVITY_LOG_CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    #[cfg(test)]
    pub fn entries(&self) -> impl Iterator<Item = &ActivityLogEntry> {
        self.entries.iter()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn to_values(&self, limit: usize) -> Vec<Value> {
        self.entries
            .iter()
            .rev()
            .take(limit)
            .map(ActivityLogEntry::to_value)
            .collect()
    }
}

pub fn truncate_body(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(MAX_NOTIFICATION_BODY_CHARS).collect())
}

pub fn notification_for(
    notice: Notice,
    hook_sourced: bool,
    runtime_label: &str,
    body: Option<&str>,
) -> Option<Notification> {
    if !hook_sourced && notice == Notice::Finished {
        return None;
    }
    let kind = match notice {
        Notice::Finished => KIND_FINISHED,
        Notice::NeedsInput => KIND_NEEDS_INPUT,
    };
    Some(Notification {
        kind,
        runtime_label: runtime_label.to_owned(),
        body: truncate_body(body),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_screen_sourced_completion_never_notifies_but_a_menu_edge_does() {
        assert_eq!(
            notification_for(Notice::Finished, false, "Claude Code", Some("done")),
            None
        );
        assert_eq!(
            notification_for(Notice::NeedsInput, false, "Claude Code", None)
                .map(|notification| notification.kind),
            Some(KIND_NEEDS_INPUT)
        );
    }

    #[test]
    fn a_finished_notification_carries_the_label_and_a_two_hundred_character_body() {
        let long = "x".repeat(500);
        let notification = notification_for(Notice::Finished, true, "Claude Code", Some(&long))
            .expect("a hook-sourced completion notifies");
        assert_eq!(notification.kind, KIND_FINISHED);
        assert_eq!(notification.runtime_label, "Claude Code");
        assert_eq!(
            notification.body.as_deref().map(str::len),
            Some(MAX_NOTIFICATION_BODY_CHARS)
        );

        let bare = notification_for(Notice::NeedsInput, true, "Codex", Some("   "))
            .expect("a need for input notifies");
        assert_eq!(bare.kind, KIND_NEEDS_INPUT);
        assert_eq!(bare.body, None);
    }

    #[test]
    fn the_activity_log_keeps_each_settlement_and_stays_bounded() {
        let session = SessionId::new();
        let mut log = ActivityLog::default();
        log.record(&session, SessionGeneration::FIRST, &Outcome::Expired, 10);
        log.record(&session, SessionGeneration::FIRST, &Outcome::Expired, 20);
        assert_eq!(log.len(), 2);

        log.record(
            &session,
            SessionGeneration::FIRST,
            &Outcome::Failed(Some("matcher".to_string())),
            30,
        );
        log.record(&session, SessionGeneration::FIRST, &Outcome::Cancelled, 40);
        assert_eq!(
            log.entries()
                .map(|entry| entry.outcome.as_str())
                .collect::<Vec<_>>(),
            vec!["expired", "expired", "failed:matcher", "cancelled"]
        );

        for tick in 0..ACTIVITY_LOG_CAPACITY * 2 {
            let outcome = if tick % 2 == 0 {
                Outcome::Completed
            } else {
                Outcome::Expired
            };
            log.record(
                &SessionId::new(),
                SessionGeneration::FIRST,
                &outcome,
                tick as u64,
            );
        }
        assert_eq!(log.len(), ACTIVITY_LOG_CAPACITY);
        assert_eq!(log.to_values(3).len(), 3);
    }
}
