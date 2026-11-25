use gpui::{Context, Entity};
use paneflow_config::schema::LayoutNode;
use paneflow_layout::{NewNode, Op, OpError, Preset, TabId};

use crate::PaneFlowApp;
use crate::layout::{LayoutTree, MAX_PANES};
use crate::pane::Pane;

type LaunchInput = (u64, Option<String>, Option<String>);

fn launch_inputs_of(node: &NewNode, found: &mut Vec<LaunchInput>) {
    match node {
        NewNode::Pane(pane) => {
            for surface in &pane.surfaces {
                let definition = &surface.definition;
                if definition
                    .surface_type
                    .as_deref()
                    .is_some_and(|kind| kind != "terminal")
                {
                    continue;
                }
                let command = definition.command.clone().filter(|text| !text.is_empty());
                let prompt = definition.prompt.clone().filter(|text| !text.is_empty());
                if command.is_some() || prompt.is_some() {
                    found.push((surface.id.0, command, prompt));
                }
            }
        }
        NewNode::Split { children, .. } => {
            for child in children {
                launch_inputs_of(child, found);
            }
        }
    }
}

impl PaneFlowApp {
    fn model_active_tab(&mut self, ws_idx: usize, cx: &mut Context<Self>) -> Option<TabId> {
        self.adopt_layout(cx);
        let tab = self.workspaces.get(ws_idx)?.active_tab().id;
        self.layout.tab_id(tab)
    }

    fn model_zoomed_pane(&self, tab: TabId) -> Option<Entity<Pane>> {
        let state = self.layout.model.state();
        let (ws, idx) = state.find_tab(tab)?;
        state.workspaces[ws].tabs[idx]
            .zoomed
            .and_then(|pane| self.layout.pane(pane))
    }

    pub(crate) fn zoom_pane_through_model(
        &mut self,
        ws_idx: usize,
        pane: &Entity<Pane>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(tab) = self.model_active_tab(ws_idx, cx) else {
            return false;
        };
        let Some(pane) = self.layout.pane_id(pane) else {
            return false;
        };
        self.apply_layout_op(
            Op::SetZoom {
                tab,
                pane: Some(pane),
            },
            cx,
        )
        .is_ok()
    }

