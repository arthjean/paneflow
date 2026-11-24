use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};

use paneflow_config::schema::{
    ButtonCommand, LayoutNode, ManagedWorktreeDef, SessionId, SpaceId, SurfaceDefinition,
    TabTitleSource,
};

use crate::ids::{IdSource, PaneId, SplitId, SurfaceId, TabId, WorkspaceId};
use crate::model::{
    Child, Direction, MAX_PANE_SURFACES, MAX_PANES, MAX_TABS_PER_WORKSPACE, Node, Pane, Split,
    State, Surface, SurfaceKind, Tab, TabExtras, Workspace, WorkspaceExtras,
};
use crate::session::{empty_tab, export_node};
use crate::tree;
use crate::undo::{CLOSED_SESSION_GRACE_MS, ClosedPane, ClosedRecord, ClosedTab, UndoHistory};
use crate::{Effect, OpError};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewSurface {
    pub id: SurfaceId,
    pub definition: SurfaceDefinition,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewPane {
    pub id: PaneId,
    pub surfaces: Vec<NewSurface>,
    pub active: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NewNode {
    Pane(NewPane),
    Split {
        direction: Direction,
        ratios: Vec<f32>,
        children: Vec<NewNode>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewTab {
    pub id: TabId,
    pub title: String,
    pub title_source: TabTitleSource,
    pub worktree: Option<String>,
    pub root: Option<NewNode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewWorkspace {
    pub id: WorkspaceId,
    pub title: String,
    pub cwd: String,
    pub space: Option<SpaceId>,
    pub durable_id: Option<paneflow_config::schema::WorkspaceId>,
    pub custom_buttons: Vec<ButtonCommand>,
    pub managed_worktrees: Vec<ManagedWorktreeDef>,
    pub tabs: Vec<NewTab>,
    pub active_tab: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    Before,
    After,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    EvenHorizontal,
    EvenVertical,
    Tiled,
    MainVertical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Split {
        target: PaneId,
        direction: Direction,
        edge: Edge,
        pane: NewPane,
        focus: bool,
    },
    AddSurface {
        pane: PaneId,
        surface: Box<NewSurface>,
    },
    SelectSurface {
        surface: SurfaceId,
    },
    OpenTab {
        workspace: WorkspaceId,
        tab: NewTab,
        fill_empty_active: bool,
    },
    CreateWorkspace {
        workspace: NewWorkspace,
        select: bool,
    },
    CloseSurface {
        surface: SurfaceId,
    },
    ClosePane {
        pane: PaneId,
        keep_undo: bool,
    },
    CloseTab {
        tab: TabId,
        keep_sessions: bool,
    },
    CloseWorkspace {
        workspace: WorkspaceId,
        focus: bool,
    },
    FocusPane {
        pane: PaneId,
    },
    SelectTab {
        tab: TabId,
    },
    SelectWorkspace {
        workspace: WorkspaceId,
        focus: bool,
    },
    RenameSurface {
        surface: SurfaceId,
        name: Option<String>,
    },
    RenameTab {
        tab: TabId,
        title: String,
        source: TabTitleSource,
    },
    UnlockTabTitle {
        tab: TabId,
    },
    RenameWorkspace {
        workspace: WorkspaceId,
        title: String,
    },
    SetWorkspaceCwd {
        workspace: WorkspaceId,
        cwd: String,
    },
    BindSession {
        surface: SurfaceId,
        session: SessionId,
    },
    UnbindSession {
        surface: SurfaceId,
    },
    UpdateSurface {
        surface: SurfaceId,
        definition: Box<SurfaceDefinition>,
    },
    BindWorktree {
        tab: TabId,
        worktree: Option<String>,
    },
    SwapPanes {
        a: PaneId,
        b: PaneId,
    },
    MovePane {
        pane: PaneId,
        target: PaneId,
        direction: Direction,
        edge: Edge,
    },
    MoveSurface {
        surface: SurfaceId,
        target: PaneId,
        split: Option<(Direction, Edge, PaneId)>,
    },
    MovePaneToNewTab {
        pane: PaneId,
        workspace: WorkspaceId,
        tab: TabId,
        index: Option<usize>,
    },
    MoveTab {
        tab: TabId,
        workspace: WorkspaceId,
        index: Option<usize>,
    },
    ReorderTab {
        tab: TabId,
        to: usize,
    },
    ReorderWorkspace {
        workspace: WorkspaceId,
        to: usize,
    },
    SetRatios {
        split: SplitId,
        ratios: Vec<f32>,
    },
    Equalize {
        tab: TabId,
    },
    ApplyPreset {
        tab: TabId,
        preset: Preset,
    },
    SetZoom {
        tab: TabId,
        pane: Option<PaneId>,
    },
    SetDetached {
        pane: PaneId,
        detached: bool,
    },
    RestoreLayout {
        tab: TabId,
        layout: NewNode,
    },
    LaunchTemplate {
        workspace: WorkspaceId,
        root: NewNode,
        focus: Option<PaneId>,
        cwd: Option<String>,
    },
    UndoClose {
        tab: Option<TabId>,
    },
}

fn pane_ids(pane: &NewPane, ids: &mut Vec<u64>) {
    ids.push(pane.id.0);
    ids.extend(pane.surfaces.iter().map(|surface| surface.id.0));
}

fn node_ids(node: &NewNode, ids: &mut Vec<u64>) {
    match node {
        NewNode::Pane(pane) => pane_ids(pane, ids),
        NewNode::Split { children, .. } => {
            for child in children {
                node_ids(child, ids);
            }
        }
    }
}

fn tab_ids(tab: &NewTab, ids: &mut Vec<u64>) {
    ids.push(tab.id.0);
    if let Some(root) = &tab.root {
        node_ids(root, ids);
    }
}

impl Op {
    pub fn new_ids(&self) -> Vec<u64> {
        let mut ids = Vec::new();
        match self {
            Self::Split { pane, .. } => pane_ids(pane, &mut ids),
            Self::AddSurface { surface, .. } => ids.push(surface.id.0),
            Self::OpenTab { tab, .. } => tab_ids(tab, &mut ids),
            Self::CreateWorkspace { workspace, .. } => {
                ids.push(workspace.id.0);
                for tab in &workspace.tabs {
                    tab_ids(tab, &mut ids);
                }
            }
            Self::MovePaneToNewTab { tab, .. } => ids.push(tab.0),
            Self::MoveSurface {
                split: Some((_, _, pane)),
                ..
            } => ids.push(pane.0),
            Self::RestoreLayout { layout, .. } => node_ids(layout, &mut ids),
            Self::LaunchTemplate { root, .. } => node_ids(root, &mut ids),
            _ => {}
        }
        ids
    }

    pub fn creates(&self) -> bool {
        !self.new_ids().is_empty()
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Split { .. } => "split",
            Self::AddSurface { .. } => "add_surface",
            Self::SelectSurface { .. } => "select_surface",
            Self::OpenTab { .. } => "open_tab",
            Self::CreateWorkspace { .. } => "create_workspace",
            Self::CloseSurface { .. } => "close_surface",
            Self::ClosePane { .. } => "close_pane",
            Self::CloseTab { .. } => "close_tab",
            Self::CloseWorkspace { .. } => "close_workspace",
            Self::FocusPane { .. } => "focus_pane",
            Self::SelectTab { .. } => "select_tab",
            Self::SelectWorkspace { .. } => "select_workspace",
            Self::RenameSurface { .. } => "rename_surface",
            Self::RenameTab { .. } => "rename_tab",
            Self::UnlockTabTitle { .. } => "unlock_tab_title",
            Self::RenameWorkspace { .. } => "rename_workspace",
            Self::SetWorkspaceCwd { .. } => "set_workspace_cwd",
            Self::BindSession { .. } => "bind_session",
            Self::UnbindSession { .. } => "unbind_session",
            Self::UpdateSurface { .. } => "update_surface",
            Self::BindWorktree { .. } => "bind_worktree",
            Self::SwapPanes { .. } => "swap_panes",
            Self::MovePane { .. } => "move_pane",
            Self::MoveSurface { .. } => "move_surface",
            Self::MovePaneToNewTab { .. } => "move_pane_to_new_tab",
            Self::MoveTab { .. } => "move_tab",
            Self::ReorderTab { .. } => "reorder_tab",
            Self::ReorderWorkspace { .. } => "reorder_workspace",
            Self::SetRatios { .. } => "set_ratios",
            Self::Equalize { .. } => "equalize",
            Self::ApplyPreset { .. } => "apply_preset",
            Self::SetZoom { .. } => "set_zoom",
            Self::SetDetached { .. } => "set_detached",
            Self::RestoreLayout { .. } => "restore_layout",
            Self::LaunchTemplate { .. } => "launch_template",
            Self::UndoClose { .. } => "undo_close",
        }
    }
}

pub(crate) struct Scratch {
    pub(crate) state: State,
    pub(crate) counter: IdSource,
    pub(crate) reserved: BTreeSet<u64>,
    pub(crate) undo: UndoHistory,
    pub(crate) effects: Vec<Effect>,
    pub(crate) focus: Option<PaneId>,
    pub(crate) now_ms: u64,
}

impl Scratch {
    fn focus(&mut self, pane: Option<PaneId>) {
        let Some(pane) = pane else {
            return;
        };
        if let Some((ws, tab)) = self.state.find_pane(pane) {
            self.state.workspaces[ws].tabs[tab].focused = Some(pane);
            self.focus = Some(pane);
        }
    }

    fn focus_first_of_shown(&mut self) {
        let pane = shown_index(&self.state)
            .and_then(|idx| self.state.workspaces[idx].active_tab())
            .and_then(|tab| tab.root.as_ref())
            .and_then(first_docked);
        self.focus(pane);
    }
}

pub(crate) fn run(s: &mut Scratch, op: Op) -> Result<(), OpError> {
    match op {
        Op::Split {
            target,
            direction,
            edge,
            pane,
            focus,
        } => split(s, target, direction, edge, pane, focus),
        Op::AddSurface { pane, surface } => add_surface(s, pane, *surface),
        Op::SelectSurface { surface } => select_surface(s, surface),
        Op::OpenTab {
            workspace,
            tab,
            fill_empty_active,
        } => open_tab(s, workspace, tab, fill_empty_active),
        Op::CreateWorkspace { workspace, select } => create_workspace(s, workspace, select),
        Op::CloseSurface { surface } => close_surface(s, surface),
        Op::ClosePane { pane, keep_undo } => close_pane(s, pane, keep_undo),
        Op::CloseTab { tab, keep_sessions } => close_tab(s, tab, keep_sessions),
        Op::CloseWorkspace { workspace, focus } => close_workspace(s, workspace, focus),
        Op::FocusPane { pane } => focus_pane(s, pane),
        Op::SelectTab { tab } => select_tab(s, tab),
        Op::SelectWorkspace { workspace, focus } => select_workspace(s, workspace, focus),
        Op::RenameSurface { surface, name } => {
            surface_mut(s, surface)?.definition.custom_name = name;
            Ok(())
        }
        Op::RenameTab { tab, title, source } => rename_tab(s, tab, &title, source),
        Op::UnlockTabTitle { tab } => {
            tab_mut(&mut s.state, tab)?.title_source = TabTitleSource::Preset;
            Ok(())
        }
        Op::RenameWorkspace { workspace, title } => {
            workspace_mut(&mut s.state, workspace)?.title = title;
            Ok(())
        }
        Op::SetWorkspaceCwd { workspace, cwd } => {
            workspace_mut(&mut s.state, workspace)?.cwd = cwd;
            Ok(())
        }
        Op::BindSession { surface, session } => bind_session(s, surface, session),
        Op::UnbindSession { surface } => {
            surface_mut(s, surface)?.definition.session = None;
            Ok(())
        }
        Op::UpdateSurface {
            surface,
            definition,
        } => update_surface(s, surface, *definition),
        Op::BindWorktree { tab, worktree } => {
            tab_mut(&mut s.state, tab)?.worktree = worktree;
            Ok(())
        }
        Op::SwapPanes { a, b } => swap_panes(s, a, b),
        Op::MovePane {
            pane,
            target,
            direction,
            edge,
        } => move_pane(s, pane, target, direction, edge),
        Op::MoveSurface {
            surface,
            target,
            split,
        } => move_surface(s, surface, target, split),
        Op::MovePaneToNewTab {
            pane,
            workspace,
            tab,
            index,
        } => move_pane_to_new_tab(s, pane, workspace, tab, index),
        Op::MoveTab {
            tab,
            workspace,
            index,
        } => move_tab(s, tab, workspace, index),
        Op::ReorderTab { tab, to } => reorder_tab(s, tab, to),
        Op::ReorderWorkspace { workspace, to } => reorder_workspace(s, workspace, to),
        Op::SetRatios { split, ratios } => set_ratios(s, split, ratios),
        Op::Equalize { tab } => {
            let tab = tab_mut(&mut s.state, tab)?;
            if tab.zoomed.is_none()
                && let Some(root) = &mut tab.root
            {
                root.equalize();
            }
            Ok(())
        }
        Op::ApplyPreset { tab, preset } => apply_preset(s, tab, preset),
        Op::SetZoom { tab, pane } => set_zoom(s, tab, pane),
        Op::SetDetached { pane, detached } => set_detached(s, pane, detached),
        Op::RestoreLayout { tab, layout } => restore_layout(s, tab, layout),
        Op::LaunchTemplate {
            workspace,
            root,
            focus,
            cwd,
        } => launch_template(s, workspace, root, focus, cwd),
        Op::UndoClose { tab } => undo_close(s, tab),
    }
}

fn workspace_mut(state: &mut State, id: WorkspaceId) -> Result<&mut Workspace, OpError> {
    state
        .workspaces
        .iter_mut()
        .find(|ws| ws.id == id)
        .ok_or(OpError::NotFound)
}

fn tab_mut(state: &mut State, id: TabId) -> Result<&mut Tab, OpError> {
    let (ws, tab) = state.find_tab(id).ok_or(OpError::NotFound)?;
    state.workspaces[ws]
        .tabs
        .get_mut(tab)
        .ok_or(OpError::NotFound)
}

fn tab_of_pane(state: &mut State, pane: PaneId) -> Result<&mut Tab, OpError> {
    let (ws, tab) = state.find_pane(pane).ok_or(OpError::NotFound)?;
    state.workspaces[ws]
        .tabs
        .get_mut(tab)
        .ok_or(OpError::NotFound)
}

fn surface_mut(s: &mut Scratch, id: SurfaceId) -> Result<&mut Surface, OpError> {
    let location = s.state.find_surface(id).ok_or(OpError::NotFound)?;
    s.state.workspaces[location.workspace_index].tabs[location.tab_index]
        .root
        .as_mut()
        .and_then(|root| root.pane_mut(location.pane))
        .and_then(|pane| pane.surfaces.get_mut(location.index))
        .ok_or(OpError::NotFound)
}

fn consume(s: &mut Scratch, id: u64) -> Result<(), OpError> {
    if s.reserved.remove(&id) {
        Ok(())
    } else {
        Err(OpError::Invalid)
    }
}

fn next_split(counter: &IdSource) -> impl FnMut() -> SplitId + '_ {
    move || SplitId(counter.allocate())
}

fn build_pane(s: &mut Scratch, new: NewPane) -> Result<Pane, OpError> {
    if new.surfaces.is_empty() || new.surfaces.len() > MAX_PANE_SURFACES {
        return Err(OpError::Invalid);
    }
    consume(s, new.id.0)?;
    let mut surfaces = Vec::with_capacity(new.surfaces.len());
    for surface in new.surfaces {
        surfaces.push(build_surface(s, surface)?);
    }
    let active = new.active.min(surfaces.len() - 1);
    Ok(Pane {
        id: new.id,
        surfaces,
        active,
        detached: false,
    })
}

fn build_surface(s: &mut Scratch, new: NewSurface) -> Result<Surface, OpError> {
    consume(s, new.id.0)?;
    let mut definition = new.definition;
    definition.focus = None;
    let surface = Surface {
        id: new.id,
        definition,
    };
    if surface.kind() == SurfaceKind::Terminal {
        s.effects.push(Effect::Spawn {
            surface: surface.id,
        });
    }
    Ok(surface)
}

fn build_node(s: &mut Scratch, new: NewNode) -> Result<Node, OpError> {
    match new {
        NewNode::Pane(pane) => build_pane(s, pane).map(Node::Pane),
        NewNode::Split {
            direction,
            ratios,
            children,
        } => {
            if children.len() < 2 || ratios.len() != children.len() {
                return Err(OpError::Invalid);
            }
            let mut built = Vec::with_capacity(children.len());
            for (child, ratio) in children.into_iter().zip(ratios) {
                built.push(Child {
                    node: build_node(s, child)?,
                    ratio,
                });
            }
            Ok(Node::Split(Split {
                id: SplitId(s.counter.allocate()),
                direction,
                children: built,
            }))
        }
    }
}

fn new_node_leaves(node: &NewNode) -> usize {
    match node {
        NewNode::Pane(_) => 1,
        NewNode::Split { children, .. } => children.iter().map(new_node_leaves).sum(),
    }
}

fn first_docked(root: &Node) -> Option<PaneId> {
    root.panes()
        .into_iter()
        .find(|pane| !pane.detached)
        .map(|pane| pane.id)
}

fn shown_index(state: &State) -> Option<usize> {
    state
        .workspaces
        .get(state.active_workspace)
        .filter(|ws| ws.space == state.active_space)
        .map(|_| state.active_workspace)
}

fn point_at(state: &mut State, idx: usize) {
    if let Some(ws) = state.workspaces.get(idx) {
        state.active_space = ws.space.clone();
        state.active_workspace = idx;
    }
}

fn surface_sessions(pane: &Pane) -> Vec<SessionId> {
    pane.surfaces
        .iter()
        .filter_map(|surface| surface.session().cloned())
        .collect()
}

fn stop_all(s: &mut Scratch, sessions: Vec<SessionId>) {
    for session in sessions {
        s.undo.release(&session);
        s.effects.push(Effect::Stop { session });
    }
}

fn fresh_pane_in_tab(s: &mut Scratch, cwd: Option<String>) -> Pane {
    let pane = PaneId(s.counter.allocate());
    let surface = SurfaceId(s.counter.allocate());
    s.effects.push(Effect::Spawn { surface });
    Pane {
        id: pane,
        surfaces: vec![Surface {
            id: surface,
            definition: SurfaceDefinition {
                cwd,
                ..SurfaceDefinition::default()
            },
        }],
        active: 0,
        detached: false,
    }
}

fn split(
    s: &mut Scratch,
    target: PaneId,
    direction: Direction,
    edge: Edge,
    new: NewPane,
    focus: bool,
) -> Result<(), OpError> {
    let tab = tab_of_pane(&mut s.state, target)?;
    if tab.zoomed.is_some() {
        return Err(OpError::Zoomed);
    }
    if tab.pane_count() >= MAX_PANES {
        return Err(OpError::PaneLimit);
    }
    let pane = build_pane(s, new)?;
    let new_id = pane.id;
    let Scratch { state, counter, .. } = s;
    let tab = tab_of_pane(state, target)?;
    let root = tab.root.as_mut().ok_or(OpError::NotFound)?;
    root.split_at_pane(target, direction, pane, &mut next_split(counter))
        .map_err(|_| OpError::NotFound)?;
    if edge == Edge::Before {
        root.swap_panes(target, new_id);
    }
    if focus {
        s.focus(Some(new_id));
    }
    Ok(())
}

fn add_surface(s: &mut Scratch, pane: PaneId, new: NewSurface) -> Result<(), OpError> {
    let count = s.state.pane(pane).ok_or(OpError::NotFound)?.surfaces.len();
    if count >= crate::model::MAX_PANE_SURFACES.min(PANE_TAB_CAP) {
        return Err(OpError::SurfaceLimit);
    }
    let surface = build_surface(s, new)?;
    let tab = tab_of_pane(&mut s.state, pane)?;
    let target = tab
        .root
        .as_mut()
        .and_then(|root| root.pane_mut(pane))
        .ok_or(OpError::NotFound)?;
    target.surfaces.push(surface);
    target.active = target.surfaces.len() - 1;
    s.focus(Some(pane));
    Ok(())
}

pub const PANE_TAB_CAP: usize = 8;

fn select_surface(s: &mut Scratch, surface: SurfaceId) -> Result<(), OpError> {
    let location = s.state.find_surface(surface).ok_or(OpError::NotFound)?;
    let tab = &mut s.state.workspaces[location.workspace_index].tabs[location.tab_index];
    let pane = tab
        .root
        .as_mut()
        .and_then(|root| root.pane_mut(location.pane))
        .ok_or(OpError::NotFound)?;
    pane.active = location.index;
    Ok(())
}

fn build_tab(s: &mut Scratch, new: NewTab) -> Result<Tab, OpError> {
    consume(s, new.id.0)?;
    let root = match new.root {
        Some(root) => {
            if new_node_leaves(&root) > MAX_PANES {
                return Err(OpError::PaneLimit);
            }
            Some(build_node(s, root)?)
        }
        None => None,
    };
    let focused = root.as_ref().and_then(first_docked);
    Ok(Tab {
        id: new.id,
        title: new.title,
        title_source: new.title_source,
        root,
        focused,
        zoomed: None,
        worktree: new.worktree,
        extras: TabExtras::default(),
    })
}

fn open_tab(
    s: &mut Scratch,
    workspace: WorkspaceId,
    new: NewTab,
    fill_empty_active: bool,
) -> Result<(), OpError> {
    let tab = build_tab(s, new)?;
    let ws = workspace_mut(&mut s.state, workspace)?;
    if fill_empty_active
        && let Some(active) = ws.tabs.get_mut(ws.active_tab)
        && active.root.is_none()
    {
        if !tab.title.is_empty() && active.title_source.yields_to(tab.title_source) {
            active.title = tab.title;
            active.title_source = tab.title_source;
        }
        active.focused = tab.focused;
        active.root = tab.root;
        return Ok(());
    }
    place_tab(ws, tab)
}

fn place_tab(ws: &mut Workspace, tab: Tab) -> Result<(), OpError> {
    if ws.is_empty_shell() {
        ws.tabs[0] = tab;
        ws.active_tab = 0;
        return Ok(());
    }
    if ws.tabs.len() >= MAX_TABS_PER_WORKSPACE {
        return Err(OpError::TabLimit);
    }
    ws.tabs.push(tab);
    ws.active_tab = ws.tabs.len() - 1;
    Ok(())
}

fn create_workspace(s: &mut Scratch, new: NewWorkspace, select: bool) -> Result<(), OpError> {
    consume(s, new.id.0)?;
    let mut tabs = Vec::new();
    for tab in new.tabs.into_iter().take(MAX_TABS_PER_WORKSPACE) {
        tabs.push(build_tab(s, tab)?);
    }
    if tabs.is_empty() {
        tabs.push(empty_tab(TabId(s.counter.allocate())));
    }
    let active_tab = new.active_tab.min(tabs.len() - 1);
    s.state.workspaces.push(Workspace {
        id: new.id,
        title: new.title,
        cwd: new.cwd,
        space: new.space,
        tabs,
        active_tab,
        extras: WorkspaceExtras {
            durable_id: new.durable_id,
            custom_buttons: new.custom_buttons,
            managed_worktrees: new.managed_worktrees,
            ..WorkspaceExtras::default()
        },
    });
    if select {
        let idx = s.state.workspaces.len() - 1;
        point_at(&mut s.state, idx);
    }
    Ok(())
}

fn close_surface(s: &mut Scratch, surface: SurfaceId) -> Result<(), OpError> {
    let location = s.state.find_surface(surface).ok_or(OpError::NotFound)?;
    let count = s
        .state
        .pane(location.pane)
        .map_or(0, |pane| pane.surfaces.len());
    if count <= 1 {
        return close_pane(s, location.pane, false);
    }
    let tab = &mut s.state.workspaces[location.workspace_index].tabs[location.tab_index];
    let pane = tab
        .root
        .as_mut()
        .and_then(|root| root.pane_mut(location.pane))
        .ok_or(OpError::NotFound)?;
    let removed = pane.surfaces.remove(location.index);
    if pane.active > location.index || pane.active >= pane.surfaces.len() {
        pane.active = pane.active.saturating_sub(1);
    }
    if let Some(session) = removed.session().cloned() {
        stop_all(s, vec![session]);
    }
    Ok(())
}

fn close_pane(s: &mut Scratch, pane: PaneId, keep_undo: bool) -> Result<(), OpError> {
    let (ws_idx, tab_idx) = s.state.find_pane(pane).ok_or(OpError::NotFound)?;
    let ws_id = s.state.workspaces[ws_idx].id;
    let ws_cwd = s.state.workspaces[ws_idx].cwd.clone();
    let tab = &mut s.state.workspaces[ws_idx].tabs[tab_idx];
    let tab_id = tab.id;
    let worktree = tab.worktree.clone();
    let root = tab.root.take().ok_or(OpError::NotFound)?;
    let (rest, removed, focus) = if keep_undo && tab.zoomed.is_none() {
        let closed = root.close_pane(pane);
        (closed.node, closed.pane, closed.focus)
    } else {
        let (rest, removed) = root.remove_pane(pane);
        let focus = if keep_undo {
            rest.as_ref().and_then(first_docked)
        } else {
            None
        };
        (rest, removed, focus)
    };
    let removed = removed.ok_or(OpError::NotFound)?;
    if !keep_undo {
        let sessions = surface_sessions(&removed);
        stop_all(s, sessions);
    }
    let tab = &mut s.state.workspaces[ws_idx].tabs[tab_idx];
    if tab.zoomed == Some(pane) || (keep_undo && tab.zoomed.is_some()) {
        tab.zoomed = None;
    }
    let was_focused = tab.focused == Some(pane);
    let fallback = rest.as_ref().and_then(first_docked);
    tab.root = rest;
    if was_focused {
        tab.focused = None;
    }
    if tab.root.is_some() {
        if keep_undo {
            s.focus(focus.or(fallback));
        } else if was_focused {
            s.focus(fallback);
        }
    } else {
        let cwd = if keep_undo { None } else { Some(ws_cwd) };
        let fresh = fresh_pane_in_tab(s, cwd);
        let fresh_id = fresh.id;
        s.state.workspaces[ws_idx].tabs[tab_idx].root = Some(Node::Pane(fresh));
        if keep_undo {
            s.focus_first_of_shown();
        } else {
            s.state.workspaces[ws_idx].tabs[tab_idx].focused = Some(fresh_id);
        }
    }
    if keep_undo {
        let mut kept = removed.clone();
        let active = kept.active.min(kept.surfaces.len().saturating_sub(1));
        let others: Vec<SessionId> = kept
            .surfaces
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != active)
            .filter_map(|(_, surface)| surface.session().cloned())
            .collect();
        kept.surfaces = kept.surfaces.drain(active..=active).collect();
        kept.active = 0;
        let held: Vec<SessionId> = surface_sessions(&kept);
        record_close(
            s,
            ClosedRecord::Pane(ClosedPane {
                workspace: ws_id,
                tab: tab_id,
                worktree,
                pane: kept,
            }),
            held,
            others,
        );
    }
    Ok(())
}

fn record_close(
    s: &mut Scratch,
    record: ClosedRecord,
    held: Vec<SessionId>,
    others: Vec<SessionId>,
) {
    let superseded: HashSet<SessionId> = held.iter().cloned().collect();
    s.undo.superseded_by(&superseded);
    let evicted = s.undo.push(record);
    for session in evicted {
        s.effects.push(Effect::Stop { session });
    }
    stop_all(s, others);
    let until_ms = s.now_ms.saturating_add(CLOSED_SESSION_GRACE_MS);
    for session in held {
        s.undo.hold(session.clone(), until_ms);
        s.effects.push(Effect::Hold { session, until_ms });
    }
}

fn close_tab(s: &mut Scratch, tab: TabId, keep_sessions: bool) -> Result<(), OpError> {
    let (ws_idx, tab_idx) = s.state.find_tab(tab).ok_or(OpError::NotFound)?;
    let ws = &mut s.state.workspaces[ws_idx];
    let ws_id = ws.id;
    let removed = ws.tabs.remove(tab_idx);
    if ws.tabs.is_empty() {
        ws.tabs.push(empty_tab(TabId(s.counter.allocate())));
        ws.active_tab = 0;
    } else if ws.active_tab >= ws.tabs.len() {
        ws.active_tab = ws.tabs.len() - 1;
    } else if ws.active_tab > tab_idx {
        ws.active_tab -= 1;
    }
    if shown_index(&s.state) == Some(ws_idx) {
        s.focus_first_of_shown();
    }
    let Some(root) = removed.root.as_ref() else {
        return Ok(());
    };
    let layout = export_node(root);
    let mut held = Vec::new();
    let mut others = Vec::new();
    let mut every = Vec::new();
    let mut panes: Vec<Pane> = root
        .panes()
        .into_iter()
        .map(|pane| {
            let active = pane.active.min(pane.surfaces.len().saturating_sub(1));
            for (index, surface) in pane.surfaces.iter().enumerate() {
                if let Some(session) = surface.session().cloned() {
                    every.push(session.clone());
                    if index == active {
                        held.push(session);
                    } else {
                        others.push(session);
                    }
                }
            }
            Pane {
                id: pane.id,
                surfaces: pane.surfaces[active..=active].to_vec(),
                active: 0,
                detached: false,
            }
        })
        .collect();
    if !keep_sessions {
        for pane in &mut panes {
            for surface in &mut pane.surfaces {
                surface.definition.session = None;
            }
        }
    }
    let record = ClosedRecord::Tab(ClosedTab {
        workspace: ws_id,
        tab: removed.id,
        index: tab_idx,
        title: removed.title.clone(),
        title_source: removed.title_source,
        worktree: removed.worktree.clone(),
        layout,
        panes,
    });
    if keep_sessions {
        record_close(s, record, held, others);
    } else {
        stop_all(s, every);
        for session in s.undo.push(record) {
            s.effects.push(Effect::Stop { session });
        }
    }
    Ok(())
}

fn close_workspace(s: &mut Scratch, workspace: WorkspaceId, focus: bool) -> Result<(), OpError> {
    let idx = s
        .state
        .workspace_index(workspace)
        .ok_or(OpError::NotFound)?;
    let removed = s.state.workspaces.remove(idx);
    let active_space = s.state.active_space.clone();
    s.state.active_workspace = active_after_removal(
        &s.state.workspaces,
        s.state.active_workspace,
        idx,
        active_space.as_ref(),
    );
    if focus {
        s.focus_first_of_shown();
    }
    let sessions: Vec<SessionId> = removed
        .tabs
        .iter()
        .filter_map(|tab| tab.root.as_ref())
        .flat_map(|root| root.panes().into_iter().flat_map(surface_sessions))
        .collect();
    stop_all(s, sessions);
    Ok(())
}

fn active_after_removal(
    remaining: &[Workspace],
    active: usize,
    removed: usize,
    active_space: Option<&SpaceId>,
) -> usize {
    if remaining.is_empty() {
        return 0;
    }
    if active != removed {
        return if active > removed { active - 1 } else { active };
    }
    let split = removed.min(remaining.len());
    let in_space = |ws: &Workspace| ws.space.as_ref() == active_space;
    remaining[split..]
        .iter()
        .position(in_space)
        .map(|offset| split + offset)
        .or_else(|| remaining[..split].iter().rposition(in_space))
        .unwrap_or(split.min(remaining.len() - 1))
}

fn focus_pane(s: &mut Scratch, pane: PaneId) -> Result<(), OpError> {
    let tab = tab_of_pane(&mut s.state, pane)?;
    tab.focused = Some(pane);
    Ok(())
}

fn select_tab(s: &mut Scratch, tab: TabId) -> Result<(), OpError> {
    let (ws_idx, tab_idx) = s.state.find_tab(tab).ok_or(OpError::NotFound)?;
    s.state.workspaces[ws_idx].active_tab = tab_idx;
    if shown_index(&s.state) != Some(ws_idx) {
        point_at(&mut s.state, ws_idx);
    }
    s.focus_first_of_shown();
    Ok(())
}

fn select_workspace(s: &mut Scratch, workspace: WorkspaceId, focus: bool) -> Result<(), OpError> {
    let idx = s
        .state
        .workspace_index(workspace)
        .ok_or(OpError::NotFound)?;
    point_at(&mut s.state, idx);
    if focus {
        s.focus_first_of_shown();
    }
    Ok(())
}

fn rename_tab(
    s: &mut Scratch,
    tab: TabId,
    title: &str,
    source: TabTitleSource,
) -> Result<(), OpError> {
    let tab = tab_mut(&mut s.state, tab)?;
    let title = title.trim();
    if title.is_empty() || !tab.title_source.yields_to(source) {
        return Err(OpError::Invalid);
    }
    tab.title.clear();
    tab.title.push_str(title);
    tab.title_source = source;
    Ok(())
}

fn bind_session(s: &mut Scratch, surface: SurfaceId, session: SessionId) -> Result<(), OpError> {
    let target = surface_mut(s, surface)?;
    if target.kind() != SurfaceKind::Terminal || target.definition.session.is_some() {
        return Err(OpError::Invalid);
    }
    target.definition.session = Some(session);
    Ok(())
}

fn update_surface(
    s: &mut Scratch,
    surface: SurfaceId,
    mut definition: SurfaceDefinition,
) -> Result<(), OpError> {
    let target = surface_mut(s, surface)?;
    definition.focus = None;
    definition.session = target.definition.session.clone();
    definition.surface_type = target.definition.surface_type.clone();
    target.definition = definition;
    Ok(())
}

fn swap_panes(s: &mut Scratch, a: PaneId, b: PaneId) -> Result<(), OpError> {
    let (ws_a, tab_a) = s.state.find_pane(a).ok_or(OpError::NotFound)?;
    let (ws_b, tab_b) = s.state.find_pane(b).ok_or(OpError::NotFound)?;
    if (ws_a, tab_a) != (ws_b, tab_b) || a == b {
        return Err(OpError::Invalid);
    }
    let tab = &mut s.state.workspaces[ws_a].tabs[tab_a];
    if tab.zoomed.is_some() {
        return Err(OpError::Zoomed);
    }
    let root = tab.root.as_mut().ok_or(OpError::NotFound)?;
    if !root.swap_panes(a, b) {
        return Err(OpError::NotFound);
    }
    s.focus(Some(a));
    Ok(())
}

fn move_pane(
    s: &mut Scratch,
    pane: PaneId,
    target: PaneId,
    direction: Direction,
    edge: Edge,
) -> Result<(), OpError> {
    let (ws_a, tab_a) = s.state.find_pane(pane).ok_or(OpError::NotFound)?;
    let (ws_b, tab_b) = s.state.find_pane(target).ok_or(OpError::NotFound)?;
    if (ws_a, tab_a) != (ws_b, tab_b) || pane == target {
        return Err(OpError::Invalid);
    }
    let Scratch { state, counter, .. } = s;
    let tab = &mut state.workspaces[ws_a].tabs[tab_a];
    if tab.zoomed.is_some() {
        return Err(OpError::Zoomed);
    }
    let root = tab.root.take().ok_or(OpError::NotFound)?;
    let (rest, moved) = root.remove_pane(pane);
    let moved = moved.ok_or(OpError::NotFound)?;
    let mut rest = rest.ok_or(OpError::Invalid)?;
    let mut alloc = next_split(counter);
    if let Err(moved) = rest.split_at_pane(target, direction, moved, &mut alloc) {
        rest.split_first_leaf(Direction::Vertical, moved, &mut alloc);
    } else if edge == Edge::Before {
        rest.swap_panes(target, pane);
    }
    tab.root = Some(rest);
    drop(alloc);
    s.focus(Some(pane));
    Ok(())
}

fn move_surface(
    s: &mut Scratch,
    surface: SurfaceId,
    target: PaneId,
    split: Option<(Direction, Edge, PaneId)>,
) -> Result<(), OpError> {
    let location = s.state.find_surface(surface).ok_or(OpError::NotFound)?;
    let (ws_t, tab_t) = s.state.find_pane(target).ok_or(OpError::NotFound)?;
    if (location.workspace_index, location.tab_index) != (ws_t, tab_t) {
        return Err(OpError::Invalid);
    }
    let tab = &s.state.workspaces[ws_t].tabs[tab_t];
    if tab.zoomed.is_some() {
        return Err(OpError::Zoomed);
    }
    let source_count = s
        .state
        .pane(location.pane)
        .map_or(0, |pane| pane.surfaces.len());
    let target_pane = s.state.pane(target).ok_or(OpError::NotFound)?;
    if target_pane.detached || s.state.pane(location.pane).is_some_and(|p| p.detached) {
        return Err(OpError::Invalid);
    }
    match split {
        None => {
            if location.pane == target {
                return Err(OpError::Invalid);
            }
            if target_pane.surfaces.len() >= PANE_TAB_CAP {
                return Err(OpError::SurfaceLimit);
            }
        }
        Some(_) => {
            if location.pane == target && source_count <= 1 {
                return Err(OpError::Invalid);
            }
            if tab.pane_count() >= MAX_PANES && source_count > 1 {
                return Err(OpError::PaneLimit);
            }
        }
    }
    let moved = take_surface(
        &mut s.state,
        location.workspace_index,
        location.tab_index,
        location.pane,
        location.index,
    )?;
    match split {
        None => {
            let tab = &mut s.state.workspaces[ws_t].tabs[tab_t];
            let pane = tab
                .root
                .as_mut()
                .and_then(|root| root.pane_mut(target))
                .ok_or(OpError::NotFound)?;
            pane.surfaces.push(moved);
            pane.active = pane.surfaces.len() - 1;
            s.focus(Some(target));
        }
        Some((direction, edge, new_pane)) => {
            consume(s, new_pane.0)?;
            let pane = Pane {
                id: new_pane,
                surfaces: vec![moved],
                active: 0,
                detached: false,
            };
            let Scratch { state, counter, .. } = s;
            let tab = &mut state.workspaces[ws_t].tabs[tab_t];
            let root = tab.root.as_mut().ok_or(OpError::NotFound)?;
            let mut alloc = next_split(counter);
            match root.split_at_pane(target, direction, pane, &mut alloc) {
                Ok(()) => {
                    if edge == Edge::Before {
                        root.swap_panes(target, new_pane);
                    }
                }
                Err(_) => return Err(OpError::NotFound),
            }
            drop(alloc);
            s.focus(Some(new_pane));
        }
    }
    Ok(())
}

fn take_surface(
    state: &mut State,
    ws_idx: usize,
    tab_idx: usize,
    pane: PaneId,
    index: usize,
) -> Result<Surface, OpError> {
    let tab = &mut state.workspaces[ws_idx].tabs[tab_idx];
    let source = tab
        .root
        .as_mut()
        .and_then(|root| root.pane_mut(pane))
        .ok_or(OpError::NotFound)?;
    if source.surfaces.len() > 1 {
        let moved = source.surfaces.remove(index);
        if source.active > index || source.active >= source.surfaces.len() {
            source.active = source.active.saturating_sub(1);
        }
        return Ok(moved);
    }
    let root = tab.root.take().ok_or(OpError::NotFound)?;
    let (rest, removed) = root.remove_pane(pane);
    tab.root = rest;
    if tab.focused == Some(pane) {
        tab.focused = None;
    }
    let mut removed = removed.ok_or(OpError::NotFound)?;
    if removed.surfaces.is_empty() {
        return Err(OpError::NotFound);
    }
    Ok(removed.surfaces.remove(0))
}

fn rehome(s: &mut Scratch, pane: &Pane, workspace: WorkspaceId) {
    for session in surface_sessions(pane) {
        s.effects.push(Effect::Rehome { session, workspace });
    }
}

fn move_pane_to_new_tab(
    s: &mut Scratch,
    pane: PaneId,
    workspace: WorkspaceId,
    tab_id: TabId,
    index: Option<usize>,
) -> Result<(), OpError> {
    let mut index = index;
    let (src_ws, src_tab) = s.state.find_pane(pane).ok_or(OpError::NotFound)?;
    let dest_ws = s
        .state
        .workspace_index(workspace)
        .ok_or(OpError::NotFound)?;
    let source_panes = s.state.workspaces[src_ws].tabs[src_tab].pane_count();
    if source_panes <= 1 && src_ws == dest_ws {
        return Err(OpError::Invalid);
    }
    if !s.state.workspaces[dest_ws].is_empty_shell()
        && s.state.workspaces[dest_ws].tabs.len() >= MAX_TABS_PER_WORKSPACE
    {
        return Err(OpError::TabLimit);
    }
    consume(s, tab_id.0)?;
    let source = &mut s.state.workspaces[src_ws].tabs[src_tab];
    source.zoomed = None;
    let root = source.root.take().ok_or(OpError::NotFound)?;
    let (rest, moved) = root.remove_pane(pane);
    let mut moved = moved.ok_or(OpError::NotFound)?;
    moved.detached = false;
    let mut dest_ws = dest_ws;
    match rest {
        Some(rest) => {
            source.focused = first_docked(&rest);
            source.root = Some(rest);
        }
        None => {
            let ws = &mut s.state.workspaces[src_ws];
            ws.tabs.remove(src_tab);
            if ws.tabs.is_empty() {
                ws.tabs.push(empty_tab(TabId(s.counter.allocate())));
                ws.active_tab = 0;
            } else if ws.active_tab >= ws.tabs.len() {
                ws.active_tab = ws.tabs.len() - 1;
            } else if ws.active_tab > src_tab {
                ws.active_tab -= 1;
            }
            if src_ws == dest_ws {
                dest_ws = s
                    .state
                    .workspace_index(workspace)
                    .ok_or(OpError::NotFound)?;
                index = index.map(|index| if src_tab < index { index - 1 } else { index });
            }
        }
    }
    let target = s.state.workspaces[dest_ws].id;
    rehome(s, &moved, target);
    let tab = Tab {
        id: tab_id,
        title: String::new(),
        title_source: TabTitleSource::Preset,
        focused: Some(moved.id),
        root: Some(Node::Pane(moved)),
        zoomed: None,
        worktree: None,
        extras: TabExtras::default(),
    };
    place_tab(&mut s.state.workspaces[dest_ws], tab)?;
    if let Some(index) = index {
        let dest = &mut s.state.workspaces[dest_ws];
        let last = dest.tabs.len().saturating_sub(1);
        reorder_in(dest, last, index.min(last));
    }
    if shown_index(&s.state) != Some(dest_ws) {
        point_at(&mut s.state, dest_ws);
    }
    s.focus_first_of_shown();
    Ok(())
}

fn move_tab(
    s: &mut Scratch,
    tab: TabId,
    workspace: WorkspaceId,
    index: Option<usize>,
) -> Result<(), OpError> {
    let (src_ws, src_tab) = s.state.find_tab(tab).ok_or(OpError::NotFound)?;
    let dest_ws = s
        .state
        .workspace_index(workspace)
        .ok_or(OpError::NotFound)?;
    if src_ws == dest_ws {
        return Err(OpError::Invalid);
    }
    let dest = &s.state.workspaces[dest_ws];
    if !dest.is_empty_shell() && dest.tabs.len() >= MAX_TABS_PER_WORKSPACE {
        return Err(OpError::TabLimit);
    }
    let ws = &mut s.state.workspaces[src_ws];
    let moved = ws.tabs.remove(src_tab);
    if ws.tabs.is_empty() {
        ws.tabs.push(empty_tab(TabId(s.counter.allocate())));
        ws.active_tab = 0;
    } else if ws.active_tab >= ws.tabs.len() {
        ws.active_tab = ws.tabs.len() - 1;
    } else if ws.active_tab > src_tab {
        ws.active_tab -= 1;
    }
    let target = s.state.workspaces[dest_ws].id;
    if let Some(root) = &moved.root {
        let panes: Vec<Pane> = root.panes().into_iter().cloned().collect();
        for pane in &panes {
            rehome(s, pane, target);
        }
    }
    let dest = &mut s.state.workspaces[dest_ws];
    place_tab(dest, moved)?;
    if let Some(index) = index {
        let last = dest.tabs.len() - 1;
        reorder_in(dest, last, index.min(last));
    }
    point_at(&mut s.state, dest_ws);
    s.focus_first_of_shown();
    Ok(())
}

fn reorder_in(ws: &mut Workspace, from: usize, to: usize) -> bool {
    if from >= ws.tabs.len() || to > ws.tabs.len() || from == to {
        return false;
    }
    let active_id = ws.tabs[ws.active_tab.min(ws.tabs.len() - 1)].id;
    let tab = ws.tabs.remove(from);
    let at = to.min(ws.tabs.len());
    ws.tabs.insert(at, tab);
    if let Some(idx) = ws.tabs.iter().position(|tab| tab.id == active_id) {
        ws.active_tab = idx;
    }
    true
}

fn reorder_tab(s: &mut Scratch, tab: TabId, to: usize) -> Result<(), OpError> {
    let (ws, from) = s.state.find_tab(tab).ok_or(OpError::NotFound)?;
    reorder_in(&mut s.state.workspaces[ws], from, to);
    Ok(())
}

fn reorder_workspace(s: &mut Scratch, workspace: WorkspaceId, to: usize) -> Result<(), OpError> {
    let from = s
        .state
        .workspace_index(workspace)
        .ok_or(OpError::NotFound)?;
    let workspaces = &mut s.state.workspaces;
    let space = workspaces[from].space.clone();
    let mut naive: Vec<usize> = (0..workspaces.len()).collect();
    let moved = naive.remove(from);
    naive.insert(to.min(naive.len()), moved);
    let mut members = naive
        .into_iter()
        .filter(|&idx| workspaces[idx].space == space);
    let order: Vec<usize> = (0..workspaces.len())
        .map(|idx| {
            if workspaces[idx].space == space {
                members.next().unwrap_or(idx)
            } else {
                idx
            }
        })
        .collect();
    if order.iter().enumerate().all(|(slot, &idx)| slot == idx) {
        return Ok(());
    }
    let shown = workspaces.get(s.state.active_workspace).map(|ws| ws.id);
    let mut slots: Vec<Option<Workspace>> =
        std::mem::take(workspaces).into_iter().map(Some).collect();
    *workspaces = order
        .into_iter()
        .filter_map(|idx| slots[idx].take())
        .collect();
    if let Some(id) = shown {
        s.state.active_workspace = s.state.workspace_index(id).unwrap_or(0);
    }
    Ok(())
}

fn set_ratios(s: &mut Scratch, split: SplitId, ratios: Vec<f32>) -> Result<(), OpError> {
    let target = s
        .state
        .workspaces
        .iter_mut()
        .flat_map(|ws| ws.tabs.iter_mut())
        .filter_map(|tab| tab.root.as_mut())
        .find_map(|root| root.split_mut(split))
        .ok_or(OpError::NotFound)?;
    if ratios.len() != target.children.len() {
        return Err(OpError::Invalid);
    }
    for (child, ratio) in target
        .children
        .iter_mut()
        .zip(tree::floored_ratios(&ratios))
    {
        child.ratio = ratio;
    }
    Ok(())
}

fn apply_preset(s: &mut Scratch, tab: TabId, preset: Preset) -> Result<(), OpError> {
    let Scratch { state, counter, .. } = s;
    let tab = tab_mut(state, tab)?;
    let focused = tab.focused;
    tab.zoomed = None;
    let Some(root) = tab.root.take() else {
        return Ok(());
    };
    if root.leaf_count() <= 1 {
        tab.root = Some(root);
        return Ok(());
    }
    let mut panes = root.into_panes();
    let mut alloc = next_split(counter);
    let rebuilt = match preset {
        Preset::EvenHorizontal => tree::equal_split(alloc(), Direction::Vertical, panes),
        Preset::EvenVertical => tree::equal_split(alloc(), Direction::Horizontal, panes),
        Preset::Tiled => tree::tiled(panes, &mut alloc),
        Preset::MainVertical => {
            let main_idx = focused
                .and_then(|focused| panes.iter().position(|pane| pane.id == focused))
                .unwrap_or(0);
            let main = panes.remove(main_idx);
            Some(tree::main_vertical(main, panes, &mut alloc))
        }
    };
    let focus = match preset {
        Preset::MainVertical => rebuilt.as_ref().and_then(Node::first_pane).map(|p| p.id),
        _ => rebuilt.as_ref().and_then(first_docked),
    };
    tab.root = rebuilt;
    drop(alloc);
    s.focus(focus);
    Ok(())
}

fn set_zoom(s: &mut Scratch, tab: TabId, pane: Option<PaneId>) -> Result<(), OpError> {
    let tab = tab_mut(&mut s.state, tab)?;
    match pane {
        None => {
            let zoomed = tab.zoomed.take();
            s.focus(zoomed);
        }
        Some(pane) => {
            let root = tab.root.as_ref().ok_or(OpError::NotFound)?;
            if !root.contains(pane) {
                return Err(OpError::NotFound);
            }
            if root.leaf_count() <= 1 {
                return Err(OpError::Invalid);
            }
            tab.zoomed = Some(pane);
            s.focus(Some(pane));
        }
    }
    Ok(())
}

fn set_detached(s: &mut Scratch, pane: PaneId, detached: bool) -> Result<(), OpError> {
    let tab = tab_of_pane(&mut s.state, pane)?;
    if detached {
        tab.zoomed = None;
    }
    let target = tab
        .root
        .as_mut()
        .and_then(|root| root.pane_mut(pane))
        .ok_or(OpError::NotFound)?;
    target.detached = detached;
    Ok(())
}

fn restore_layout(s: &mut Scratch, tab: TabId, layout: NewNode) -> Result<(), OpError> {
    let needed = new_node_leaves(&layout);
    if needed == 0 {
        return Err(OpError::Invalid);
    }
    if needed > MAX_PANES {
        return Err(OpError::PaneLimit);
    }
    let current = tab_mut(&mut s.state, tab)?;
    current.zoomed = None;
    let focused = current.focused;
    let existing: Vec<Pane> = current
        .root
        .take()
        .map(Node::into_panes)
        .unwrap_or_default();
    let mut reused = existing.into_iter();
    let mut kept: Vec<Pane> = reused.by_ref().take(needed).collect();
    let surplus: Vec<Pane> = reused.collect();
    kept.reverse();
    let root = place_layout(s, layout, &mut kept)?;
    for pane in &surplus {
        stop_all(s, surface_sessions(pane));
    }
    let current = tab_mut(&mut s.state, tab)?;
    current.focused = focused.filter(|focused| root.contains(*focused));
    current.root = Some(root);
    Ok(())
}

fn place_layout(s: &mut Scratch, layout: NewNode, reuse: &mut Vec<Pane>) -> Result<Node, OpError> {
    match layout {
        NewNode::Pane(fresh) => match reuse.pop() {
            Some(pane) => {
                s.reserved.remove(&fresh.id.0);
                for surface in &fresh.surfaces {
                    s.reserved.remove(&surface.id.0);
                }
                Ok(Node::Pane(pane))
            }
            None => build_pane(s, fresh).map(Node::Pane),
        },
        NewNode::Split {
            direction,
            ratios,
            children,
        } => {
            if children.len() < 2 || ratios.len() != children.len() {
                return Err(OpError::Invalid);
            }
            let mut built = Vec::with_capacity(children.len());
            for (child, ratio) in children.into_iter().zip(ratios) {
                built.push(Child {
                    node: place_layout(s, child, reuse)?,
                    ratio,
                });
            }
            Ok(Node::Split(Split {
                id: SplitId(s.counter.allocate()),
                direction,
                children: built,
            }))
        }
    }
}

fn launch_template(
    s: &mut Scratch,
    workspace: WorkspaceId,
    root: NewNode,
    focus: Option<PaneId>,
    cwd: Option<String>,
) -> Result<(), OpError> {
    let idx = s
        .state
        .workspace_index(workspace)
        .ok_or(OpError::NotFound)?;
    let ws = &s.state.workspaces[idx];
    if ws.active_tab().is_some_and(Tab::is_zoomed) {
        return Err(OpError::Zoomed);
    }
    if new_node_leaves(&root) > MAX_PANES {
        return Err(OpError::PaneLimit);
    }
    let root = build_node(s, root)?;
    let ws = &mut s.state.workspaces[idx];
    if let Some(cwd) = cwd {
        ws.cwd = cwd;
    }
    let active = ws.active_tab.min(ws.tabs.len() - 1);
    let tab = &mut ws.tabs[active];
    let focus = focus
        .filter(|focus| root.contains(*focus))
        .or_else(|| first_docked(&root));
    tab.focused = None;
    tab.root = Some(root);
    tab.zoomed = None;
    if shown_index(&s.state) != Some(idx) {
        point_at(&mut s.state, idx);
    }
    s.focus(focus);
    Ok(())
}

fn undo_close(s: &mut Scratch, tab: Option<TabId>) -> Result<(), OpError> {
    for session in s.undo.expire(s.now_ms) {
        s.effects.push(Effect::Stop { session });
    }
    let record = match tab {
        Some(tab) => s.undo.take_tab(tab),
        None => s.undo.pop(),
    }
    .ok_or(OpError::NotFound)?;
    match record {
        ClosedRecord::Pane(record) => undo_pane(s, record),
        ClosedRecord::Tab(record) => undo_tab(s, record),
    }
}

fn reopen_surfaces(s: &mut Scratch, pane: &mut Pane, release: bool) {
    let attached: HashSet<SessionId> = s
        .state
        .workspaces
        .iter()
        .flat_map(|ws| ws.tabs.iter())
        .filter_map(|tab| tab.root.as_ref())
        .flat_map(|root| root.panes().into_iter().flat_map(surface_sessions))
        .collect();
    for surface in &mut pane.surfaces {
        if surface.kind() != SurfaceKind::Terminal {
            continue;
        }
        let Some(session) = surface.definition.session.clone() else {
            s.effects.push(Effect::Spawn {
                surface: surface.id,
            });
            continue;
        };
        let held = s.undo.release(&session);
        if held && release {
            s.effects.push(Effect::Stop { session });
            surface.definition.session = None;
            s.effects.push(Effect::Spawn {
                surface: surface.id,
            });
        } else if held && !attached.contains(&session) {
            s.effects.push(Effect::Reattach {
                surface: surface.id,
                session,
            });
        } else {
            surface.definition.session = None;
            s.effects.push(Effect::Spawn {
                surface: surface.id,
            });
        }
    }
}

fn undo_pane(s: &mut Scratch, record: ClosedPane) -> Result<(), OpError> {
    let ws_idx = s
        .state
        .workspace_index(record.workspace)
        .or_else(|| shown_index(&s.state))
        .ok_or(OpError::NotFound)?;
    let ws = &s.state.workspaces[ws_idx];
    let tab_idx = ws
        .tab_index(record.tab)
        .unwrap_or_else(|| ws.active_tab.min(ws.tabs.len().saturating_sub(1)));
    let tab = ws.tabs.get(tab_idx).ok_or(OpError::NotFound)?;
    if tab.is_zoomed() {
        return Err(OpError::Zoomed);
    }
    if tab.pane_count() >= MAX_PANES {
        return Err(OpError::PaneLimit);
    }
    let destination_worktree = tab.worktree.clone();
    s.state.workspaces[ws_idx].active_tab = tab_idx;
    point_at(&mut s.state, ws_idx);
    let mut pane = record.pane;
    pane.detached = false;
    let release = record.worktree != destination_worktree;
    reopen_surfaces(s, &mut pane, release);
    let restored = pane.id;
    let Scratch { state, counter, .. } = s;
    let tab = &mut state.workspaces[ws_idx].tabs[tab_idx];
    let mut alloc = next_split(counter);
    match tab.root.as_mut() {
        Some(root) => {
            let anchor = first_docked(root);
            match anchor {
                Some(anchor) => {
                    if let Err(pane) =
                        root.split_at_pane(anchor, Direction::Horizontal, pane, &mut alloc)
                    {
                        root.split_first_leaf(Direction::Horizontal, pane, &mut alloc);
                    }
                }
                None => root.split_first_leaf(Direction::Horizontal, pane, &mut alloc),
            }
        }
        None => tab.root = Some(Node::Pane(pane)),
    }
    drop(alloc);
    s.focus(Some(restored));
    Ok(())
}

fn undo_tab(s: &mut Scratch, record: ClosedTab) -> Result<(), OpError> {
    let ws_idx = s
        .state
        .workspace_index(record.workspace)
        .or_else(|| shown_index(&s.state))
        .ok_or(OpError::NotFound)?;
    let ws = &s.state.workspaces[ws_idx];
    if ws.tabs.len() >= MAX_TABS_PER_WORKSPACE {
        return Err(OpError::TabLimit);
    }
    let mut panes = record.panes;
    for pane in &mut panes {
        pane.detached = false;
        reopen_surfaces(s, pane, false);
    }
    panes.reverse();
    let root = rebuild_from_layout(s, &record.layout, &mut panes);
    let tab = Tab {
        id: record.tab,
        title: record.title,
        title_source: record.title_source,
        focused: None,
        root: Some(root),
        zoomed: None,
        worktree: record.worktree,
        extras: TabExtras::default(),
    };
    let ws = &mut s.state.workspaces[ws_idx];
    place_tab(ws, tab)?;
    let last = ws.tabs.len() - 1;
    reorder_in(ws, last, record.index.min(last));
    if shown_index(&s.state) != Some(ws_idx) {
        point_at(&mut s.state, ws_idx);
    }
    s.focus_first_of_shown();
    Ok(())
}

fn rebuild_from_layout(s: &mut Scratch, layout: &LayoutNode, panes: &mut Vec<Pane>) -> Node {
    match layout {
        LayoutNode::Pane { .. } => match panes.pop() {
            Some(pane) => Node::Pane(pane),
            None => Node::Pane(fresh_pane_in_tab(s, None)),
        },
        LayoutNode::Split {
            direction,
            children,
            ..
        } => {
            let resolved = layout.resolved_ratios();
            let count = children.len().max(1);
            let built: Vec<Child> = children
                .iter()
                .enumerate()
                .map(|(i, child)| Child {
                    ratio: resolved.get(i).copied().unwrap_or(1.0 / count as f64) as f32,
                    node: rebuild_from_layout(s, child, panes),
                })
                .collect();
            Node::Split(Split {
                id: SplitId(s.counter.allocate()),
                direction: Direction::from_layout_str(direction),
                children: built,
            })
        }
    }
}
