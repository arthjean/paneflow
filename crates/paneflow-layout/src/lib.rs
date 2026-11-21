mod events;
mod ids;
mod model;
mod ops;
mod session;
mod tree;
mod undo;

use std::collections::BTreeSet;

use paneflow_config::schema::SessionId;

pub use events::{Event, EventScope};
pub use ids::{IdBlock, IdSource, LEASE_BLOCK, PaneId, SplitId, SurfaceId, TabId, WorkspaceId};
pub use model::{
    Child, Direction, MAX_PANE_SURFACES, MAX_PANES, MAX_TABS_PER_WORKSPACE, Node, Pane, Split,
    State, Surface, SurfaceKind, SurfaceLocation, Tab, TabExtras, Workspace, WorkspaceExtras,
};
pub use ops::{Edge, NewNode, NewPane, NewSurface, NewTab, NewWorkspace, Op, Preset};
pub use session::{DetachedBounds, DetachedPlacement, Loaded, export, export_node, load};
pub use undo::{
    CLOSED_SESSION_GRACE_MS, ClosedPane, ClosedRecord, ClosedTab, HeldSession, MAX_CLOSED_RECORDS,
    UndoHistory,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpError {
    NotFound,
    Zoomed,
    PaneLimit,
    TabLimit,
    SurfaceLimit,
    IdsExhausted,
    Invalid,
}

impl OpError {
    pub fn reason(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::Zoomed => "zoomed",
            Self::PaneLimit => "pane_limit",
            Self::TabLimit => "tab_limit",
            Self::SurfaceLimit => "surface_limit",
            Self::IdsExhausted => "ids_exhausted",
            Self::Invalid => "invalid",
        }
    }
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason())
    }
}

