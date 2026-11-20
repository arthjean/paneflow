use std::collections::{HashSet, VecDeque};

use paneflow_config::schema::{LayoutNode, SessionId, TabTitleSource};

use crate::ids::{PaneId, TabId, WorkspaceId};
use crate::model::Pane;

pub const MAX_CLOSED_RECORDS: usize = 5;
pub const CLOSED_SESSION_GRACE_MS: u64 = 5_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldSession {
    pub session: SessionId,
    pub until_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClosedPane {
    pub workspace: WorkspaceId,
    pub tab: TabId,
    pub worktree: Option<String>,
    pub pane: Pane,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClosedTab {
    pub workspace: WorkspaceId,
    pub tab: TabId,
    pub index: usize,
    pub title: String,
    pub title_source: TabTitleSource,
    pub worktree: Option<String>,
    pub layout: LayoutNode,
    pub panes: Vec<Pane>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClosedRecord {
    Pane(ClosedPane),
    Tab(ClosedTab),
}

impl ClosedRecord {
    fn panes(&self) -> &[Pane] {
        match self {
            Self::Pane(record) => std::slice::from_ref(&record.pane),
            Self::Tab(record) => &record.panes,
        }
    }

    pub fn sessions(&self) -> Vec<SessionId> {
        self.panes()
            .iter()
            .flat_map(|pane| pane.surfaces.iter())
            .filter_map(|surface| surface.session().cloned())
            .collect()
    }

    pub fn pane_ids(&self) -> Vec<PaneId> {
        self.panes().iter().map(|pane| pane.id).collect()
    }

    fn contains_id(&self, id: u64) -> bool {
        let tab = match self {
            Self::Pane(_) => None,
            Self::Tab(record) => Some(record.tab.get()),
        };
        tab == Some(id)
            || self.panes().iter().any(|pane| {
                pane.id.get() == id || pane.surfaces.iter().any(|surface| surface.id.get() == id)
            })
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct UndoHistory {
    records: VecDeque<ClosedRecord>,
    held: Vec<HeldSession>,
}

impl UndoHistory {
    pub fn records(&self) -> impl Iterator<Item = &ClosedRecord> {
        self.records.iter()
    }

    pub fn contains_id(&self, id: u64) -> bool {
        self.records.iter().any(|record| record.contains_id(id))
    }

    pub fn newest(&self) -> Option<&ClosedRecord> {
        self.records.back()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn held(&self) -> &[HeldSession] {
        &self.held
    }

    pub fn held_sessions(&self) -> Vec<SessionId> {
        self.held.iter().map(|held| held.session.clone()).collect()
    }

    pub fn holds(&self, session: &SessionId) -> bool {
        self.held.iter().any(|held| &held.session == session)
    }

    pub(crate) fn push(&mut self, record: ClosedRecord) -> Vec<SessionId> {
        let mut evicted = Vec::new();
        if self.records.len() >= MAX_CLOSED_RECORDS
            && let Some(oldest) = self.records.pop_front()
        {
            for session in oldest.sessions() {
                if self.release(&session) {
                    evicted.push(session);
                }
            }
        }
        self.records.push_back(record);
        evicted
    }

    pub(crate) fn hold(&mut self, session: SessionId, until_ms: u64) {
        self.held.retain(|held| held.session != session);
        self.held.push(HeldSession { session, until_ms });
    }

    pub(crate) fn release(&mut self, session: &SessionId) -> bool {
        let before = self.held.len();
        self.held.retain(|held| &held.session != session);
        before != self.held.len()
    }

    pub(crate) fn expire(&mut self, now_ms: u64) -> Vec<SessionId> {
        let expired: Vec<SessionId> = self
            .held
            .iter()
            .filter(|held| held.until_ms <= now_ms)
            .map(|held| held.session.clone())
            .collect();
        self.held.retain(|held| held.until_ms > now_ms);
        expired
    }

    pub(crate) fn pop(&mut self) -> Option<ClosedRecord> {
        self.records.pop_back()
    }

    pub(crate) fn take_tab(&mut self, tab: TabId) -> Option<ClosedRecord> {
        let position = self
            .records
            .iter()
            .rposition(|record| matches!(record, ClosedRecord::Tab(closed) if closed.tab == tab))?;
        self.records.remove(position)
    }

    pub(crate) fn superseded_by(&mut self, sessions: &HashSet<SessionId>) {
        for record in &mut self.records {
            let panes: &mut [Pane] = match record {
                ClosedRecord::Pane(closed) => std::slice::from_mut(&mut closed.pane),
                ClosedRecord::Tab(closed) => &mut closed.panes,
            };
            for surface in panes.iter_mut().flat_map(|pane| pane.surfaces.iter_mut()) {
                if surface
                    .definition
                    .session
                    .as_ref()
                    .is_some_and(|session| sessions.contains(session))
                {
                    surface.definition.session = None;
                }
            }
        }
        self.held.retain(|held| !sessions.contains(&held.session));
    }
}
