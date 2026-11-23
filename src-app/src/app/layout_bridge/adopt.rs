use gpui::{App, Context, Entity};
use paneflow_config::schema::SurfaceDefinition;
use paneflow_layout::{
    Child, Direction, Node, Pane, PaneId, Split, SplitId, State, Surface, SurfaceId, Tab,
    TabExtras, TabId, Workspace, WorkspaceExtras, WorkspaceId,
};

use super::LayoutBridge;
use crate::PaneFlowApp;
use crate::layout::{LayoutTree, SplitDirection};
use crate::pane::PaneSurface;

pub(super) fn surface_definition(surface: &PaneSurface, cx: &App) -> SurfaceDefinition {
    match surface {
        PaneSurface::Terminal(view) => {
            let terminal = &view.read(cx).terminal;
            SurfaceDefinition {
                surface_type: Some("terminal".to_string()),
                env: view.read(cx).persisted_env(),
                custom_name: terminal.custom_name.clone(),
                cwd: terminal.current_cwd.clone(),
                font_size: terminal.font_size_override,
                session: Some(terminal.session_id.clone()),
                agent: terminal.detected_agent.map(|agent| agent.tag().to_string()),
                agent_session: view.read(cx).agent_session().cloned(),
                ..SurfaceDefinition::default()
            }
        }
        PaneSurface::Markdown(view) => SurfaceDefinition {
            surface_type: Some("markdown".to_string()),
            path: Some(view.read(cx).path.display().to_string()),
            ..SurfaceDefinition::default()
        },
    }
}

impl LayoutBridge {
    pub(super) fn own_or_reserved(&mut self, own: Option<u64>) -> u64 {
        match own {
            Some(id) if self.unclaimed(id) && self.model.reserve_exact(id) => id,
            _ => self.model.reserve(),
        }
    }

    fn unclaimed(&self, id: u64) -> bool {
        id != 0
            && !self.panes.contains_key(&PaneId(id))
            && !self.surfaces.contains_key(&SurfaceId(id))
            && !self.workspaces.contains_key(&WorkspaceId(id))
            && !self.tabs.contains_key(&TabId(id))
            && !self.splits.contains_key(&SplitId(id))
            && !self.model.state().contains_id(id)
    }

    fn adopt_surface(&mut self, surface: &PaneSurface, cx: &App) -> Surface {
        let entity = super::surface_entity(surface);
        let id = match self.surface_ids.get(&entity) {
            Some(id) => *id,
            None => {
                let own = match surface {
                    PaneSurface::Terminal(view) => Some(SurfaceId(view.read(cx).surface_id())),
                    PaneSurface::Markdown(_) => None,
                };
                let id = own
                    .filter(|id| self.unclaimed(id.0))
                    .unwrap_or_else(|| SurfaceId(self.model.reserve()));
                self.register_surface(id, surface.clone());
                id
            }
        };
        let mut definition = self
            .model
            .state()
            .surface(id)
            .map(|known| known.definition.clone())
            .unwrap_or_default();
        let live = surface_definition(surface, cx);
        definition.surface_type = live.surface_type;
        definition.custom_name = live.custom_name;
        definition.session = live.session;
        definition.font_size = live.font_size;
        if live.agent.is_some() {
            definition.agent = live.agent;
        }
        if live.agent_session.is_some() {
            definition.agent_session = live.agent_session;
        }
        if live.cwd.is_some() {
            definition.cwd = live.cwd;
        }
        if live.path.is_some() {
            definition.path = live.path;
        }
        if let Some(live_env) = live.env {
            definition
                .env
                .get_or_insert_with(Default::default)
                .extend(live_env);
        }
        Surface { id, definition }
    }

    fn adopt_pane(&mut self, pane: &Entity<crate::pane::Pane>, cx: &App) -> Pane {
        let id = match self.pane_ids.get(&pane.entity_id()) {
            Some(id) => *id,
            None => {
                let id = pane
                    .read(cx)
                    .restored_layout_id
                    .map(PaneId)
                    .filter(|id| self.unclaimed(id.0))
                    .unwrap_or_else(|| PaneId(self.model.reserve()));
                self.register_pane(id, pane.clone());
                id
            }
        };
        let pane_ref = pane.read(cx);
        let surfaces: Vec<PaneSurface> = pane_ref.surfaces().to_vec();
        let active = pane_ref.active_surface_idx();
        let detached = pane_ref.is_detached();
        Pane {
            id,
            surfaces: surfaces
                .iter()
                .map(|surface| self.adopt_surface(surface, cx))
                .collect(),
            active,
            detached,
        }
    }