impl std::error::Error for OpError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Spawn {
        surface: SurfaceId,
    },
    Reattach {
        surface: SurfaceId,
        session: SessionId,
    },
    Hold {
        session: SessionId,
        until_ms: u64,
    },
    Stop {
        session: SessionId,
    },
    Rehome {
        session: SessionId,
        workspace: WorkspaceId,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    pub revision: u64,
    pub events: Vec<Event>,
    pub scopes: Vec<EventScope>,
    pub effects: Vec<Effect>,
    pub focus: Option<PaneId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub revision: u64,
    pub state: State,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Snapshot {
    pub revision: u64,
    pub state: State,
    #[serde(default)]
    pub undo: UndoHistory,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    state: State,
    counter: IdSource,
    reserved: BTreeSet<u64>,
    revision: u64,
    undo: UndoHistory,
}

impl Default for Model {
    fn default() -> Self {
        Self::new()
    }
}

impl Model {
    pub fn new() -> Self {
        Self::from_parts(
            State {
                version: paneflow_config::schema::SESSION_SCHEMA_VERSION,
                workspaces: Vec::new(),
                active_workspace: 0,
                active_space: None,
            },
            IdSource::starting_at(1),
            UndoHistory::default(),
        )
    }

    pub(crate) fn from_parts(state: State, counter: IdSource, undo: UndoHistory) -> Self {
        Self {
            state,
            counter,
            reserved: BTreeSet::new(),
            revision: 0,
            undo,
        }
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn next_id(&self) -> u64 {
        self.counter.next_value()
    }

    pub fn ids(&self) -> IdSource {
        self.counter.clone()
    }

    pub fn with_ids(mut self, ids: IdSource) -> Self {
        ids.claim(self.counter.next_value().saturating_sub(1));
        self.counter = ids;
        self
    }

    pub fn undo_history(&self) -> &UndoHistory {
        &self.undo
    }

    pub fn view(&self) -> View {
        View {
            revision: self.revision,
            state: self.state.clone(),
        }
    }

    pub fn reserve(&mut self) -> u64 {
        let id = self.counter.allocate();
        self.reserved.insert(id);
        id
    }

    pub fn reserve_exact(&mut self, id: u64) -> bool {
        if id == 0
            || self.reserved.contains(&id)
            || self.state.contains_id(id)
            || self.undo.contains_id(id)
        {
            return false;
        }
        self.counter.claim(id);
        self.reserved.insert(id);
        true
    }

    pub fn reserve_surface(
        &mut self,
        definition: paneflow_config::schema::SurfaceDefinition,
    ) -> NewSurface {
        NewSurface {
            id: SurfaceId(self.reserve()),
            definition,
        }
    }

    pub fn reserve_pane(
        &mut self,
        definitions: Vec<paneflow_config::schema::SurfaceDefinition>,
        active: usize,
    ) -> NewPane {
        let id = PaneId(self.reserve());
        NewPane {
            id,
            surfaces: definitions
                .into_iter()
                .map(|definition| self.reserve_surface(definition))
                .collect(),
            active,
        }
    }

    pub fn reserve_node(&mut self, layout: &paneflow_config::schema::LayoutNode) -> NewNode {
        match layout {
            paneflow_config::schema::LayoutNode::Pane { surfaces } => {
                let active = surfaces
                    .iter()
                    .position(|surface| surface.focus == Some(true))
                    .unwrap_or(0);
                let mut definitions: Vec<_> =
                    surfaces.iter().take(MAX_PANE_SURFACES).cloned().collect();
                if definitions.is_empty() {
                    definitions.push(paneflow_config::schema::SurfaceDefinition::default());
                }
                NewNode::Pane(self.reserve_pane(definitions, active))
            }
            paneflow_config::schema::LayoutNode::Split {
                direction,
                children,
                ..
            } => NewNode::Split {
                direction: Direction::from_layout_str(direction),
                ratios: layout
                    .resolved_ratios()
                    .into_iter()
                    .map(|ratio| ratio as f32)
                    .collect(),
                children: children
                    .iter()
                    .map(|child| self.reserve_node(child))
                    .collect(),
            },
        }
    }

    pub fn apply(&mut self, op: Op, now_ms: u64) -> Result<Applied, OpError> {
        let ids = self.counter.clone();
        self.apply_with_ids(op, now_ms, &ids)
    }

    pub fn apply_with_ids(
        &mut self,
        op: Op,
        now_ms: u64,
        ids: &IdSource,
    ) -> Result<Applied, OpError> {
        let mut scratch = ops::Scratch {
            state: self.state.clone(),
            counter: ids.fork(),
            reserved: self.reserved.clone(),
            undo: self.undo.clone(),
            effects: Vec::new(),
            focus: None,
            now_ms,
        };
        ops::run(&mut scratch, op)?;
        if scratch.counter.is_overdrawn() {
            return Err(OpError::IdsExhausted);
        }
        let events = events::diff(&self.state, &scratch.state);
        let scopes = events
            .iter()
            .map(|event| event.scope(&self.state, &scratch.state))
            .collect();
        self.state = scratch.state;
        ids.adopt(&scratch.counter);
        if !ids.shares_with(&self.counter) {
            let highest = highest_id(&self.state);
            if highest >= self.counter.next_value() {
                self.counter.claim(highest);
            }
        }
        self.reserved = scratch.reserved;
        self.undo = scratch.undo;
        self.revision += 1;
        Ok(Applied {
            revision: self.revision,
            events,
            scopes,
            effects: scratch.effects,
            focus: scratch.focus,
        })
    }

    pub fn set_revision(&mut self, revision: u64) {
        self.revision = revision;
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            revision: self.revision,
            state: self.state.clone(),
            undo: self.undo.clone(),
        }
    }

    pub fn restore(&mut self, snapshot: Snapshot) -> Vec<Event> {
        let highest = highest_id(&snapshot.state).max(snapshot.undo.highest_id());
        if highest >= self.counter.next_value() {
            self.counter.claim(highest);
        }
        for id in collect_ids(&snapshot.state) {
            self.reserved.remove(&id);
        }
        let events = events::diff(&self.state, &snapshot.state);
        self.state = snapshot.state;
        self.undo = snapshot.undo;
        self.revision = snapshot.revision;
        events
    }

    pub fn is_unused_id(&self, id: u64) -> bool {
        id != 0 && !self.state.contains_id(id) && !self.undo.contains_id(id)
    }

    pub fn resync(&mut self, state: State) -> Option<Vec<Event>> {
        if self.state == state {
            return None;
        }
        let highest = highest_id(&state);
        if highest >= self.counter.next_value() {
            self.counter.claim(highest);
        }
        for id in collect_ids(&state) {
            self.reserved.remove(&id);
        }
        let events = events::diff(&self.state, &state);
        self.state = state;
        self.revision += 1;
        Some(events)
    }

    pub fn complete_spawn(&mut self, surface: SurfaceId, session: SessionId) -> bool {
        let Some(location) = self.state.find_surface(surface) else {
            return false;
        };
        let Some(target) = self.state.workspaces[location.workspace_index].tabs[location.tab_index]
            .root
            .as_mut()
            .and_then(|root| root.pane_mut(location.pane))
            .and_then(|pane| pane.surfaces.get_mut(location.index))
        else {
            return false;
        };
        if target.kind() != SurfaceKind::Terminal || target.definition.session.is_some() {
            return false;
        }
        target.definition.session = Some(session);
        true
    }

    pub fn forget_sessions(&mut self, sessions: &std::collections::HashSet<SessionId>) {
        self.undo.superseded_by(sessions);
    }

    pub fn expire_holds(&mut self, now_ms: u64) -> Vec<Effect> {
        self.undo
            .expire(now_ms)
            .into_iter()
            .map(|session| Effect::Stop { session })
            .collect()
    }

    pub fn held_sessions(&self) -> Vec<SessionId> {
        self.undo.held_sessions()
    }
}

fn collect_ids(state: &State) -> Vec<u64> {
    let mut ids = Vec::new();
    for ws in &state.workspaces {
        ids.push(ws.id.get());
        for tab in &ws.tabs {
            ids.push(tab.id.get());
            if let Some(root) = &tab.root {
                ids.extend(root.split_ids().into_iter().map(SplitId::get));
                for pane in root.panes() {
                    ids.push(pane.id.get());
                    ids.extend(pane.surfaces.iter().map(|surface| surface.id.get()));
                }
            }
        }
    }
    ids
}

impl State {
    pub fn contains_id(&self, id: u64) -> bool {
        collect_ids(self).contains(&id)
    }
}

fn highest_id(state: &State) -> u64 {
    collect_ids(state).into_iter().max().unwrap_or(0)
}