    pub(crate) fn exit_zoom_through_model(
        &mut self,
        ws_idx: usize,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Pane>> {
        let tab = self.model_active_tab(ws_idx, cx)?;
        let zoomed = self.model_zoomed_pane(tab)?;
        self.apply_layout_op(Op::SetZoom { tab, pane: None }, cx)
            .ok()?;
        Some(zoomed)
    }

    pub(crate) fn divider_drag_ended(&mut self, split: usize, cx: &mut Context<Self>) {
        let Some(id) = self.layout.split_ids.get(&split).copied() else {
            return;
        };
        let ratios = self.workspaces.iter().find_map(|ws| {
            ws.tabs().iter().find_map(|tab| {
                [tab.root.as_ref(), tab.saved_layout.as_ref()]
                    .into_iter()
                    .flatten()
                    .find_map(|root| root.split_ratios(split))
            })
        });
        let Some(ratios) = ratios else {
            return;
        };
        self.layout.resizing = Some(split);
        let applied = self.apply_layout_op(Op::SetRatios { split: id, ratios }, cx);
        self.layout.resizing = None;
        if let Err(error) = applied {
            log::debug!("layout: a divider drag was refused: {error}");
            self.project_layout(cx);
            cx.notify();
        }
    }

    pub(crate) fn cancel_divider_drags(&mut self, cx: &mut Context<Self>) {
        let dragging = self.workspaces.iter().any(|ws| {
            ws.tabs().iter().any(|tab| {
                [tab.root.as_ref(), tab.saved_layout.as_ref()]
                    .into_iter()
                    .flatten()
                    .any(LayoutTree::cancel_resize)
            })
        });
        if dragging {
            self.project_layout(cx);
        }
    }

    pub(crate) fn exit_pane_zoom_through_model(
        &mut self,
        pane: &Entity<Pane>,
        cx: &mut Context<Self>,
    ) {
        self.adopt_layout(cx);
        let Some(id) = self.layout.pane_id(pane) else {
            return;
        };
        let state = self.layout.model.state();
        let Some((ws, idx)) = state.find_pane(id) else {
            return;
        };
        let tab = &state.workspaces[ws].tabs[idx];
        if tab.zoomed.is_none() {
            return;
        }
        let tab = tab.id;
        let _ = self.apply_layout_op(Op::SetZoom { tab, pane: None }, cx);
    }

    pub(crate) fn reveal_surface_through_model(
        &mut self,
        surface_id: u64,
        cx: &mut Context<Self>,
    ) -> Option<crate::app::ipc_handler::SurfaceLocation> {
        let loc = self.locate_surface(surface_id)?;
        self.adopt_layout(cx);
        let pane_id = self.layout.pane_id(&loc.pane)?;
        let state = self.layout.model.state();
        let (ws, tab) = state.find_pane(pane_id)?;
        let tab = &state.workspaces[ws].tabs[tab];
        let (tab, hidden) = (tab.id, tab.zoomed.is_some_and(|zoomed| zoomed != pane_id));
        let app_tab = self.layout.app_tab_id(tab)?;
        let tab_idx = self.workspaces[loc.workspace_idx]
            .tabs()
            .iter()
            .position(|t| t.id == app_tab)?;
        let pending = self.pending_pane_focus.clone();
        if self.workspaces[loc.workspace_idx].active_tab_idx() != tab_idx {
            self.select_tab_through_model(loc.workspace_idx, tab_idx, cx);
        }
        if hidden {
            let _ = self.apply_layout_op(Op::SetZoom { tab, pane: None }, cx);
        }
        self.pending_pane_focus = pending;
        self.locate_surface(surface_id)
    }

    pub(crate) fn reveal_pane_through_model(
        &mut self,
        ws_idx: usize,
        pane: &Entity<Pane>,
        cx: &mut Context<Self>,
    ) -> bool {
        let holding_tab = self.workspaces.get(ws_idx).and_then(|ws| {
            ws.tabs()
                .iter()
                .position(|tab| {
                    tab.root
                        .as_ref()
                        .is_some_and(|root| root.contains_leaf(pane))
                })
                .filter(|&tab_idx| tab_idx != ws.active_tab_idx())
        });
        if let Some(tab_idx) = holding_tab {
            let pending = self.pending_pane_focus.clone();
            self.select_tab_through_model(ws_idx, tab_idx, cx);
            self.pending_pane_focus = pending;
        }
        let hidden = self.workspaces.get(ws_idx).is_some_and(|ws| {
            let tab = ws.active_tab();
            tab.is_zoomed()
                && !tab
                    .root
                    .as_ref()
                    .is_some_and(|root| root.contains_leaf(pane))
        });
        if hidden {
            self.exit_zoom_through_model(ws_idx, cx);
        }
        hidden
    }

    pub(crate) fn apply_preset_through_model(
        &mut self,
        preset: Preset,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(ws_idx) = self.shown_workspace_idx() else {
            return false;
        };
        let Some(tab) = self.model_active_tab(ws_idx, cx) else {
            return false;
        };
        self.apply_layout_op(Op::ApplyPreset { tab, preset }, cx)
            .is_ok()
    }

    pub(crate) fn equalize_through_model(&mut self, cx: &mut Context<Self>) {
        let Some(ws_idx) = self.shown_workspace_idx() else {
            return;
        };
        if let Some(tab) = self.model_active_tab(ws_idx, cx) {
            let _ = self.apply_layout_op(Op::Equalize { tab }, cx);
        }
    }

    pub(crate) fn restore_layout_through_model(
        &mut self,
        ws_idx: usize,
        layout: &LayoutNode,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let needed = layout.leaf_count();
        if needed == 0 {
            return Err("Layout has no panes".into());
        }
        if needed > MAX_PANES {
            return Err(format!("Layout exceeds maximum pane count ({MAX_PANES})"));
        }
        let Some(tab) = self.model_active_tab(ws_idx, cx) else {
            return Err("No active workspace".into());
        };
        let fallback = std::path::PathBuf::from(&self.workspaces[ws_idx].cwd);
        let node = self.layout.model.reserve_node(layout);
        self.stage_restored_surfaces(&node, &fallback);
        let mut launches = Vec::new();
        launch_inputs_of(&node, &mut launches);
        self.apply_layout_op(Op::RestoreLayout { tab, layout: node }, cx)
            .map_err(|error| match error {
                OpError::PaneLimit => format!("Layout exceeds maximum pane count ({MAX_PANES})"),
                _ => "No active workspace".into(),
            })?;
        for (surface, command, prompt) in launches {
            let Some(terminal) = self.terminal_for_surface(surface) else {
                continue;
            };
            match (command, prompt) {
                (Some(command), prompt) => {
                    Self::schedule_launch_command(&terminal, command, prompt, usize::MAX, cx);
                }
                (None, Some(prompt)) => {
                    Self::schedule_prompt_prefill(&terminal, prompt, usize::MAX, cx);
                }
                (None, None) => {}
            }
        }
        Ok(())
    }

    fn stage_restored_surfaces(&mut self, node: &NewNode, fallback: &std::path::Path) {
        match node {
            NewNode::Pane(pane) => {
                for surface in &pane.surfaces {
                    let definition = surface.definition.clone();
                    let fallback = fallback.to_path_buf();
                    self.layout.staged_surfaces.insert(
                        surface.id,
                        Box::new(move |_, ws_id, cx| {
                            match Self::build_restored_surface(ws_id, &definition, &fallback, cx) {
                                Some(surface) => surface,
                                None => {
                                    let view = gpui::AppContext::new(cx, |cx| {
                                        crate::terminal::TerminalView::with_cwd(
                                            ws_id,
                                            Some(fallback),
                                            None,
                                            cx,
                                        )
                                    });
                                    super::subscribed_terminal(view, cx)
                                }
                            }
                        }),
                    );
                }
            }
            NewNode::Split { children, .. } => {
                for child in children {
                    self.stage_restored_surfaces(child, fallback);
                }
            }
        }
    }

    pub(crate) fn launch_template_through_model(
        &mut self,
        ws_idx: usize,
        tree: &LayoutTree,
        focus: Option<&Entity<Pane>>,
        cwd: Option<String>,
        cx: &mut Context<Self>,
    ) -> Result<(), OpError> {
        self.adopt_layout(cx);
        let workspace = self
            .workspaces
            .get(ws_idx)
            .and_then(|ws| self.layout.workspace_id(ws.id))
            .ok_or(OpError::NotFound)?;
        let root = self.layout.stage_tree(tree, cx);
        let focus = focus.and_then(|pane| self.layout.pane_id(pane));
        self.apply_layout_op(
            Op::LaunchTemplate {
                workspace,
                root,
                focus,
                cwd,
            },
            cx,
        )
        .map(|_| ())
    }
}