    fn model_split_ratios(&self, id: SplitId) -> Option<Vec<f32>> {
        fn find(node: &Node, id: SplitId) -> Option<Vec<f32>> {
            let Node::Split(split) = node else {
                return None;
            };
            if split.id == id {
                return Some(split.children.iter().map(|child| child.ratio).collect());
            }
            split
                .children
                .iter()
                .find_map(|child| find(&child.node, id))
        }
        self.model.state().workspaces.iter().find_map(|ws| {
            ws.tabs
                .iter()
                .find_map(|tab| tab.root.as_ref().and_then(|root| find(root, id)))
        })
    }

    fn adopt_node(&mut self, tree: &LayoutTree, cx: &App) -> Node {
        match tree {
            LayoutTree::Leaf(pane) => Node::Pane(self.adopt_pane(pane, cx)),
            LayoutTree::Container {
                direction,
                children,
                drag,
                ..
            } => {
                let cells = tree.split_cells();
                let key = cells.as_ref().map(crate::layout::SplitCells::key);
                let id = match key.and_then(|key| self.split_ids.get(&key)) {
                    Some(id) => *id,
                    None => {
                        let id = SplitId(self.model.reserve());
                        if let (Some(key), Some(cells)) = (key, cells) {
                            self.split_ids.insert(key, id);
                            self.splits.insert(id, cells);
                        }
                        id
                    }
                };
                let resizing = drag.get().is_some() || key.is_some() && key == self.resizing;
                let held = resizing
                    .then(|| self.model_split_ratios(id))
                    .flatten()
                    .filter(|ratios| ratios.len() == children.len());
                Node::Split(Split {
                    id,
                    direction: match direction {
                        SplitDirection::Vertical => Direction::Vertical,
                        SplitDirection::Horizontal => Direction::Horizontal,
                    },
                    children: children
                        .iter()
                        .enumerate()
                        .map(|(idx, child)| Child {
                            node: self.adopt_node(&child.node, cx),
                            ratio: held
                                .as_ref()
                                .map_or_else(|| child.ratio.get(), |ratios| ratios[idx]),
                        })
                        .collect(),
                })
            }
        }
    }

    fn adopt_tab(&mut self, tab: &crate::workspace::Tab, live: TabExtras, cx: &App) -> Tab {
        let id = match self.tab_ids.get(&tab.id) {
            Some(id) => *id,
            None => {
                let id = Some(TabId(tab.id))
                    .filter(|id| self.unclaimed(id.0))
                    .unwrap_or_else(|| TabId(self.model.reserve()));
                self.tab_ids.insert(tab.id, id);
                self.tabs.insert(id, tab.id);
                id
            }
        };
        let previous = self
            .model
            .state()
            .find_tab(id)
            .map(|(ws, idx)| self.model.state().workspaces[ws].tabs[idx].focused);
        let full = tab.saved_layout.as_ref().or(tab.root.as_ref());
        let root = full.map(|tree| self.adopt_node(tree, cx));
        let zoomed = tab
            .saved_layout
            .as_ref()
            .and(tab.root.as_ref())
            .and_then(LayoutTree::first_leaf)
            .and_then(|pane| self.pane_ids.get(&pane.entity_id()).copied());
        let focused = previous
            .flatten()
            .filter(|pane| root.as_ref().is_some_and(|root| root.contains(*pane)))
            .or_else(|| root.as_ref().and_then(Node::first_pane).map(|pane| pane.id));
        Tab {
            id,
            title: tab.title().to_string(),
            title_source: tab.title_source(),
            root,
            focused,
            zoomed,
            worktree: tab
                .worktree
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            extras: live,
        }
    }

