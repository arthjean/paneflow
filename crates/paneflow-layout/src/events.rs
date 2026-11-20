use std::collections::BTreeMap;

use crate::ids::{PaneId, SurfaceId, TabId, WorkspaceId};
use crate::model::{Node, State};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    WorkspaceCreated { workspace: WorkspaceId },
    WorkspaceClosed { workspace: WorkspaceId },
    WorkspaceRenamed { workspace: WorkspaceId },
    WorkspaceSelected { workspace: WorkspaceId },
    TabCreated { workspace: WorkspaceId, tab: TabId },
    TabClosed { workspace: WorkspaceId, tab: TabId },
    TabRenamed { tab: TabId },
    TabSelected { workspace: WorkspaceId, tab: TabId },
    PaneCreated { tab: TabId, pane: PaneId },
    PaneClosed { tab: TabId, pane: PaneId },
    PaneFocused { tab: TabId, pane: PaneId },
    SurfaceCreated { pane: PaneId, surface: SurfaceId },
    SurfaceClosed { pane: PaneId, surface: SurfaceId },
    SurfaceRenamed { surface: SurfaceId },
    LayoutChanged { tab: TabId },
}

impl Event {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::WorkspaceCreated { .. } => "workspace.created",
            Self::WorkspaceClosed { .. } => "workspace.closed",
            Self::WorkspaceRenamed { .. } => "workspace.renamed",
            Self::WorkspaceSelected { .. } => "workspace.selected",
            Self::TabCreated { .. } => "tab.created",
            Self::TabClosed { .. } => "tab.closed",
            Self::TabRenamed { .. } => "tab.renamed",
            Self::TabSelected { .. } => "tab.selected",
            Self::PaneCreated { .. } => "pane.created",
            Self::PaneClosed { .. } => "pane.closed",
            Self::PaneFocused { .. } => "pane.focused",
            Self::SurfaceCreated { .. } => "surface.created",
            Self::SurfaceClosed { .. } => "surface.closed",
            Self::SurfaceRenamed { .. } => "surface.renamed",
            Self::LayoutChanged { .. } => "layout.changed",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventScope {
    pub workspace: Option<WorkspaceId>,
    pub tab: Option<TabId>,
    pub pane: Option<PaneId>,
    pub surface: Option<SurfaceId>,
    pub session: Option<paneflow_config::schema::SessionId>,
}

fn tab_workspace(state: &State, tab: TabId) -> Option<WorkspaceId> {
    let (ws, _) = state.find_tab(tab)?;
    Some(state.workspaces.get(ws)?.id)
}

fn pane_tab(state: &State, pane: PaneId) -> Option<(WorkspaceId, TabId)> {
    let (ws, tab) = state.find_pane(pane)?;
    let workspace = state.workspaces.get(ws)?;
    Some((workspace.id, workspace.tabs.get(tab)?.id))
}

impl Event {
    pub fn scope(&self, before: &State, after: &State) -> EventScope {
        let either =
            |find: &dyn Fn(&State) -> Option<WorkspaceId>| find(after).or_else(|| find(before));
        match *self {
            Self::WorkspaceCreated { workspace }
            | Self::WorkspaceClosed { workspace }
            | Self::WorkspaceRenamed { workspace }
            | Self::WorkspaceSelected { workspace } => EventScope {
                workspace: Some(workspace),
                ..EventScope::default()
            },
            Self::TabCreated { workspace, tab }
            | Self::TabClosed { workspace, tab }
            | Self::TabSelected { workspace, tab } => EventScope {
                workspace: Some(workspace),
                tab: Some(tab),
                ..EventScope::default()
            },
            Self::TabRenamed { tab } | Self::LayoutChanged { tab } => EventScope {
                workspace: either(&|state| tab_workspace(state, tab)),
                tab: Some(tab),
                ..EventScope::default()
            },
            Self::PaneCreated { tab, pane }
            | Self::PaneClosed { tab, pane }
            | Self::PaneFocused { tab, pane } => EventScope {
                workspace: either(&|state| tab_workspace(state, tab)),
                tab: Some(tab),
                pane: Some(pane),
                ..EventScope::default()
            },
            Self::SurfaceCreated { pane, surface } | Self::SurfaceClosed { pane, surface } => {
                surface_scope(before, after, Some(pane), surface)
            }
            Self::SurfaceRenamed { surface } => surface_scope(before, after, None, surface),
        }
    }
}

fn surface_scope(
    before: &State,
    after: &State,
    pane: Option<PaneId>,
    surface: SurfaceId,
) -> EventScope {
    let located = after
        .find_surface(surface)
        .map(|location| (after, location))
        .or_else(|| {
            before
                .find_surface(surface)
                .map(|location| (before, location))
        });
    let pane = pane.or(located.map(|(_, location)| location.pane));
    let owner = pane.and_then(|pane| pane_tab(after, pane).or_else(|| pane_tab(before, pane)));
    EventScope {
        workspace: owner.map(|(workspace, _)| workspace),
        tab: owner.map(|(_, tab)| tab),
        pane,
        surface: Some(surface),
        session: located
            .and_then(|(state, _)| state.surface(surface))
            .and_then(|known| known.session().cloned()),
    }
}

type TabEntry<'a> = (
    WorkspaceId,
    &'a str,
    Option<PaneId>,
    Option<&'a Node>,
    Option<PaneId>,
);

struct Index<'a> {
    workspaces: BTreeMap<WorkspaceId, &'a str>,
    tabs: BTreeMap<TabId, TabEntry<'a>>,
    panes: BTreeMap<PaneId, TabId>,
    surfaces: BTreeMap<SurfaceId, (PaneId, Option<&'a str>)>,
}

