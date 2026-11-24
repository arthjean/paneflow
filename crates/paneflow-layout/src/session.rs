use std::collections::HashSet;

use paneflow_config::schema::{
    DetachedPaneLocation, DetachedPaneSession, LayoutNode, MAX_DETACHED_PANE_WINDOWS, SessionState,
    SurfaceDefinition, TabLayoutIds, TabSession, TabTitleSource, WorkspaceSession,
};

use crate::ids::{IdSource, PaneId, SplitId, SurfaceId, TabId, WorkspaceId};
use crate::model::{
    Child, Direction, MAX_PANE_SURFACES, MAX_TABS_PER_WORKSPACE, Node, Pane, Split, State, Surface,
    Tab, TabExtras, Workspace, WorkspaceExtras,
};
use crate::{Model, UndoHistory};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetachedBounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetachedPlacement {
    pub pane: PaneId,
    pub bounds: DetachedBounds,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub model: Model,
    pub detached: Vec<DetachedPlacement>,
    pub reassigned: usize,
    pub dropped_detached: usize,
}

struct Claims {
    seen: HashSet<u64>,
    highest: u64,
    reassigned: usize,
}

impl Claims {
    fn claim(&mut self, stated: Option<u64>) -> u64 {
        match stated {
            Some(id) if id != 0 && self.seen.insert(id) => {
                self.highest = self.highest.max(id);
                id
            }
            Some(_) => {
                self.reassigned += 1;
                0
            }
            None => 0,
        }
    }
}

pub fn load(session: &SessionState) -> Loaded {
    let mut claims = Claims {
        seen: HashSet::new(),
        highest: 0,
        reassigned: 0,
    };
    let mut workspaces: Vec<Workspace> = session
        .workspaces
        .iter()
        .map(|ws| load_workspace(ws, &mut claims))
        .collect();
    let floor = session.next_layout_id.unwrap_or(0).max(claims.highest + 1);
    let counter = IdSource::starting_at(floor);
    for ws in &mut workspaces {
        fill_workspace(ws, &counter);
    }
    if claims.reassigned > 0 {
        tracing::warn!(
            reassigned = claims.reassigned,
            "session load: corrupt or duplicated layout ids received fresh ids"
        );
    }
    let mut state = State {
        version: session.version,
        active_workspace: session.active_workspace,
        active_space: session.active_space.clone(),
        workspaces,
    };
    let (detached, dropped_detached) = load_detached(&session.detached_panes, &state);
    for placement in &detached {
        if let Some((ws, tab)) = state.find_pane(placement.pane)
            && let Some(pane) = state.workspaces[ws].tabs[tab]
                .root
                .as_mut()
                .and_then(|root| root.pane_mut(placement.pane))
        {
            pane.detached = true;
        }
    }
    let mut model = Model::from_parts(state, counter, UndoHistory::default());
    model.set_revision(session.layout_revision.unwrap_or(0));
    Loaded {
        model,
        detached,
        reassigned: claims.reassigned,
        dropped_detached,
    }
}

fn load_workspace(ws: &WorkspaceSession, claims: &mut Claims) -> Workspace {
    let mut tabs: Vec<Tab> = ws
        .tabs
        .iter()
        .take(MAX_TABS_PER_WORKSPACE)
        .map(|tab| load_tab(tab, claims))
        .collect();
    if ws.tabs.len() > MAX_TABS_PER_WORKSPACE {
        tracing::warn!(
            tabs = ws.tabs.len(),
            cap = MAX_TABS_PER_WORKSPACE,
            "session load: workspace holds too many tabs, keeping the first ones"
        );
    }
    if tabs.is_empty() {
        tabs.push(empty_tab(TabId(0)));
    }
    let active_tab = ws.active_tab.min(tabs.len().saturating_sub(1));
    Workspace {
        id: WorkspaceId(claims.claim(ws.layout_id)),
        title: ws.title.clone(),
        cwd: ws.cwd.clone(),
        space: ws.space.clone(),
        tabs,
        active_tab,
        extras: WorkspaceExtras {
            durable_id: ws.id.clone(),
            legacy_layout: ws.legacy_layout.clone(),
            custom_buttons: ws.custom_buttons.clone(),
            expanded_paths: ws.expanded_paths.clone(),
            managed_worktrees: ws.managed_worktrees.clone(),
            sidebar_collapsed: ws.sidebar_collapsed,
            muted: ws.muted,
        },
    }
}