    fn adopt_workspace(
        &mut self,
        ws: &crate::workspace::Workspace,
        live: (WorkspaceExtras, Vec<TabExtras>),
        cx: &App,
    ) -> Workspace {
        let id = match self.workspace_ids.get(&ws.id) {
            Some(id) => *id,
            None => {
                let id = Some(WorkspaceId(ws.id))
                    .filter(|id| self.unclaimed(id.0))
                    .unwrap_or_else(|| WorkspaceId(self.model.reserve()));
                self.workspace_ids.insert(ws.id, id);
                self.workspaces.insert(id, ws.id);
                id
            }
        };
        let legacy_layout = self
            .model
            .state()
            .workspace(id)
            .and_then(|known| known.extras.legacy_layout.clone());
        let (extras, tab_extras) = live;
        let mut tab_extras = tab_extras.into_iter();
        Workspace {
            id,
            title: ws.title.clone(),
            cwd: ws.cwd.clone(),
            space: ws.space.clone(),
            tabs: ws
                .tabs()
                .iter()
                .map(|tab| {
                    let live = tab_extras.next().unwrap_or_default();
                    self.adopt_tab(tab, live, cx)
                })
                .collect(),
            active_tab: ws.active_tab_idx(),
            extras: WorkspaceExtras {
                legacy_layout,
                ..extras
            },
        }
    }
}

impl PaneFlowApp {
    pub(crate) fn hint_layout_focus(&mut self, pane: &Entity<crate::pane::Pane>) {
        let Some(id) = self.layout.pane_id(pane) else {
            return;
        };
        let mut state = self.layout.model.state().clone();
        let Some((ws, tab)) = state.find_pane(id) else {
            return;
        };
        let tab = &mut state.workspaces[ws].tabs[tab];
        if tab.focused == Some(id) {
            return;
        }
        tab.focused = Some(id);
        self.layout.resync_recorded(state);
    }

    fn live_extras(
        &self,
        ws: &crate::workspace::Workspace,
        cx: &App,
    ) -> (WorkspaceExtras, Vec<TabExtras>) {
        let workspace = WorkspaceExtras {
            durable_id: Some(ws.durable_id.clone()),
            legacy_layout: None,
            custom_buttons: ws.custom_buttons.clone(),
            expanded_paths: crate::app::session::persisted_expanded_paths(
                &ws.cwd,
                &ws.files_expanded,
            ),
            managed_worktrees: ws
                .managed_worktrees
                .iter()
                .map(|wt| paneflow_config::schema::ManagedWorktreeDef {
                    path: wt.path.to_string_lossy().into_owned(),
                    repo_root: wt.repo_root.to_string_lossy().into_owned(),
                    branch: wt.branch.clone(),
                    teardown: wt.teardown.as_str().to_string(),
                })
                .collect(),
            sidebar_collapsed: !ws.sidebar_expanded,
            muted: ws.muted,
        };
        let tabs = ws
            .tabs()
            .iter()
            .map(|tab| TabExtras {
                worktree_raw: tab
                    .worktree
                    .as_deref()
                    .filter(|path| path.to_str().is_none())
                    .map(crate::runtime_paths::path_to_raw),
                unread: ws
                    .agent_completion_notification
                    .is_unread_for(&tab.surface_ids(cx)),
                pull_request: self.tab_pull_request(ws, tab).map(|pr| {
                    paneflow_config::schema::PullRequestSession {
                        branch: self.tab_row_branch(ws, tab),
                        number: pr.number,
                        state: pr.state.wire_str().to_string(),
                    }
                }),
                dock: None,
            })
            .collect();
        (workspace, tabs)
    }

    pub(crate) fn adopt_layout(&mut self, cx: &mut Context<Self>) {
        let extras: Vec<(WorkspaceExtras, Vec<TabExtras>)> = self
            .workspaces
            .iter()
            .map(|ws| self.live_extras(ws, cx))
            .collect();
        let mut bridge = std::mem::take(&mut self.layout);
        let state = State {
            version: paneflow_config::schema::SESSION_SCHEMA_VERSION,
            workspaces: self
                .workspaces
                .iter()
                .zip(extras)
                .map(|(ws, live)| bridge.adopt_workspace(ws, live, cx))
                .collect(),
            active_workspace: self.active_idx,
            active_space: self.active_space.clone(),
        };
        bridge.resync_recorded(state);
        self.layout = bridge;
    }
}
