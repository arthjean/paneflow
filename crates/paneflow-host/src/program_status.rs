use paneflow_terminal_ghostty::{ProgramStatusKind, ProgramStatusReport, ProgramStatusState};
use serde_json::{Value, json};

pub const MAX_PROGRAM_STATUS_RECORDS: usize = 256;

#[derive(Debug)]
struct Record {
    report: ProgramStatusReport,
    updated: u64,
}

#[derive(Debug, Default)]
pub struct ProgramStatusRecords {
    records: Vec<Record>,
    clock: u64,
}

impl ProgramStatusRecords {
    pub fn apply(&mut self, report: ProgramStatusReport) {
        if report.state == ProgramStatusState::Clear {
            self.clear_subtree(&report.id);
            return;
        }
        self.clock = self.clock.wrapping_add(1);
        let updated = self.clock;
        if let Some(record) = self
            .records
            .iter_mut()
            .find(|record| record.report.id == report.id)
        {
            *record = Record { report, updated };
            return;
        }
        if self.records.len() >= MAX_PROGRAM_STATUS_RECORDS
            && let Some(stalest) = self
                .records
                .iter()
                .enumerate()
                .min_by_key(|(_, record)| record.updated)
                .map(|(index, _)| index)
        {
            self.records.swap_remove(stalest);
        }
        self.records.push(Record { report, updated });
    }

    pub fn release_finished_program(&mut self) {
        self.records.retain(|record| {
            matches!(
                record.report.state,
                ProgramStatusState::Done | ProgramStatusState::Error
            )
        });
    }

    pub fn acknowledge_seen(&mut self) {
        self.records.retain(|record| {
            !matches!(
                record.report.state,
                ProgramStatusState::Done | ProgramStatusState::Error
            )
        });
    }

    pub fn clear(&mut self) {
        self.records.clear();
    }

    pub fn current(&self) -> Option<&ProgramStatusReport> {
        self.records
            .iter()
            .find(|record| record.report.id.is_empty())
            .or_else(|| self.records.iter().max_by_key(|record| record.updated))
            .map(|record| &record.report)
    }

    #[cfg(test)]
    pub(crate) fn reports(&self) -> impl Iterator<Item = &ProgramStatusReport> {
        self.records.iter().map(|record| &record.report)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    fn clear_subtree(&mut self, id: &str) {
        if id.is_empty() {
            self.clear();
            return;
        }
        self.records
            .retain(|record| !is_within(&record.report.id, id));
    }
}

fn is_within(id: &str, subtree: &str) -> bool {
    id.strip_prefix(subtree)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

pub fn state_wire(state: ProgramStatusState) -> &'static str {
    match state {
        ProgramStatusState::Idle => "idle",
        ProgramStatusState::Working => "working",
        ProgramStatusState::Done => "done",
        ProgramStatusState::Blocked => "blocked",
        ProgramStatusState::Error => "error",
        ProgramStatusState::Clear => "clear",
    }
}

fn kind_wire(kind: ProgramStatusKind) -> &'static str {
    match kind {
        ProgramStatusKind::Permission => "permission",
        ProgramStatusKind::Question => "question",
        ProgramStatusKind::Auth => "auth",
    }
}