fn index(state: &State) -> Index<'_> {
    let mut out = Index {
        workspaces: BTreeMap::new(),
        tabs: BTreeMap::new(),
        panes: BTreeMap::new(),
        surfaces: BTreeMap::new(),
    };
    for ws in &state.workspaces {
        out.workspaces.insert(ws.id, ws.title.as_str());
        for tab in &ws.tabs {
            out.tabs.insert(
                tab.id,
                (
                    ws.id,
                    tab.title.as_str(),
                    tab.focused,
                    tab.root.as_ref(),
                    tab.zoomed,
                ),
            );
            let Some(root) = &tab.root else {
                continue;
            };
            for pane in root.panes() {
                out.panes.insert(pane.id, tab.id);
                for surface in &pane.surfaces {
                    out.surfaces
                        .insert(surface.id, (pane.id, surface.display_name()));
                }
            }
        }
    }
    out
}

fn shown(state: &State) -> Option<(WorkspaceId, TabId)> {
    let ws = state
        .workspaces
        .get(state.active_workspace)
        .filter(|ws| ws.space == state.active_space)?;
    Some((ws.id, ws.active_tab()?.id))
}

fn same_shape(before: &Node, after: &Node) -> bool {
    match (before, after) {
        (Node::Pane(old), Node::Pane(new)) => {
            old.id == new.id
                && old.active == new.active
                && old.detached == new.detached
                && old.surfaces.len() == new.surfaces.len()
                && old
                    .surfaces
                    .iter()
                    .zip(&new.surfaces)
                    .all(|(old, new)| old.id == new.id)
        }
        (Node::Split(old), Node::Split(new)) => {
            old.id == new.id
                && old.direction == new.direction
                && old.children.len() == new.children.len()
                && old.children.iter().zip(&new.children).all(|(old, new)| {
                    old.ratio.to_bits() == new.ratio.to_bits() && same_shape(&old.node, &new.node)
                })
        }
        _ => false,
    }
}

fn shape_differs(before: Option<&Node>, after: Option<&Node>) -> bool {
    match (before, after) {
        (Some(before), Some(after)) => !same_shape(before, after),
        (None, None) => false,
        _ => true,
    }
}

pub(crate) fn diff(before: &State, after: &State) -> Vec<Event> {
    let old = index(before);
    let new = index(after);
    let mut events = Vec::new();

    for (id, title) in &new.workspaces {
        match old.workspaces.get(id) {
            None => events.push(Event::WorkspaceCreated { workspace: *id }),
            Some(previous) if previous != title => {
                events.push(Event::WorkspaceRenamed { workspace: *id });
            }
            Some(_) => {}
        }
    }
    for (id, (ws, title, _, _, _)) in &new.tabs {
        match old.tabs.get(id) {
            None => events.push(Event::TabCreated {
                workspace: *ws,
                tab: *id,
            }),
            Some((_, previous, _, _, _)) if previous != title => {
                events.push(Event::TabRenamed { tab: *id });
            }
            Some(_) => {}
        }
    }
    for (id, tab) in &new.panes {
        if !old.panes.contains_key(id) {
            events.push(Event::PaneCreated {
                tab: *tab,
                pane: *id,
            });
        }
    }
    for (id, (pane, name)) in &new.surfaces {
        match old.surfaces.get(id) {
            None => events.push(Event::SurfaceCreated {
                pane: *pane,
                surface: *id,
            }),
            Some((_, previous)) if previous != name => {
                events.push(Event::SurfaceRenamed { surface: *id });
            }
            Some(_) => {}
        }
    }
    for (id, (pane, _)) in &old.surfaces {
        if !new.surfaces.contains_key(id) {
            events.push(Event::SurfaceClosed {
                pane: *pane,
                surface: *id,
            });
        }
    }
    for (id, tab) in &old.panes {
        if !new.panes.contains_key(id) {
            events.push(Event::PaneClosed {
                tab: *tab,
                pane: *id,
            });
        }
    }
    for (id, (ws, _, _, _, _)) in &old.tabs {
        if !new.tabs.contains_key(id) {
            events.push(Event::TabClosed {
                workspace: *ws,
                tab: *id,
            });
        }
    }
    for id in old.workspaces.keys() {
        if !new.workspaces.contains_key(id) {
            events.push(Event::WorkspaceClosed { workspace: *id });
        }
    }
    for (id, (_, _, focused, root, zoomed)) in &new.tabs {
        let Some((_, _, old_focused, old_root, old_zoomed)) = old.tabs.get(id) else {
            continue;
        };
        if shape_differs(*old_root, *root) || old_zoomed != zoomed {
            events.push(Event::LayoutChanged { tab: *id });
        }
        if let Some(pane) = focused
            && old_focused != focused
        {
            events.push(Event::PaneFocused {
                tab: *id,
                pane: *pane,
            });
        }
    }
    let (old_shown, new_shown) = (shown(before), shown(after));
    if old_shown.map(|(ws, _)| ws) != new_shown.map(|(ws, _)| ws)
        && let Some((ws, _)) = new_shown
    {
        events.push(Event::WorkspaceSelected { workspace: ws });
    }
    for ws in &after.workspaces {
        let Some(active) = ws.active_tab() else {
            continue;
        };
        let previous = before
            .workspace(ws.id)
            .and_then(|old| old.active_tab())
            .map(|tab| tab.id);
        if previous.is_some() && previous != Some(active.id) {
            events.push(Event::TabSelected {
                workspace: ws.id,
                tab: active.id,
            });
        }
    }
    events
}