pub(crate) fn empty_tab(id: TabId) -> Tab {
    Tab {
        id,
        title: String::new(),
        title_source: TabTitleSource::Preset,
        root: None,
        focused: None,
        zoomed: None,
        worktree: None,
        extras: TabExtras::default(),
    }
}

fn load_tab(tab: &TabSession, claims: &mut Claims) -> Tab {
    let ids = tab.layout_ids.as_ref();
    let id = TabId(claims.claim(ids.map(|ids| ids.tab)));
    let leaf_count = tab.layout.as_ref().map_or(0, LayoutNode::leaf_count);
    let shaped = ids.filter(|ids| ids.panes.len() == leaf_count);
    if ids.is_some() && shaped.is_none() {
        claims.reassigned += 1;
    }
    let mut leaf = 0usize;
    let root = tab
        .layout
        .as_ref()
        .map(|layout| load_node(layout, shaped, &mut leaf, claims));
    let focused = root.as_ref().and_then(Node::first_pane).map(|pane| pane.id);
    Tab {
        id,
        title: tab.title.clone(),
        title_source: tab.title_source.unwrap_or_default(),
        root,
        focused,
        zoomed: None,
        worktree: tab.worktree.clone(),
        extras: TabExtras {
            worktree_raw: tab.worktree_raw.clone(),
            unread: tab.unread,
            pull_request: tab.pull_request.clone(),
            dock: tab.dock.clone(),
        },
    }
}

fn load_node(
    node: &LayoutNode,
    ids: Option<&TabLayoutIds>,
    leaf: &mut usize,
    claims: &mut Claims,
) -> Node {
    match node {
        LayoutNode::Pane { surfaces } => {
            let index = *leaf;
            *leaf += 1;
            let pane_id = PaneId(claims.claim(ids.and_then(|ids| ids.panes.get(index).copied())));
            let stated = ids.and_then(|ids| ids.surfaces.get(index));
            let shaped =
                stated.filter(|stated| stated.len() == surfaces.len().min(MAX_PANE_SURFACES));
            if stated.is_some() && shaped.is_none() {
                claims.reassigned += 1;
            }
            let mut definitions: Vec<SurfaceDefinition> =
                surfaces.iter().take(MAX_PANE_SURFACES).cloned().collect();
            if definitions.is_empty() {
                definitions.push(SurfaceDefinition::default());
            }
            let active = definitions
                .iter()
                .position(|surface| surface.focus == Some(true))
                .unwrap_or(0);
            let surfaces = definitions
                .into_iter()
                .enumerate()
                .map(|(i, definition)| Surface {
                    id: SurfaceId(claims.claim(shaped.and_then(|ids| ids.get(i).copied()))),
                    definition,
                })
                .collect();
            Node::Pane(Pane {
                id: pane_id,
                surfaces,
                active,
                detached: false,
            })
        }
        LayoutNode::Split {
            direction,
            children,
            ..
        } => {
            let resolved = node.resolved_ratios();
            let count = children.len().max(1);
            Node::Split(Split {
                id: SplitId(0),
                direction: Direction::from_layout_str(direction),
                children: children
                    .iter()
                    .enumerate()
                    .map(|(i, child)| Child {
                        ratio: resolved.get(i).copied().unwrap_or(1.0 / count as f64) as f32,
                        node: load_node(child, ids, leaf, claims),
                    })
                    .collect(),
            })
        }
    }
}

fn fill_workspace(ws: &mut Workspace, counter: &IdSource) {
    if ws.id.0 == 0 {
        ws.id = WorkspaceId(counter.allocate());
    }
    for tab in &mut ws.tabs {
        if tab.id.0 == 0 {
            tab.id = TabId(counter.allocate());
        }
        if let Some(root) = &mut tab.root {
            fill_node(root, counter);
        }
        tab.focused = tab.root.as_ref().and_then(Node::first_pane).map(|p| p.id);
    }
}

