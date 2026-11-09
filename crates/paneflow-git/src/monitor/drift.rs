use std::collections::{BTreeMap, BTreeSet};

use crate::{Entry, Snapshot, WorktreeChange};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Drift {
    pub(crate) paths: usize,
    pub(crate) kinds: BTreeMap<&'static str, usize>,
    pub(crate) head: bool,
    pub(crate) truncated: bool,
}

impl Drift {
    pub(crate) fn between(modeled: &Snapshot, full: &Snapshot) -> Self {
        let modeled_entries = by_path(modeled);
        let full_entries = by_path(full);
        let paths: BTreeSet<&[u8]> = modeled_entries
            .keys()
            .chain(full_entries.keys())
            .copied()
            .chain(modeled.sides.keys().map(Vec::as_slice))
            .chain(full.sides.keys().map(Vec::as_slice))
            .collect();
        let mut drift = Drift {
            head: modeled.head != full.head,
            truncated: modeled.truncated != full.truncated,
            ..Drift::default()
        };
        for path in paths {
            let left = modeled_entries.get(path).copied();
            let right = full_entries.get(path).copied();
            if left == right && modeled.sides.get(path) == full.sides.get(path) {
                continue;
            }
            drift.paths += 1;
            *drift.kinds.entry(kind_of(right)).or_default() += 1;
        }
        drift
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.paths == 0 && !self.head && !self.truncated
    }
}

impl std::fmt::Display for Drift {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} paths differ", self.paths)?;
        if !self.kinds.is_empty() {
            let kinds: Vec<String> = self
                .kinds
                .iter()
                .map(|(kind, count)| format!("{count} {kind}"))
                .collect();
            write!(f, " (full snapshot: {})", kinds.join(", "))?;
        }
        if self.head {
            write!(f, ", the compared tree differs")?;
        }
        if self.truncated {
            write!(f, ", truncation differs")?;
        }
        Ok(())
    }
}

fn by_path(snapshot: &Snapshot) -> BTreeMap<&[u8], &Entry> {
    snapshot
        .entries
        .iter()
        .map(|entry| (entry.path.as_slice(), entry))
        .collect()
}

fn kind_of(entry: Option<&Entry>) -> &'static str {
    let Some(entry) = entry else {
        return "clean";
    };
    if entry.conflict.is_some() {
        return "conflicted";
    }
    if entry.untracked {
        return "untracked";
    }
    match entry.index_to_worktree {
        Some(WorktreeChange::Modified) => "modified",
        Some(WorktreeChange::Deleted) => "deleted",
        Some(WorktreeChange::TypeChange) => "type_change",
        Some(WorktreeChange::IntentToAdd) => "intent_to_add",
        Some(WorktreeChange::Submodule(_)) => "submodule",
        None if entry.head_to_index.is_some() => "staged",
        None if entry.stat_dirty => "stat_dirty",
        None => "clean",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Head;

    fn snapshot(entries: Vec<Entry>) -> Snapshot {
        Snapshot {
            head: Head::Unborn,
            prefix: Vec::new(),
            entries,
            sides: BTreeMap::new(),
            truncated: false,
            submodules_opened: 0,
        }
    }

    fn entry(path: &str, change: Option<WorktreeChange>, untracked: bool) -> Entry {
        Entry {
            path: path.as_bytes().to_vec(),
            index_to_worktree: change,
            untracked,
            ..Entry::default()
        }
    }

    #[test]
    fn equal_snapshots_show_no_drift_even_when_submodule_counts_differ() {
        let left = snapshot(vec![entry("a", Some(WorktreeChange::Modified), false)]);
        let right = Snapshot {
            submodules_opened: 3,
            ..left.clone()
        };
        assert!(Drift::between(&left, &right).is_empty());
    }

    #[test]
    fn a_drift_counts_each_differing_path_under_the_full_snapshot_kind_and_names_no_path() {
        let modeled = snapshot(vec![entry("gone", Some(WorktreeChange::Modified), false)]);
        let full = snapshot(vec![
            entry("mapped.txt", Some(WorktreeChange::Modified), false),
            entry("new.txt", None, true),
        ]);
        let drift = Drift::between(&modeled, &full);
        assert_eq!(drift.paths, 3);
        assert_eq!(
            drift.kinds,
            BTreeMap::from([("clean", 1), ("modified", 1), ("untracked", 1)])
        );
        let line = drift.to_string();
        assert_eq!(
            line,
            "3 paths differ (full snapshot: 1 clean, 1 modified, 1 untracked)"
        );
        assert!(!line.contains("mapped"));
    }
}
