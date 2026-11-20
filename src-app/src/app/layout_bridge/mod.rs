mod adopt;
mod arrange;
mod effects;
mod gestures;
#[cfg(test)]
mod guard;
mod project;
pub(crate) mod publish;
mod select;
mod stage;

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::time::Instant;

use gpui::{Context, Entity, EntityId};
use paneflow_config::schema::{SessionId, SurfaceDefinition};
use paneflow_layout::{
    Applied, Model, Op, OpError, PaneId, SplitId, SurfaceId, TabId, WorkspaceId,
};

use crate::PaneFlowApp;
use crate::layout::SplitCells;
use crate::pane::{Pane, PaneSurface};
use crate::workspace::Workspace;

pub(crate) use effects::HeldSurface;
pub(crate) use gestures::model_direction;

pub(crate) type Spawner =
    Box<dyn FnOnce(&mut PaneFlowApp, u64, &mut Context<PaneFlowApp>) -> PaneSurface>;

pub(crate) struct LayoutBridge {
    model: Model,
    epoch: Instant,
    panes: HashMap<PaneId, Entity<Pane>>,
    pane_ids: HashMap<EntityId, PaneId>,
    surfaces: HashMap<SurfaceId, PaneSurface>,
    surface_ids: HashMap<EntityId, SurfaceId>,
    workspaces: HashMap<WorkspaceId, u64>,
    workspace_ids: HashMap<u64, WorkspaceId>,
    tabs: HashMap<TabId, u64>,
    tab_ids: HashMap<u64, TabId>,
    splits: HashMap<SplitId, SplitCells>,
    split_ids: HashMap<usize, SplitId>,
    resizing: Option<usize>,
    saved_revision: u64,
    save_scheduled: bool,
    staged_surfaces: HashMap<SurfaceId, Spawner>,
    staged_workspaces: HashMap<WorkspaceId, Workspace>,
    held: HashMap<SessionId, HeldSurface>,
    events: publish::LayoutEventSink,
}

impl Default for LayoutBridge {
    fn default() -> Self {
        Self::new(Model::new())
    }
}

impl LayoutBridge {
    pub(crate) fn new(model: Model) -> Self {
        Self {
            model: model.with_ids(crate::layout_ids::source()),
            epoch: Instant::now(),
            panes: HashMap::new(),
            pane_ids: HashMap::new(),
            surfaces: HashMap::new(),
            surface_ids: HashMap::new(),
            workspaces: HashMap::new(),
            workspace_ids: HashMap::new(),
            tabs: HashMap::new(),
            tab_ids: HashMap::new(),
            splits: HashMap::new(),
            split_ids: HashMap::new(),
            resizing: None,
            saved_revision: 0,
            save_scheduled: false,
            staged_surfaces: HashMap::new(),
            staged_workspaces: HashMap::new(),
            held: HashMap::new(),
            events: publish::LayoutEventSink::default(),
        }
    }

    pub(crate) fn publish_applied(&self, applied: &Applied) {
        self.events.publish(publish::published_events(
            &applied.events,
            &applied.scopes,
            applied.revision,
        ));
    }

    #[cfg(test)]
    pub(crate) fn model(&self) -> &Model {
        &self.model
    }

    pub(crate) fn ms_at(&self, at: Instant) -> u64 {
        u64::try_from(at.saturating_duration_since(self.epoch).as_millis()).unwrap_or(u64::MAX)
    }

    pub(crate) fn reserve_surface(
        &mut self,
        definition: SurfaceDefinition,
        spawner: Spawner,
    ) -> paneflow_layout::NewSurface {
        let surface = self.model.reserve_surface(definition);
        self.staged_surfaces.insert(surface.id, spawner);
        surface
    }

    pub(crate) fn reserve_pane(
        &mut self,
        definition: SurfaceDefinition,
        spawner: Spawner,
    ) -> paneflow_layout::NewPane {
        let surface = self.reserve_surface(definition, spawner);
        paneflow_layout::NewPane {
            id: PaneId(self.model.reserve()),
            surfaces: vec![surface],
            active: 0,
        }
    }

    pub(crate) fn stage_workspace(&mut self, id: WorkspaceId, workspace: Workspace) {
        self.workspace_ids.insert(workspace.id, id);
        self.workspaces.insert(id, workspace.id);
        self.staged_workspaces.insert(id, workspace);
    }

    pub(crate) fn pane_id(&self, pane: &Entity<Pane>) -> Option<PaneId> {
        self.pane_ids.get(&pane.entity_id()).copied()
    }

    pub(crate) fn pane(&self, id: PaneId) -> Option<Entity<Pane>> {
        self.panes.get(&id).cloned()
    }