fn fill_node(node: &mut Node, counter: &IdSource) {
    match node {
        Node::Pane(pane) => {
            if pane.id.0 == 0 {
                pane.id = PaneId(counter.allocate());
            }
            for surface in &mut pane.surfaces {
                if surface.id.0 == 0 {
                    surface.id = SurfaceId(counter.allocate());
                }
            }
        }
        Node::Split(split) => {
            split.id = SplitId(counter.allocate());
            for child in &mut split.children {
                fill_node(&mut child.node, counter);
            }
        }
    }
}

fn load_detached(saved: &[DetachedPaneSession], state: &State) -> (Vec<DetachedPlacement>, usize) {
    let mut placements: Vec<DetachedPlacement> = Vec::new();
    let mut dropped = 0usize;
    for entry in saved.iter().take(MAX_DETACHED_PANE_WINDOWS) {
        if !entry.has_valid_bounds() {
            dropped += 1;
            continue;
        }
        let by_id = entry
            .pane
            .map(PaneId)
            .filter(|pane| state.find_pane(*pane).is_some());
        let pane = by_id.or_else(|| positional_pane(state, entry));
        match pane {
            Some(pane) if !placements.iter().any(|placed| placed.pane == pane) => {
                placements.push(DetachedPlacement {
                    pane,
                    bounds: DetachedBounds {
                        x: entry.x,
                        y: entry.y,
                        width: entry.width,
                        height: entry.height,
                    },
                });
            }
            _ => dropped += 1,
        }
    }
    if dropped > 0 {
        tracing::warn!(
            dropped,
            "session load: detached pane placements that name no pane were dropped"
        );
    }
    (placements, dropped)
}

fn positional_pane(state: &State, entry: &DetachedPaneSession) -> Option<PaneId> {
    let DetachedPaneLocation {
        workspace,
        tab,
        leaf,
    } = entry.location;
    let root = state
        .workspaces
        .get(workspace)?
        .tabs
        .get(tab)?
        .root
        .as_ref()?;
    let panes = root.pane_ids();
    if entry.layout_leaf_count != Some(panes.len()) {
        return None;
    }
    panes.get(leaf).copied()
}

pub fn export(model: &Model, detached: &[DetachedPlacement]) -> SessionState {
    let state = model.state();
    SessionState {
        version: state.version,
        active_workspace: state.active_workspace,
        active_space: state.active_space.clone(),
        workspaces: state.workspaces.iter().map(export_workspace).collect(),
        detached_panes: export_detached(state, detached),
        next_layout_id: Some(model.next_id()),
        layout_revision: Some(model.revision()),
    }
}

fn export_workspace(ws: &Workspace) -> WorkspaceSession {
    WorkspaceSession {
        id: ws.extras.durable_id.clone(),
        layout_id: Some(ws.id.0),
        title: ws.title.clone(),
        cwd: ws.cwd.clone(),
        tabs: ws.tabs.iter().map(export_tab).collect(),
        active_tab: ws.active_tab,
        legacy_layout: ws.extras.legacy_layout.clone(),
        custom_buttons: ws.extras.custom_buttons.clone(),
        expanded_paths: ws.extras.expanded_paths.clone(),
        managed_worktrees: ws.extras.managed_worktrees.clone(),
        sidebar_collapsed: ws.extras.sidebar_collapsed,
        muted: ws.extras.muted,
        space: ws.space.clone(),
    }
}

fn export_tab(tab: &Tab) -> TabSession {
    let panes: Vec<&Pane> = tab.root.as_ref().map(Node::panes).unwrap_or_default();
    TabSession {
        title: tab.title.clone(),
        title_source: Some(tab.title_source),
        layout: tab.root.as_ref().map(export_node),
        worktree: tab.worktree.clone(),
        worktree_raw: tab.extras.worktree_raw.clone(),
        unread: tab.extras.unread,
        pull_request: tab.extras.pull_request.clone(),
        dock: tab.extras.dock.clone(),
        layout_ids: Some(TabLayoutIds {
            tab: tab.id.0,
            panes: panes.iter().map(|pane| pane.id.0).collect(),
            surfaces: panes
                .iter()
                .map(|pane| pane.surfaces.iter().map(|surface| surface.id.0).collect())
                .collect(),
        }),
    }
}

