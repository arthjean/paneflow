use paneflow_config::schema::{
    ButtonCommand, DockSession, LayoutNode, ManagedWorktreeDef, PullRequestSession, SessionId,
    SpaceId, SurfaceDefinition, TabTitleSource,
};

use crate::ids::{PaneId, SplitId, SurfaceId, TabId, WorkspaceId};

pub const MAX_PANES: usize = paneflow_config::schema::MAX_LAYOUT_LEAVES;
pub const MAX_TABS_PER_WORKSPACE: usize = 32;
pub const MAX_PANE_SURFACES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    Horizontal,
    Vertical,
}

impl Direction {
    pub fn from_layout_str(value: &str) -> Self {
        match value {
            "vertical" => Self::Vertical,
            _ => Self::Horizontal,
        }
    }

    pub fn as_layout_str(self) -> &'static str {
        match self {
            Self::Horizontal => "horizontal",
            Self::Vertical => "vertical",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceKind {
    Terminal,
    Markdown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Surface {
    pub id: SurfaceId,
    pub definition: SurfaceDefinition,
}

impl Surface {
    pub fn kind(&self) -> SurfaceKind {
        if self.definition.is_terminal() {
            SurfaceKind::Terminal
        } else {
            SurfaceKind::Markdown
        }
    }

    pub fn session(&self) -> Option<&SessionId> {
        match self.kind() {
            SurfaceKind::Terminal => self.definition.session.as_ref(),
            SurfaceKind::Markdown => None,
        }
    }

    pub fn display_name(&self) -> Option<&str> {
        self.definition
            .custom_name
            .as_deref()
            .or(self.definition.name.as_deref())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pane {
    pub id: PaneId,
    pub surfaces: Vec<Surface>,
    pub active: usize,
    pub detached: bool,
}

impl Pane {
    pub fn active_surface(&self) -> Option<&Surface> {
        self.surfaces
            .get(self.active.min(self.surfaces.len().saturating_sub(1)))
    }

    pub fn surface_index(&self, id: SurfaceId) -> Option<usize> {
        self.surfaces.iter().position(|surface| surface.id == id)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Child {
    pub node: Node,
    pub ratio: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Split {
    pub id: SplitId,
    pub direction: Direction,
    pub children: Vec<Child>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Pane(Pane),
    Split(Split),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TabExtras {
    pub worktree_raw: Option<Vec<u8>>,
    pub unread: bool,
    pub pull_request: Option<PullRequestSession>,
    pub dock: Option<DockSession>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tab {
    pub id: TabId,
    pub title: String,
    pub title_source: TabTitleSource,
    pub root: Option<Node>,
    pub focused: Option<PaneId>,
    pub zoomed: Option<PaneId>,
    pub worktree: Option<String>,
    pub extras: TabExtras,
}

impl Tab {
    pub fn is_empty_shell(&self) -> bool {
        self.title.is_empty() && self.root.is_none()
    }

    pub fn pane_count(&self) -> usize {
        self.root.as_ref().map_or(0, Node::leaf_count)
    }

    pub fn is_zoomed(&self) -> bool {
        self.zoomed.is_some()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkspaceExtras {
    pub durable_id: Option<paneflow_config::schema::WorkspaceId>,
    pub legacy_layout: Option<LayoutNode>,
    pub custom_buttons: Vec<ButtonCommand>,
    pub expanded_paths: Vec<String>,
    pub managed_worktrees: Vec<ManagedWorktreeDef>,
    pub sidebar_collapsed: bool,
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub title: String,
    pub cwd: String,
    pub space: Option<SpaceId>,
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    pub extras: WorkspaceExtras,
}

impl Workspace {
    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs
            .get(self.active_tab.min(self.tabs.len().saturating_sub(1)))
    }

    pub fn is_empty_shell(&self) -> bool {
        matches!(self.tabs.as_slice(), [tab] if tab.is_empty_shell())
    }

    pub fn tab_index(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == id)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub version: u32,
    pub workspaces: Vec<Workspace>,
    pub active_workspace: usize,
    pub active_space: Option<SpaceId>,
}

impl State {
    pub fn workspace_index(&self, id: WorkspaceId) -> Option<usize> {
        self.workspaces.iter().position(|ws| ws.id == id)
    }

    pub fn workspace(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.workspaces.iter().find(|ws| ws.id == id)
    }

    pub fn find_tab(&self, id: TabId) -> Option<(usize, usize)> {
        self.workspaces
            .iter()
            .enumerate()
            .find_map(|(ws_idx, ws)| ws.tab_index(id).map(|tab_idx| (ws_idx, tab_idx)))
    }

    pub fn tab(&self, id: TabId) -> Option<&Tab> {
        let (ws, tab) = self.find_tab(id)?;
        self.workspaces.get(ws)?.tabs.get(tab)
    }

    pub fn find_pane(&self, id: PaneId) -> Option<(usize, usize)> {
        self.workspaces.iter().enumerate().find_map(|(ws_idx, ws)| {
            ws.tabs
                .iter()
                .position(|tab| tab.root.as_ref().is_some_and(|root| root.contains(id)))
                .map(|tab_idx| (ws_idx, tab_idx))
        })
    }

    pub fn pane(&self, id: PaneId) -> Option<&Pane> {
        let (ws, tab) = self.find_pane(id)?;
        self.workspaces
            .get(ws)?
            .tabs
            .get(tab)?
            .root
            .as_ref()?
            .pane(id)
    }

    pub fn find_surface(&self, id: SurfaceId) -> Option<SurfaceLocation> {
        for (ws_idx, ws) in self.workspaces.iter().enumerate() {
            for (tab_idx, tab) in ws.tabs.iter().enumerate() {
                let Some(root) = &tab.root else {
                    continue;
                };
                for pane in root.panes() {
                    if let Some(index) = pane.surface_index(id) {
                        return Some(SurfaceLocation {
                            workspace: ws.id,
                            tab: tab.id,
                            pane: pane.id,
                            index,
                            workspace_index: ws_idx,
                            tab_index: tab_idx,
                        });
                    }
                }
            }
        }
        None
    }

    pub fn surface(&self, id: SurfaceId) -> Option<&Surface> {
        let location = self.find_surface(id)?;
        self.pane(location.pane)?.surfaces.get(location.index)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceLocation {
    pub workspace: WorkspaceId,
    pub tab: TabId,
    pub pane: PaneId,
    pub index: usize,
    pub workspace_index: usize,
    pub tab_index: usize,
}