    pub(crate) fn surface_id_of(&self, entity: EntityId) -> Option<SurfaceId> {
        self.surface_ids.get(&entity).copied()
    }

    pub(crate) fn surface(&self, id: SurfaceId) -> Option<PaneSurface> {
        self.surfaces.get(&id).cloned()
    }

    pub(crate) fn workspace_id(&self, app_id: u64) -> Option<WorkspaceId> {
        self.workspace_ids.get(&app_id).copied()
    }

    pub(crate) fn app_workspace_id(&self, id: WorkspaceId) -> Option<u64> {
        self.workspaces.get(&id).copied()
    }

    pub(crate) fn tab_id(&self, app_id: u64) -> Option<TabId> {
        self.tab_ids.get(&app_id).copied()
    }

    pub(crate) fn app_tab_id(&self, id: TabId) -> Option<u64> {
        self.tabs.get(&id).copied()
    }

    fn register_pane(&mut self, id: PaneId, pane: Entity<Pane>) {
        self.pane_ids.insert(pane.entity_id(), id);
        self.panes.insert(id, pane);
    }

    fn register_surface(&mut self, id: SurfaceId, surface: PaneSurface) {
        self.surface_ids.insert(surface_entity(&surface), id);
        self.surfaces.insert(id, surface);
    }

    fn prune(&mut self) {
        let state = self.model.state();
        let mut panes = std::collections::HashSet::new();
        let mut surfaces = std::collections::HashSet::new();
        let mut splits = std::collections::HashSet::new();
        let mut tabs = std::collections::HashSet::new();
        let mut workspaces = std::collections::HashSet::new();
        for ws in &state.workspaces {
            workspaces.insert(ws.id);
            for tab in &ws.tabs {
                tabs.insert(tab.id);
                let Some(root) = &tab.root else {
                    continue;
                };
                splits.extend(root.split_ids());
                for pane in root.panes() {
                    panes.insert(pane.id);
                    surfaces.extend(pane.surfaces.iter().map(|surface| surface.id));
                }
            }
        }
        self.panes.retain(|id, _| panes.contains(id));
        self.pane_ids.retain(|_, id| panes.contains(id));
        self.surfaces.retain(|id, _| surfaces.contains(id));
        self.surface_ids.retain(|_, id| surfaces.contains(id));
        self.splits.retain(|id, _| splits.contains(id));
        self.split_ids.retain(|_, id| splits.contains(id));
        self.tabs.retain(|id, _| tabs.contains(id));
        self.tab_ids.retain(|_, id| tabs.contains(id));
        self.workspaces.retain(|id, _| workspaces.contains(id));
        self.workspace_ids.retain(|_, id| workspaces.contains(id));
        self.staged_surfaces.clear();
        self.staged_workspaces.clear();
    }
}

pub(crate) fn subscribed_terminal(
    view: Entity<crate::terminal::TerminalView>,
    cx: &mut Context<PaneFlowApp>,
) -> PaneSurface {
    cx.subscribe(&view, PaneFlowApp::handle_terminal_event)
        .detach();
    PaneSurface::Terminal(view)
}

pub(crate) fn surface_entity(surface: &PaneSurface) -> EntityId {
    match surface {
        PaneSurface::Terminal(view) => view.entity_id(),
        PaneSurface::Markdown(view) => view.entity_id(),
    }
}

fn op_label(op: &Op) -> String {
    let debug = format!("{op:?}");
    debug
        .split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or_default()
        .to_string()
}

impl PaneFlowApp {
    pub(crate) fn apply_layout_op(
        &mut self,
        op: Op,
        cx: &mut Context<Self>,
    ) -> Result<Applied, OpError> {
        let label = op_label(&op);
        self.apply_layout_with(&label, move |model, now_ms| model.apply(op, now_ms), cx)
    }

    #[cfg(test)]
    pub(crate) fn apply_panicking_layout_op(&mut self, cx: &mut Context<Self>) -> OpError {
        let outcome = self.apply_layout_with(
            "Split",
            |_, _| -> Result<Applied, OpError> { panic!("a layout op panicked in a test build") },
            cx,
        );
        outcome.err().unwrap_or(OpError::NotFound)
    }