pub fn export_node(node: &Node) -> LayoutNode {
    match node {
        Node::Pane(pane) => LayoutNode::Pane {
            surfaces: pane
                .surfaces
                .iter()
                .enumerate()
                .map(|(index, surface)| SurfaceDefinition {
                    focus: Some(index == pane.active),
                    ..surface.definition.clone()
                })
                .collect(),
        },
        Node::Split(split) => LayoutNode::Split {
            direction: split.direction.as_layout_str().to_string(),
            ratio: None,
            ratios: Some(
                split
                    .children
                    .iter()
                    .map(|child| f64::from(child.ratio))
                    .collect(),
            ),
            children: split
                .children
                .iter()
                .map(|child| export_node(&child.node))
                .collect(),
        },
    }
}

fn export_detached(state: &State, detached: &[DetachedPlacement]) -> Vec<DetachedPaneSession> {
    let mut saved = Vec::new();
    for (ws_idx, ws) in state.workspaces.iter().enumerate() {
        for (tab_idx, tab) in ws.tabs.iter().enumerate() {
            let Some(root) = &tab.root else {
                continue;
            };
            let panes = root.pane_ids();
            for (leaf, pane) in panes.iter().enumerate() {
                if let Some(placement) = detached.iter().find(|placed| placed.pane == *pane) {
                    saved.push(DetachedPaneSession {
                        location: DetachedPaneLocation {
                            workspace: ws_idx,
                            tab: tab_idx,
                            leaf,
                        },
                        layout_leaf_count: Some(panes.len()),
                        pane: Some(pane.0),
                        x: placement.bounds.x,
                        y: placement.bounds.y,
                        width: placement.bounds.width,
                        height: placement.bounds.height,
                    });
                }
            }
        }
    }
    saved.truncate(MAX_DETACHED_PANE_WINDOWS);
    saved
}

#[cfg(test)]
mod tests {
    use paneflow_config::schema::{
        LayoutNode, SessionState, SurfaceDefinition, TabLayoutIds, TabSession, WorkspaceSession,
    };

    fn workspace(layout_id: Option<u64>, tab: u64) -> WorkspaceSession {
        WorkspaceSession {
            id: None,
            layout_id,
            title: "alpha".into(),
            cwd: "/work".into(),
            tabs: vec![TabSession {
                layout: Some(LayoutNode::Pane {
                    surfaces: vec![SurfaceDefinition::default()],
                }),
                layout_ids: Some(TabLayoutIds {
                    tab,
                    panes: vec![tab + 1],
                    surfaces: vec![vec![tab + 2]],
                }),
                ..TabSession::default()
            }],
            active_tab: 0,
            legacy_layout: None,
            custom_buttons: Vec::new(),
            expanded_paths: Vec::new(),
            managed_worktrees: Vec::new(),
            sidebar_collapsed: false,
            muted: false,
            space: None,
        }
    }

    #[tracing_test::traced_test]
    #[test]
    fn a_duplicated_id_is_reported_by_one_warning() {
        let state = SessionState {
            version: 3,
            active_workspace: 0,
            workspaces: vec![workspace(Some(1), 10), workspace(Some(1), 20)],
            detached_panes: Vec::new(),
            active_space: None,
            next_layout_id: Some(30),
            layout_revision: None,
        };
        let loaded = super::load(&state);
        assert_eq!(loaded.reassigned, 1);
        assert!(logs_contain("corrupt or duplicated layout ids"));
        logs_assert(
            |lines| match lines.iter().filter(|line| line.contains("WARN")).count() {
                1 => Ok(()),
                n => Err(format!("{n} warnings logged")),
            },
        );
        let second = loaded.model.state().workspaces[1].id.get();
        assert!(second >= 30);
    }
}
