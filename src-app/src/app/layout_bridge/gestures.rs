use gpui::{App, AppContext as _, Context, Entity, Focusable as _, Window};
use paneflow_config::schema::{SurfaceDefinition, TerminalSurfaceProfile};
use paneflow_layout::{Direction, Edge, Op, OpError, TabId};

use crate::PaneFlowApp;
use crate::app::workspace_ops::SurfaceLaunch;
use crate::layout::{MAX_PANES, SplitDirection};
use crate::pane::{Pane, PaneSurface};
use crate::terminal::TerminalView;

pub(crate) fn model_direction(direction: SplitDirection) -> Direction {
    match direction {
        SplitDirection::Vertical => Direction::Vertical,
        SplitDirection::Horizontal => Direction::Horizontal,
    }
}

pub(crate) fn terminal_definition() -> SurfaceDefinition {
    SurfaceDefinition {
        surface_type: Some("terminal".to_string()),
        ..SurfaceDefinition::default()
    }
}

impl PaneFlowApp {
    pub(crate) fn settle_layout_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pane) = self.pending_pane_focus.take()
            && !Self::focus_pane_window(pane.clone(), cx)
        {
            pane.read(cx).focus_handle(cx).focus(window, cx);
        }
    }

    pub(crate) fn note_layout_focus(&mut self, window: &Window, cx: &App) {
        let Some(focused) = self
            .active_workspace()
            .and_then(|ws| ws.active_tab().root.as_ref())
            .and_then(|root| root.focused_pane(window, cx))
        else {
            return;
        };
        let Some(id) = self.layout.pane_id(&focused) else {
            return;
        };
        let state = self.layout.model.state();
        let Some((ws, tab)) = state.find_pane(id) else {
            return;
        };
        if state.workspaces[ws].tabs[tab].focused == Some(id) {
            return;
        }
        let now_ms = self.layout.ms_at(std::time::Instant::now());
        if let Err(error) = self
            .layout
            .apply_recorded(Op::FocusPane { pane: id }, now_ms)
        {
            log::debug!("layout: the focused pane was refused: {error}");
        }
    }

    pub(crate) fn split_pane_through_model(
        &mut self,
        target: &Entity<Pane>,
        direction: SplitDirection,
        profile: TerminalSurfaceProfile,
        launch: SurfaceLaunch,
        cx: &mut Context<Self>,
    ) -> Result<Entity<TerminalView>, String> {
        self.adopt_layout(cx);
        let Some(target_id) = self.layout.pane_id(target) else {
            return Err("That pane no longer exists".to_string());
        };
        let source_cwd = target
            .read(cx)
            .active_terminal_opt()
            .and_then(|tv| tv.read(cx).terminal.cwd_now());
        let spawn = self.new_terminal_cwd(source_cwd);
        let env = launch.env;
        let pane = self.layout.reserve_pane(
            terminal_definition(),
            Box::new(move |_, ws_id, cx| {
                super::subscribed_terminal(
                    cx.new(|cx| TerminalView::spawned(ws_id, spawn, env, profile, cx)),
                    cx,
                )
            }),
        );
        let surface = pane.surfaces[0].id;
        let applied = self.apply_layout_op(
            Op::Split {
                target: target_id,
                direction: model_direction(direction),
                edge: Edge::After,
                pane,
                focus: true,
            },
            cx,
        );
        if let Err(error) = applied {
            return Err(match error {
                OpError::Zoomed => "Unzoom before splitting panes".to_string(),
                OpError::PaneLimit | OpError::IdsExhausted => {
                    format!("Maximum pane count reached ({MAX_PANES})")
                }
                _ => "That pane no longer exists".to_string(),
            });
        }
        let Some(PaneSurface::Terminal(terminal)) = self.layout.surface(surface) else {
            return Err("That pane no longer exists".to_string());
        };
        if let Some(command) = launch.command.as_deref() {
            terminal.read(cx).send_command(command);
            terminal.update(cx, |view, _cx| view.declare_agent_from_command(command));
        }
        Ok(terminal)
    }

    pub(crate) fn close_pane_through_model(
        &mut self,
        pane: &Entity<Pane>,
        keep_undo: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        self.adopt_layout(cx);
        let Some(pane) = self.layout.pane_id(pane) else {
            return false;
        };
        self.apply_layout_op(Op::ClosePane { pane, keep_undo }, cx)
            .is_ok()
    }

    pub(crate) fn bind_tab_worktree_through_model(
        &mut self,
        app_tab: u64,
        worktree: Option<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) -> bool {
        self.adopt_layout(cx);
        let Some(tab) = self.layout.tab_id(app_tab) else {
            return false;
        };
        self.apply_layout_op(
            Op::BindWorktree {
                tab,
                worktree: worktree.map(|path| path.to_string_lossy().into_owned()),
            },
            cx,
        )
        .is_ok()
    }

    pub(crate) fn reserve_layout_id(&mut self) -> u64 {
        self.layout.model.reserve()
    }

    pub(crate) fn layout_app_tab_id(&self, tab: TabId) -> Option<u64> {
        self.layout.app_tab_id(tab)
    }

    pub(crate) fn split_with_pane_through_model(
        &mut self,
        target: &Entity<Pane>,
        edge: crate::pane_drag::DropEdge,
        new_pane: &Entity<Pane>,
        cx: &mut Context<Self>,
    ) -> bool {
        self.adopt_layout(cx);
        let Some(target) = self.layout.pane_id(target) else {
            return false;
        };
        let (direction, before) = edge.to_split();
        let pane = match self
            .layout
            .stage_tree(&crate::layout::LayoutTree::Leaf(new_pane.clone()), cx)
        {
            paneflow_layout::NewNode::Pane(pane) => pane,
            paneflow_layout::NewNode::Split { .. } => return false,
        };
        self.apply_layout_op(
            Op::Split {
                target,
                direction: model_direction(direction),
                edge: if before { Edge::Before } else { Edge::After },
                pane,
                focus: true,
            },
            cx,
        )
        .is_ok()
    }

    pub(crate) fn move_pane_through_model(
        &mut self,
        source: &Entity<Pane>,
        target: &Entity<Pane>,
        edge: Option<crate::pane_drag::DropEdge>,
        cx: &mut Context<Self>,
    ) -> bool {
        self.adopt_layout(cx);
        let (Some(source), Some(target)) =
            (self.layout.pane_id(source), self.layout.pane_id(target))
        else {
            return false;
        };
        let op = match edge {
            None => Op::SwapPanes {
                a: source,
                b: target,
            },
            Some(edge) => {
                let (direction, before) = edge.to_split();
                Op::MovePane {
                    pane: source,
                    target,
                    direction: model_direction(direction),
                    edge: if before { Edge::Before } else { Edge::After },
                }
            }
        };
        self.apply_layout_op(op, cx).is_ok()
    }

    pub(crate) fn move_surface_through_model(
        &mut self,
        surface: &PaneSurface,
        target: &Entity<Pane>,
        edge: Option<crate::pane_drag::DropEdge>,
        cx: &mut Context<Self>,
    ) {
        self.adopt_layout(cx);
        let (Some(surface), Some(target)) = (
            self.layout.surface_id_of(super::surface_entity(surface)),
            self.layout.pane_id(target),
        ) else {
            return;
        };
        let split = edge.map(|edge| {
            let (direction, before) = edge.to_split();
            (
                model_direction(direction),
                if before { Edge::Before } else { Edge::After },
                paneflow_layout::PaneId(self.layout.model.reserve()),
            )
        });
        match self.apply_layout_op(
            Op::MoveSurface {
                surface,
                target,
                split,
            },
            cx,
        ) {
            Err(OpError::PaneLimit) => {
                self.show_toast(format!("Maximum pane count reached ({MAX_PANES})"), cx);
            }
            Err(OpError::SurfaceLimit) => {
                self.show_toast(
                    format!("Maximum tab count reached ({})", crate::pane::MAX_PANE_TABS),
                    cx,
                );
            }
            _ => {}
        }
    }

    pub(crate) fn add_surface_through_model(
        &mut self,
        pane: &Entity<Pane>,
        spawner: super::Spawner,
        cx: &mut Context<Self>,
    ) -> Option<PaneSurface> {
        self.adopt_layout(cx);
        let pane = self.layout.pane_id(pane)?;
        let surface = self.layout.reserve_surface(terminal_definition(), spawner);
        let id = surface.id;
        self.apply_layout_op(
            Op::AddSurface {
                pane,
                surface: Box::new(surface),
            },
            cx,
        )
        .ok()?;
        self.layout.surface(id)
    }

    pub(crate) fn close_surface_through_model(
        &mut self,
        pane: &Entity<Pane>,
        terminal: &Entity<TerminalView>,
        cx: &mut Context<Self>,
    ) {
        self.adopt_layout(cx);
        if self
            .layout
            .pane_id(pane)
            .is_none_or(|pane| self.layout.model.state().pane(pane).is_none())
        {
            return;
        }
        self.close_surface_entity_through_model(terminal.entity_id(), cx);
    }

    pub(crate) fn close_surface_entity_through_model(
        &mut self,
        entity: gpui::EntityId,
        cx: &mut Context<Self>,
    ) {
        self.adopt_layout(cx);
        let Some(surface) = self.layout.surface_id_of(entity) else {
            return;
        };
        let _ = self.apply_layout_op(Op::CloseSurface { surface }, cx);
    }

    pub(crate) fn close_workspace_through_model(
        &mut self,
        idx: usize,
        focus: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        self.adopt_layout(cx);
        let Some(workspace) = self
            .workspaces
            .get(idx)
            .and_then(|ws| self.layout.workspace_id(ws.id))
        else {
            return false;
        };
        self.apply_layout_op(Op::CloseWorkspace { workspace, focus }, cx)
            .is_ok()
    }

    pub(crate) fn layout_workspace_id(&self, app_id: u64) -> Option<paneflow_layout::WorkspaceId> {
        self.layout.workspace_id(app_id)
    }

    pub(crate) fn layout_tab_id(&self, app_id: u64) -> Option<TabId> {
        self.layout.tab_id(app_id)
    }

    pub(crate) fn close_tab_through_model(
        &mut self,
        ws_idx: usize,
        tab_idx: usize,
        keep_sessions: bool,
        cx: &mut Context<Self>,
    ) -> Option<TabId> {
        self.adopt_layout(cx);
        let app_tab = self.workspaces.get(ws_idx)?.tabs().get(tab_idx)?.id;
        let tab = self.layout.tab_id(app_tab)?;
        let had_layout = {
            let state = self.layout.model.state();
            state
                .find_tab(tab)
                .is_some_and(|(ws, idx)| state.workspaces[ws].tabs[idx].root.is_some())
        };
        self.apply_layout_op(Op::CloseTab { tab, keep_sessions }, cx)
            .ok()?;
        had_layout.then_some(tab)
    }

    fn undo_sessions_shown_elsewhere(&self, tab: Option<TabId>) -> bool {
        let history = self.layout.model.undo_history();
        let record = match tab {
            Some(tab) => history.records().find(|record| {
                matches!(record, paneflow_layout::ClosedRecord::Tab(closed) if closed.tab == tab)
            }),
            None => history.newest(),
        };
        let Some(record) = record else {
            return false;
        };
        let shown: std::collections::HashSet<_> = self
            .layout
            .model
            .state()
            .workspaces
            .iter()
            .flat_map(|ws| ws.tabs.iter())
            .filter_map(|tab| tab.root.as_ref())
            .flat_map(|root| root.panes())
            .flat_map(|pane| pane.surfaces.iter().filter_map(|s| s.session().cloned()))
            .collect();
        record
            .sessions()
            .iter()
            .any(|session| history.holds(session) && shown.contains(session))
    }

    fn recheck_restored_tab_worktree(
        ws_id: u64,
        tab_id: u64,
        path: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx: &mut gpui::AsyncApp| {
            let probe = path.clone();
            let exists = smol::unblock(move || probe.is_dir()).await;
            let _ = this.update(cx, |app, cx| {
                let Some((ws_idx, tab_idx)) = app.tab_position(ws_id, tab_id) else {
                    return;
                };
                if exists {
                    Self::spawn_initial_git_stats(ws_id, path.to_string_lossy().into_owned(), cx);
                } else {
                    app.set_tab_worktree(ws_idx, tab_idx, None, cx);
                }
            });
        })
        .detach();
    }

    pub(crate) fn undo_close_through_model(
        &mut self,
        tab: Option<TabId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let reopening_tab = match tab {
            Some(tab) => Some(tab),
            None => match self.layout.model.undo_history().newest() {
                Some(paneflow_layout::ClosedRecord::Tab(record)) => Some(record.tab),
                Some(paneflow_layout::ClosedRecord::Pane(_)) => None,
                None => {
                    self.show_toast("No closed pane to restore", cx);
                    return;
                }
            },
        };
        let shown_elsewhere = self.undo_sessions_shown_elsewhere(tab);
        match self.apply_layout_op(Op::UndoClose { tab }, cx) {
            Ok(_) => {
                if let Some(tab) = reopening_tab
                    && let Some(app_tab) = self.layout.app_tab_id(tab)
                    && let Some(ws_idx) = self
                        .workspaces
                        .iter()
                        .position(|ws| ws.tabs().iter().any(|known| known.id == app_tab))
                {
                    self.workspaces[ws_idx].sidebar_expanded = true;
                    let ws_id = self.workspaces[ws_idx].id;
                    if let Some(path) = self.workspaces[ws_idx]
                        .tabs()
                        .iter()
                        .find(|known| known.id == app_tab)
                        .and_then(|known| known.worktree.clone())
                    {
                        Self::recheck_restored_tab_worktree(ws_id, app_tab, path, cx);
                    }
                }
                if shown_elsewhere {
                    self.show_toast(
                        "That session is already open in another view, so a new shell opened instead",
                        cx,
                    );
                }
                self.settle_layout_focus(window, cx);
            }
            Err(error) => {
                let message = match (reopening_tab, error) {
                    (None, OpError::Zoomed) => "Unzoom before restoring a closed pane".to_string(),
                    (None, OpError::PaneLimit) => {
                        format!("Maximum pane count reached ({MAX_PANES})")
                    }
                    (None, _) => "No active workspace to restore pane".to_string(),
                    (Some(_), OpError::TabLimit) => {
                        "Tab limit reached for this workspace".to_string()
                    }
                    (Some(_), OpError::NotFound) if tab.is_some() => {
                        "Could not reopen the tab, it left the undo history".to_string()
                    }
                    (Some(_), _) => "No workspace to reopen the tab in".to_string(),
                };
                self.show_toast(message, cx);
            }
        }
    }
}
