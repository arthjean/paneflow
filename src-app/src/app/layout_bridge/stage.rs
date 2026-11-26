use gpui::{App, Context, Entity};
use paneflow_config::schema::TabTitleSource;
use paneflow_layout::{
    NewNode, NewPane, NewSurface, NewTab, NewWorkspace, Op, OpError, PaneId, SurfaceId, TabId,
    WorkspaceId,
};

use super::LayoutBridge;
use super::adopt::surface_definition;
use crate::PaneFlowApp;
use crate::layout::{LayoutTree, SplitDirection};
use crate::pane::Pane;
use crate::workspace::{Tab, Workspace};

impl LayoutBridge {
    fn stage_pane(&mut self, pane: &Entity<Pane>, cx: &App) -> NewPane {
        let id = PaneId(self.own_or_reserved(pane.read(cx).restored_layout_id));
        self.register_pane(id, pane.clone());
        let pane_ref = pane.read(cx);
        let active = pane_ref.active_surface_idx();
        let surfaces = pane_ref.surfaces().to_vec();
        NewPane {
            id,
            surfaces: surfaces
                .into_iter()
                .map(|surface| {
                    let own = match &surface {
                        crate::pane::PaneSurface::Terminal(view) => {
                            Some(view.read(cx).surface_id())
                        }
                        crate::pane::PaneSurface::Markdown(_)
                        | crate::pane::PaneSurface::Browser(_) => None,
                    };
                    let id = SurfaceId(self.own_or_reserved(own));
                    let definition = surface_definition(&surface, cx);
                    self.register_surface(id, surface);
                    NewSurface { id, definition }
                })
                .collect(),
            active,
        }
    }

    pub(crate) fn stage_tree(&mut self, tree: &LayoutTree, cx: &App) -> NewNode {
        match tree {
            LayoutTree::Leaf(pane) => NewNode::Pane(self.stage_pane(pane, cx)),
            LayoutTree::Container {
                direction,
                children,
                ..
            } => NewNode::Split {
                direction: match direction {
                    SplitDirection::Vertical => paneflow_layout::Direction::Vertical,
                    SplitDirection::Horizontal => paneflow_layout::Direction::Horizontal,
                },
                ratios: children.iter().map(|child| child.ratio.get()).collect(),
                children: children
                    .iter()
                    .map(|child| self.stage_tree(&child.node, cx))
                    .collect(),
            },
        }
    }

    fn stage_tab(&mut self, tab: &Tab, cx: &App) -> NewTab {
        let id = TabId(self.own_or_reserved(Some(tab.id)));
        self.tab_ids.insert(tab.id, id);
        self.tabs.insert(id, tab.id);
        let full = tab.saved_layout.as_ref().or(tab.root.as_ref());
        NewTab {
            id,
            title: tab.title().to_string(),
            title_source: tab.title_source(),
            worktree: tab
                .worktree
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            root: full.map(|tree| self.stage_tree(tree, cx)),
        }
    }
}

impl PaneFlowApp {
    pub(crate) fn split_tab_through_model(
        &mut self,
        tab: u64,
        anchor: Option<&Entity<Pane>>,
        direction: SplitDirection,
        new_pane: &Entity<Pane>,
        cx: &mut Context<Self>,
    ) -> Result<(), OpError> {
        self.adopt_layout(cx);
        let tab = self.layout.tab_id(tab).ok_or(OpError::NotFound)?;
        let target = match anchor {
            Some(anchor) => self.layout.pane_id(anchor).ok_or(OpError::NotFound)?,
            None => {
                let state = self.layout.model.state();
                let (ws, idx) = state.find_tab(tab).ok_or(OpError::NotFound)?;
                state.workspaces[ws].tabs[idx]
                    .root
                    .as_ref()
                    .and_then(paneflow_layout::Node::first_pane)
                    .map(|pane| pane.id)
                    .ok_or(OpError::NotFound)?
            }
        };
        let pane = match self
            .layout
            .stage_tree(&LayoutTree::Leaf(new_pane.clone()), cx)
        {
            NewNode::Pane(pane) => pane,
            NewNode::Split { .. } => return Err(OpError::Invalid),
        };
        self.apply_layout_op(
            Op::Split {
                target,
                direction: super::model_direction(direction),
                edge: paneflow_layout::Edge::After,
                pane,
                focus: false,
            },
            cx,
        )
        .map(|_| ())
    }

    pub(crate) fn open_tab_through_model(
        &mut self,
        ws_idx: usize,
        title: String,
        root: Option<LayoutTree>,
        worktree: Option<String>,
        fill_empty_active: bool,
        cx: &mut Context<Self>,
    ) -> Result<TabId, OpError> {
        self.adopt_layout(cx);
        let workspace = self
            .workspaces
            .get(ws_idx)
            .and_then(|ws| self.layout.workspace_id(ws.id))
            .ok_or(OpError::NotFound)?;
        let id = TabId(self.layout.model.reserve());
        let root = root.map(|tree| self.layout.stage_tree(&tree, cx));
        self.apply_layout_op(
            Op::OpenTab {
                workspace,
                tab: NewTab {
                    id,
                    title,
                    title_source: TabTitleSource::Preset,
                    worktree,
                    root,
                },
                fill_empty_active,
            },
            cx,
        )?;
        Ok(id)
    }

    pub(crate) fn insert_workspace_through_model(
        &mut self,
        mut workspace: Workspace,
        space: Option<paneflow_config::schema::SpaceId>,
        cx: &mut Context<Self>,
    ) -> usize {
        self.adopt_layout(cx);
        workspace.set_space(space);
        let id = WorkspaceId(self.layout.own_or_reserved(Some(workspace.id)));
        let tabs: Vec<NewTab> = workspace
            .tabs()
            .iter()
            .map(|tab| self.layout.stage_tab(tab, cx))
            .collect();
        let new = NewWorkspace {
            id,
            title: workspace.title.clone(),
            cwd: workspace.cwd.clone(),
            space: workspace.space.clone(),
            durable_id: Some(workspace.durable_id.clone()),
            custom_buttons: Vec::new(),
            managed_worktrees: Vec::new(),
            tabs,
            active_tab: workspace.active_tab_idx(),
        };
        let app_id = workspace.id;
        self.layout.stage_workspace(id, workspace);
        if let Err(error) = self.apply_layout_op(
            Op::CreateWorkspace {
                workspace: new,
                select: false,
            },
            cx,
        ) {
            log::error!("layout: a new workspace was refused: {error}");
        }
        self.workspaces
            .iter()
            .position(|ws| ws.id == app_id)
            .unwrap_or(self.workspaces.len().saturating_sub(1))
    }
}