pub fn to_json(report: Option<&ProgramStatusReport>) -> Value {
    let Some(report) = report else {
        return Value::Null;
    };
    json!({
        "state": state_wire(report.state),
        "kind": report.kind.map(kind_wire),
        "progress": report.progress,
        "id": report.id,
        "app": report.app,
        "title": report.title,
        "message": report.message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(id: &str, state: ProgramStatusState) -> ProgramStatusReport {
        ProgramStatusReport {
            state,
            kind: None,
            progress: None,
            id: id.to_string(),
            app: String::new(),
            title: String::new(),
            message: String::new(),
        }
    }

    fn ids(records: &ProgramStatusRecords) -> Vec<&str> {
        let mut ids: Vec<&str> = records
            .records
            .iter()
            .map(|record| record.report.id.as_str())
            .collect();
        ids.sort_unstable();
        ids
    }

    #[test]
    fn a_report_replaces_its_whole_record() {
        let mut records = ProgramStatusRecords::default();
        let mut first = report("", ProgramStatusState::Blocked);
        first.message = "Apply?".into();
        first.kind = Some(ProgramStatusKind::Permission);
        first.progress = Some(40);
        records.apply(first);
        records.apply(report("", ProgramStatusState::Working));

        assert_eq!(records.len(), 1);
        assert_eq!(
            records.current(),
            Some(&report("", ProgramStatusState::Working))
        );
    }

    #[test]
    fn clearing_an_id_removes_its_subtree_and_nothing_else() {
        let mut records = ProgramStatusRecords::default();
        for id in [
            "build",
            "build/test",
            "build/test/unit",
            "builder",
            "deploy",
        ] {
            records.apply(report(id, ProgramStatusState::Working));
        }

        records.apply(report("build", ProgramStatusState::Clear));

        assert_eq!(ids(&records), ["builder", "deploy"]);
    }

    #[test]
    fn clearing_the_empty_id_removes_every_record() {
        let mut records = ProgramStatusRecords::default();
        for id in ["", "build", "deploy/eu"] {
            records.apply(report(id, ProgramStatusState::Done));
        }

        records.apply(report("", ProgramStatusState::Clear));

        assert!(records.is_empty());
        assert_eq!(records.current(), None);
    }

    fn every_state(records: &mut ProgramStatusRecords) {
        records.apply(report("a", ProgramStatusState::Working));
        records.apply(report("b", ProgramStatusState::Blocked));
        records.apply(report("c", ProgramStatusState::Done));
        records.apply(report("d", ProgramStatusState::Error));
        records.apply(report("e", ProgramStatusState::Idle));
    }

    #[test]
    fn a_finished_program_releases_working_blocked_and_idle_but_keeps_done_and_error() {
        let mut records = ProgramStatusRecords::default();
        every_state(&mut records);

        records.release_finished_program();

        assert_eq!(ids(&records), ["c", "d"]);
    }

    #[test]
    fn a_seen_acknowledgement_removes_done_and_error_only() {
        let mut records = ProgramStatusRecords::default();
        every_state(&mut records);

        records.acknowledge_seen();

        assert_eq!(ids(&records), ["a", "b", "e"]);
    }

    #[test]
    fn the_root_record_wins_over_a_more_recent_child() {
        let mut records = ProgramStatusRecords::default();
        records.apply(report("", ProgramStatusState::Blocked));
        records.apply(report("job", ProgramStatusState::Working));

        assert_eq!(
            records.current().map(|current| current.state),
            Some(ProgramStatusState::Blocked)
        );
    }

    #[test]
    fn without_a_root_the_most_recently_updated_record_is_current() {
        let mut records = ProgramStatusRecords::default();
        records.apply(report("eu", ProgramStatusState::Working));
        records.apply(report("us", ProgramStatusState::Done));
        records.apply(report("eu", ProgramStatusState::Blocked));

        assert_eq!(
            records.current().map(|current| current.id.as_str()),
            Some("eu")
        );
    }

    #[test]
    fn the_257th_record_evicts_the_least_recently_updated() {
        let mut records = ProgramStatusRecords::default();
        for index in 0..MAX_PROGRAM_STATUS_RECORDS {
            records.apply(report(&format!("job{index}"), ProgramStatusState::Working));
        }
        records.apply(report("job0", ProgramStatusState::Done));

        records.apply(report("job256", ProgramStatusState::Working));

        assert_eq!(records.len(), MAX_PROGRAM_STATUS_RECORDS);
        let ids = ids(&records);
        assert!(ids.contains(&"job0"));
        assert!(!ids.contains(&"job1"));
        assert!(ids.contains(&"job256"));
    }

    #[test]
    fn ten_thousand_distinct_ids_stay_bounded_and_fast() {
        let mut records = ProgramStatusRecords::default();
        let started = std::time::Instant::now();
        for index in 0..10_000 {
            records.apply(report(&format!("job{index}"), ProgramStatusState::Working));
        }

        assert_eq!(records.len(), MAX_PROGRAM_STATUS_RECORDS);
        assert!(records.records.capacity() <= MAX_PROGRAM_STATUS_RECORDS * 2);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(
            records.current().map(|current| current.id.as_str()),
            Some("job9999")
        );
    }

    #[test]
    fn the_json_capture_names_state_and_kind_on_the_wire() {
        let mut blocked = report("tf", ProgramStatusState::Blocked);
        blocked.kind = Some(ProgramStatusKind::Permission);
        blocked.message = "Apply?".into();

        assert_eq!(
            to_json(Some(&blocked)),
            json!({
                "state": "blocked",
                "kind": "permission",
                "progress": null,
                "id": "tf",
                "app": "",
                "title": "",
                "message": "Apply?",
            })
        );
        assert_eq!(to_json(None), Value::Null);
    }
}