    fn apply_layout_with(
        &mut self,
        label: &str,
        apply: impl FnOnce(&mut Model, u64) -> Result<Applied, OpError>,
        cx: &mut Context<Self>,
    ) -> Result<Applied, OpError> {
        self.adopt_layout(cx);
        let now_ms = self.layout.ms_at(Instant::now());
        let model = &mut self.layout.model;
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| apply(model, now_ms)));
        let applied = match outcome {
            Ok(Ok(applied)) => {
                self.layout.publish_applied(&applied);
                applied
            }
            Ok(Err(error)) => {
                self.layout.prune();
                return Err(error);
            }
            Err(_) => {
                tracing::error!(
                    op = label,
                    "layout op panicked: the window keeps its last layout"
                );
                self.layout.prune();
                return Err(OpError::Invalid);
            }
        };
        self.run_layout_effects(&applied, cx);
        self.project_layout(cx);
        self.run_layout_rehomes(&applied, cx);
        if let Some(pane) = applied.focus.and_then(|id| self.layout.pane(id)) {
            self.pending_pane_focus = Some(pane);
        }
        self.layout.prune();
        self.layout_revision_changed(cx);
        cx.notify();
        Ok(applied)
    }

    pub(crate) fn start_layout_event_stream(&mut self) {
        if let Some(endpoint) = paneflow_home::serve_endpoint_path_for_current_home() {
            self.layout.events.start(endpoint);
        }
    }

    #[cfg(test)]
    pub(crate) fn pending_layout_events(
        &self,
    ) -> Vec<paneflow_api::methods::events::PublishedEvent> {
        self.layout.events.pending()
    }

    pub(crate) fn layout_revision_changed(&mut self, cx: &mut Context<Self>) {
        if self.layout.model.revision() == self.layout.saved_revision || self.layout.save_scheduled
        {
            return;
        }
        self.layout.save_scheduled = true;
        let app = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = app.update(cx, |app, cx| {
                app.layout.save_scheduled = false;
                app.layout.saved_revision = app.layout.model.revision();
                app.save_session(cx);
            });
        });
    }

    pub(crate) fn layout_pane_id(&self, pane: &Entity<Pane>) -> Option<PaneId> {
        self.layout.pane_id(pane)
    }

    pub(crate) fn layout_pane(&self, id: PaneId) -> Option<Entity<Pane>> {
        self.layout.pane(id)
    }

    pub(crate) fn set_detached_through_model(
        &mut self,
        pane: &Entity<Pane>,
        detached: bool,
        cx: &mut Context<Self>,
    ) {
        self.adopt_layout(cx);
        let Some(id) = self.layout.pane_id(pane) else {
            return;
        };
        if let Err(error) = self.apply_layout_op(Op::SetDetached { pane: id, detached }, cx) {
            log::debug!("layout: the detached state of a pane was refused: {error}");
        }
    }

    pub(crate) fn next_layout_id(&self) -> u64 {
        self.layout.model.next_id()
    }

    pub(crate) fn workspace_layout_id(&self, ws: &Workspace) -> Option<u64> {
        self.layout.workspace_ids.get(&ws.id).map(|id| id.0)
    }

    pub(crate) fn tab_layout_ids(
        &self,
        tab: &crate::workspace::Tab,
        cx: &gpui::App,
    ) -> Option<paneflow_config::schema::TabLayoutIds> {
        let id = self.layout.tab_ids.get(&tab.id)?;
        let leaves = tab
            .saved_layout
            .as_ref()
            .or(tab.root.as_ref())
            .map(crate::layout::LayoutTree::collect_leaves)
            .unwrap_or_default();
        let panes = leaves
            .iter()
            .map(|pane| self.layout.pane_id(pane).map(|id| id.0))
            .collect::<Option<Vec<u64>>>()?;
        let surfaces = leaves
            .iter()
            .map(|pane| {
                pane.read(cx)
                    .surfaces()
                    .iter()
                    .map(|surface| {
                        self.layout
                            .surface_id_of(surface_entity(surface))
                            .map(|id| id.0)
                    })
                    .collect::<Option<Vec<u64>>>()
            })
            .collect::<Option<Vec<Vec<u64>>>>()?;
        Some(paneflow_config::schema::TabLayoutIds {
            tab: id.0,
            panes,
            surfaces,
        })
    }

    pub(crate) fn terminal_for_surface(
        &self,
        surface_id: u64,
    ) -> Option<Entity<crate::terminal::TerminalView>> {
        let surface = SurfaceId(surface_id);
        self.layout.model.state().find_surface(surface)?;
        match self.layout.surfaces.get(&surface)? {
            PaneSurface::Terminal(view) => Some(view.clone()),
            PaneSurface::Markdown(_) => None,
        }
    }

    pub(crate) fn locate_surface(
        &self,
        surface_id: u64,
    ) -> Option<crate::app::ipc_handler::SurfaceLocation> {
        self.terminal_for_surface(surface_id)?;
        let state = self.layout.model.state();
        let location = state.find_surface(SurfaceId(surface_id))?;
        let (workspace_idx, tab_idx) = state.find_pane(location.pane)?;
        let pane = self.layout.panes.get(&location.pane)?.clone();
        Some(crate::app::ipc_handler::SurfaceLocation {
            workspace_idx,
            tab_idx,
            pane,
        })
    }
}
