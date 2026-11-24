use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{Context, Entity};
use paneflow_layout::{Direction, Node};

use crate::PaneFlowApp;
use crate::layout::{LayoutChild, LayoutTree, SplitCells, SplitDirection};
use crate::pane::{Pane, PaneSurface};
use crate::workspace::{Tab, Workspace};

impl PaneFlowApp {
    pub(super) fn project_layout(&mut self, cx: &mut Context<Self>) {
        let state = self.layout.model.state().clone();
        let mut previous: HashMap<u64, Workspace> = std::mem::take(&mut self.workspaces)
            .into_iter()
            .map(|ws| (ws.id, ws))
            .collect();
        let mut projected = Vec::with_capacity(state.workspaces.len());
        for model_ws in &state.workspaces {
            let known = self
                .layout
                .app_workspace_id(model_ws.id)
                .and_then(|id| previous.remove(&id));
            let staged = self.layout.staged_workspaces.remove(&model_ws.id);
            let mut ws = match known.or(staged) {
                Some(ws) => ws,
                None => Workspace::empty_with_cwd_and_id(
                    crate::workspace::restored_workspace_id_in(
                        model_ws.id.0,
                        model_ws.space.as_ref(),
                    ),
                    model_ws.title.clone(),
                    std::path::PathBuf::from(&model_ws.cwd),
                ),
            };
            self.layout.workspace_ids.insert(ws.id, model_ws.id);
            self.layout.workspaces.insert(model_ws.id, ws.id);
            ws.title.clone_from(&model_ws.title);
            ws.cwd.clone_from(&model_ws.cwd);
            adopt_recorded_worktrees(&mut ws, &model_ws.extras.managed_worktrees);
            if ws.space != model_ws.space {
                ws.set_space(model_ws.space.clone());
            }
            if let Some(durable) = model_ws
                .extras
                .durable_id
                .as_ref()
                .filter(|durable| **durable != ws.durable_id)
            {
                ws.set_durable_id(durable.clone());
            }
            let app_ws = ws.id;
            let mut old_tabs: HashMap<u64, Tab> = ws
                .take_tabs()
                .into_iter()
                .map(|tab| (tab.id, tab))
                .collect();
            let mut tabs = Vec::with_capacity(model_ws.tabs.len());
            for model_tab in &model_ws.tabs {
                let known = self
                    .layout
                    .app_tab_id(model_tab.id)
                    .and_then(|id| old_tabs.remove(&id));
                let mut tab = known.unwrap_or_else(|| {
                    let mut tab = Tab::restored(
                        model_tab.title.clone(),
                        model_tab.title_source,
                        None,
                        model_tab.worktree.as_ref().map(std::path::PathBuf::from),
                    );
                    tab.id = model_tab.id.0;
                    tab
                });
                self.layout.tab_ids.insert(tab.id, model_tab.id);
                self.layout.tabs.insert(model_tab.id, tab.id);
                tab.project_title(&model_tab.title, model_tab.title_source);
                tab.bind_worktree(model_tab.worktree.as_ref().map(std::path::PathBuf::from));
                let (root, saved) = self.project_tab_tree(model_tab, app_ws, cx);
                tab.root = root;
                tab.saved_layout = saved;
                tabs.push(tab);
            }
            ws.project_tabs(tabs, model_ws.active_tab);
            projected.push(ws);
        }
        self.workspaces = projected;
        self.active_idx = state
            .active_workspace
            .min(self.workspaces.len().saturating_sub(1));
        self.active_space = state.active_space.clone();
    }

    fn project_tab_tree(
        &mut self,
        tab: &paneflow_layout::Tab,
        app_ws: u64,
        cx: &mut Context<Self>,
    ) -> (Option<LayoutTree>, Option<LayoutTree>) {
        let Some(root) = &tab.root else {
            return (None, None);
        };
        let full = self.project_node(root, app_ws, cx);
        let zoomed = tab.zoomed.and_then(|pane| self.layout.pane(pane));
        for pane in full.collect_leaves() {
            let wanted = zoomed.as_ref() == Some(&pane);
            if pane.read(cx).zoomed != wanted {
                pane.update(cx, |pane, _| pane.zoomed = wanted);
            }
        }
        match zoomed {
            Some(pane) => (Some(LayoutTree::Leaf(pane)), Some(full)),
            None => (Some(full), None),
        }
    }

    fn project_node(&mut self, node: &Node, app_ws: u64, cx: &mut Context<Self>) -> LayoutTree {
        match node {
            Node::Pane(model_pane) => {
                let surfaces: Vec<PaneSurface> = model_pane
                    .surfaces
                    .iter()
                    .filter_map(|surface| self.layout.surface(surface.id))
                    .collect();
                LayoutTree::Leaf(self.project_pane(model_pane, surfaces, app_ws, cx))
            }
            Node::Split(split) => {
                let cells = match self.layout.splits.get(&split.id) {
                    Some(cells) => cells.clone(),
                    None => {
                        let cells = SplitCells::new();
                        self.layout.split_ids.insert(cells.key(), split.id);
                        self.layout.splits.insert(split.id, cells.clone());
                        cells
                    }
                };
                let children = split
                    .children
                    .iter()
                    .map(|child| LayoutChild {
                        node: self.project_node(&child.node, app_ws, cx),
                        ratio: Rc::new(Cell::new(child.ratio)),
                    })
                    .collect();
                let direction = match split.direction {
                    Direction::Vertical => SplitDirection::Vertical,
                    Direction::Horizontal => SplitDirection::Horizontal,
                };
                LayoutTree::projected_split(direction, children, &cells)
            }
        }
    }

    fn project_pane(
        &mut self,
        model_pane: &paneflow_layout::Pane,
        surfaces: Vec<PaneSurface>,
        app_ws: u64,
        cx: &mut Context<Self>,
    ) -> Entity<Pane> {
        if let Some(pane) = self.layout.pane(model_pane.id) {
            let active = model_pane.active;
            pane.update(cx, |pane, cx| pane.project_surfaces(surfaces, active, cx));
            return pane;
        }
        let pane = self.create_pane_with_surfaces(surfaces, model_pane.active, app_ws, cx);
        self.layout.register_pane(model_pane.id, pane.clone());
        pane
    }
}

fn adopt_recorded_worktrees(
    ws: &mut Workspace,
    recorded: &[paneflow_config::schema::ManagedWorktreeDef],
) {
    for def in recorded {
        let path = std::path::PathBuf::from(&def.path);
        if path.as_os_str().is_empty() || ws.managed_worktrees.iter().any(|wt| wt.path == path) {
            continue;
        }
        ws.managed_worktrees
            .push(crate::workspace::worktree::ManagedWorktree {
                path,
                repo_root: std::path::PathBuf::from(&def.repo_root),
                branch: def.branch.clone(),
                teardown: crate::workspace::worktree::TeardownPolicy::from_record(&def.teardown),
            });
    }
}

#[cfg(test)]
impl Tab {
    pub(crate) fn zoomed_for_test(title: &str, shown: Entity<Pane>, full: LayoutTree) -> Self {
        let mut tab = Tab::new(title, Some(LayoutTree::Leaf(shown)));
        tab.saved_layout = Some(full);
        tab
    }
}
