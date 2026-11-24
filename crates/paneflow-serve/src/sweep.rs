use std::collections::HashSet;

use paneflow_config::schema::SessionGeneration;
use paneflow_config::schema::SessionId;
use paneflow_host::SessionLifecycle;
use paneflow_layout::State;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedRecord {
    pub session: SessionId,
    pub generation: SessionGeneration,
    pub owned: bool,
    pub live: bool,
    pub lifecycle: SessionLifecycle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Orphan {
    pub session: SessionId,
    pub generation: SessionGeneration,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct OrphanSweep {
    pub forget: Vec<SessionId>,
    pub stop: Vec<Orphan>,
}

fn has_ended(lifecycle: &SessionLifecycle) -> bool {
    matches!(
        lifecycle,
        SessionLifecycle::Exited { .. } | SessionLifecycle::Failed { .. } | SessionLifecycle::Lost
    )
}

pub fn orphan_sweep(
    listed: &[ListedRecord],
    held: &HashSet<SessionId>,
    forgetting: &HashSet<SessionId>,
    sweep_live: bool,
) -> OrphanSweep {
    let mut sweep = OrphanSweep::default();
    for record in listed {
        if held.contains(&record.session) || forgetting.contains(&record.session) {
            continue;
        }
        if !record.live && has_ended(&record.lifecycle) {
            sweep.forget.push(record.session.clone());
        } else if sweep_live && record.live && record.owned {
            sweep.stop.push(Orphan {
                session: record.session.clone(),
                generation: record.generation,
            });
        }
    }
    sweep
}

pub fn layout_sessions(state: &State) -> HashSet<SessionId> {
    state
        .workspaces
        .iter()
        .flat_map(|ws| ws.tabs.iter())
        .filter_map(|tab| tab.root.as_ref())
        .flat_map(paneflow_layout::Node::panes)
        .flat_map(|pane| pane.surfaces.iter())
        .filter_map(|surface| surface.definition.session.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(owned: bool, lifecycle: SessionLifecycle) -> ListedRecord {
        ListedRecord {
            session: SessionId::new(),
            generation: SessionGeneration::FIRST,
            owned,
            live: matches!(lifecycle, SessionLifecycle::Running),
            lifecycle,
        }
    }

    fn exited() -> SessionLifecycle {
        SessionLifecycle::Exited {
            code: 0,
            signal: None,
        }
    }

    #[test]
    fn every_ended_record_no_view_holds_is_forgotten_whoever_owns_it() {
        let held = record(true, exited());
        let in_flight = record(true, exited());
        let lost_from_a_previous_host = record(false, SessionLifecycle::Lost);
        let failed = record(
            true,
            SessionLifecycle::Failed {
                reason: "shell not found".to_string(),
            },
        );
        let orphan = record(true, exited());
        let unverified = record(
            true,
            SessionLifecycle::Unverified {
                reason: "descendants unresolved".to_string(),
            },
        );
        let starting = record(true, SessionLifecycle::Starting);
        let listed = vec![
            held.clone(),
            in_flight.clone(),
            lost_from_a_previous_host.clone(),
            failed.clone(),
            orphan.clone(),
            unverified,
            starting,
        ];

        let sweep = orphan_sweep(
            &listed,
            &HashSet::from([held.session.clone()]),
            &HashSet::from([in_flight.session.clone()]),
            false,
        );

        assert_eq!(
            sweep.forget,
            vec![
                lost_from_a_previous_host.session,
                failed.session,
                orphan.session
            ],
            "a held or in-flight record stays, and an unverified or starting one may still own a process"
        );
        assert!(sweep.stop.is_empty());
    }

    #[test]
    fn a_live_orphan_is_stopped_only_by_the_sweep_that_follows_a_restore() {
        let attached = record(true, SessionLifecycle::Running);
        let orphan = record(true, SessionLifecycle::Running);
        let foreign = record(false, SessionLifecycle::Running);
        let listed = vec![attached.clone(), orphan.clone(), foreign];
        let held = HashSet::from([attached.session.clone()]);

        assert_eq!(
            orphan_sweep(&listed, &held, &HashSet::new(), false),
            OrphanSweep::default(),
            "a routine refresh never stops a live session"
        );
        assert_eq!(
            orphan_sweep(&listed, &held, &HashSet::new(), true).stop,
            vec![Orphan {
                session: orphan.session,
                generation: orphan.generation,
            }],
            "only an owned live session that nothing holds is stopped"
        );
    }
}
