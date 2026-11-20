use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{AppContext as _, Context, Entity};
use paneflow_config::schema::{SessionId, TerminalSurfaceProfile};
use paneflow_layout::{Applied, Effect, SurfaceId, WorkspaceId};

use crate::PaneFlowApp;
use crate::app::hosted_sessions::StopTarget;
use crate::pane::PaneSurface;
use crate::terminal::TerminalView;
use crate::workspace::SpawnCwd;

pub(crate) struct HeldSurface {
    pub(crate) target: StopTarget,
    pub(crate) cwd: Option<PathBuf>,
}

impl PaneFlowApp {
    fn surface_workspace(&self, surface: SurfaceId) -> Option<(WorkspaceId, u64)> {
        let location = self.layout.model.state().find_surface(surface)?;
        let app = self
            .layout
            .app_workspace_id(location.workspace)
            .or_else(|| {
                self.layout
                    .staged_workspaces
                    .get(&location.workspace)
                    .map(|ws| ws.id)
            })?;
        Some((location.workspace, app))
    }

    fn surface_worktree(&self, surface: SurfaceId) -> Option<PathBuf> {
        let state = self.layout.model.state();
        let location = state.find_surface(surface)?;
        let ws = state.workspaces.get(location.workspace_index)?;
        let tab = ws.tabs.get(location.tab_index)?;
        tab.worktree.as_ref().map(PathBuf::from)
    }

    pub(super) fn run_layout_effects(&mut self, applied: &Applied, cx: &mut Context<Self>) {
        for effect in &applied.effects {
            match effect {
                Effect::Spawn { surface } => self.spawn_layout_surface(*surface, cx),
                Effect::Reattach { surface, session } => {
                    self.reattach_layout_surface(*surface, session.clone(), cx);
                }
                Effect::Hold { session, until_ms } => {
                    self.hold_layout_session(session, *until_ms, cx);
                }
                Effect::Stop { session } => self.stop_layout_session(session, cx),
                Effect::Rehome { .. } => {}
            }
        }
    }

    pub(super) fn run_layout_rehomes(&mut self, applied: &Applied, cx: &mut Context<Self>) {
        let mut moves: Vec<(Entity<crate::pane::Pane>, usize)> = Vec::new();
        for effect in &applied.effects {
            let Effect::Rehome { session, workspace } = effect else {
                continue;
            };
            let Some(dest) = self.layout.model.state().workspace_index(*workspace) else {
                continue;
            };
            let Some(pane) = self.pane_holding_session(session, cx) else {
                continue;
            };
            if !moves.iter().any(|(known, _)| known == &pane) {
                moves.push((pane, dest));
            }
        }
        for (pane, dest) in moves {
            self.rehome_panes(std::slice::from_ref(&pane), dest, cx);
        }
    }

    fn pane_holding_session(
        &self,
        session: &SessionId,
        cx: &gpui::App,
    ) -> Option<Entity<crate::pane::Pane>> {
        let terminal = self.terminal_with_session(session, cx)?;
        crate::workspace::panes_across(&self.workspaces)
            .into_iter()
            .find(|pane| pane.read(cx).contains_terminal(&terminal))
    }

    fn terminal_with_session(
        &self,
        session: &SessionId,
        cx: &gpui::App,
    ) -> Option<Entity<TerminalView>> {
        self.layout
            .surfaces
            .values()
            .find_map(|surface| match surface {
                PaneSurface::Terminal(view) if view.read(cx).terminal.session_id == *session => {
                    Some(view.clone())
                }
                PaneSurface::Terminal(_) | PaneSurface::Markdown(_) => None,
            })
    }

    fn spawn_layout_surface(&mut self, surface: SurfaceId, cx: &mut Context<Self>) {
        if self.layout.surfaces.contains_key(&surface) {
            return;
        }
        let Some((_, app_ws)) = self.surface_workspace(surface) else {
            return;
        };
        let built = crate::layout_ids::assigning_surface(surface.0, || {
            match self.layout.staged_surfaces.remove(&surface) {
                Some(spawner) => spawner(self, app_ws, cx),
                None => self.default_layout_surface(surface, app_ws, cx),
            }
        });
        if let PaneSurface::Terminal(view) = &built {
            let session = view.read(cx).terminal.session_id.clone();
            self.layout.model.complete_spawn(surface, session);
        }
        self.layout.register_surface(surface, built);
    }

    fn default_layout_surface(
        &mut self,
        surface: SurfaceId,
        app_ws: u64,
        cx: &mut Context<Self>,
    ) -> PaneSurface {
        let definition = self
            .layout
            .model
            .state()
            .surface(surface)
            .map(|known| known.definition.clone())
            .unwrap_or_default();
        if definition.surface_type.as_deref() == Some("markdown")
            && let Some(path) = definition.path.clone()
        {
            let view = cx.new(|cx: &mut Context<crate::markdown::MarkdownView>| {
                crate::markdown::MarkdownView::open(PathBuf::from(path), cx)
            });
            return PaneSurface::Markdown(view);
        }
        let worktree = self.surface_worktree(surface);
        let cwd = definition.cwd.clone().map(PathBuf::from);
        let spawn = match (&worktree, cwd) {
            (Some(worktree), cwd) => SpawnCwd::within(Some(worktree), cwd),
            (None, Some(cwd)) => SpawnCwd {
                cwd: Some(cwd),
                ..SpawnCwd::default()
            },
            (None, None) => self.new_terminal_cwd(None),
        };
        let view = cx.new(|cx| {
            TerminalView::spawned(app_ws, spawn, None, TerminalSurfaceProfile::Normal, cx)
        });
        view.update(cx, |view, _| {
            view.terminal.custom_name = definition.custom_name.clone();
            view.terminal.font_size_override = definition.font_size;
        });
        super::subscribed_terminal(view, cx)
    }

    fn reattach_layout_surface(
        &mut self,
        surface: SurfaceId,
        session: SessionId,
        cx: &mut Context<Self>,
    ) {
        let Some((_, app_ws)) = self.surface_workspace(surface) else {
            return;
        };
        let definition = self
            .layout
            .model
            .state()
            .surface(surface)
            .map(|known| known.definition.clone())
            .unwrap_or_default();
        let held = self.layout.held.remove(&session);
        let cwd = held
            .as_ref()
            .and_then(|held| held.cwd.clone())
            .or_else(|| definition.cwd.clone().map(PathBuf::from));
        let view = crate::layout_ids::assigning_surface(surface.0, || {
            cx.new(|cx| TerminalView::attach_existing(app_ws, cwd, session.clone(), cx))
        });
        view.update(cx, |view, _| {
            view.terminal.custom_name = definition.custom_name.clone();
            view.terminal.font_size_override = definition.font_size;
        });
        cx.subscribe(&view, Self::handle_terminal_event).detach();
        let entity = view.read(cx).surface_id();
        self.layout
            .register_surface(surface, PaneSurface::Terminal(view));
        self.seed_surface_from_host(&session, app_ws, entity, cx);
    }

    fn hold_layout_session(&mut self, session: &SessionId, until_ms: u64, cx: &mut Context<Self>) {
        let Some(view) = self.terminal_with_session(session, cx) else {
            return;
        };
        let terminal = &view.read(cx).terminal;
        let Some(target) = terminal.hosted_stop_target() else {
            return;
        };
        let cwd = terminal
            .current_cwd
            .as_ref()
            .map(PathBuf::from)
            .or_else(|| terminal.cwd_now());
        self.layout
            .held
            .insert(session.clone(), HeldSurface { target, cwd });
        #[cfg(test)]
        crate::app::fake_host::record(
            cx,
            crate::app::fake_host::HostRequest::Hold {
                session: session.clone(),
            },
        );
        let until = self.layout.epoch + Duration::from_millis(until_ms);
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx: &mut gpui::AsyncApp| {
            loop {
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                executor.timer(left).await;
            }
            let _ = this.update(cx, |app, cx| app.expire_closed_sessions(cx));
        })
        .detach();
    }

    fn stop_layout_session(&mut self, session: &SessionId, cx: &mut Context<Self>) {
        if let Some(view) = self.terminal_with_session(session, cx) {
            self.layout.held.remove(session);
            self.stop_terminals(vec![view], cx);
            return;
        }
        if let Some(held) = self.layout.held.remove(session) {
            self.stop_hosted_sessions(vec![held.target], cx);
        }
    }

    pub(crate) fn expire_layout_holds(&mut self, now: Instant, cx: &mut Context<Self>) {
        let now_ms = self.layout.ms_at(now);
        let targets: Vec<StopTarget> = self
            .layout
            .model
            .expire_holds(now_ms)
            .into_iter()
            .filter_map(|effect| match effect {
                Effect::Stop { session } => self.layout.held.remove(&session),
                _ => None,
            })
            .map(|held| held.target)
            .collect();
        self.stop_hosted_sessions(targets, cx);
    }

    pub(crate) fn forget_layout_sessions(&mut self, sessions: &[SessionId]) {
        let sessions: std::collections::HashSet<SessionId> = sessions.iter().cloned().collect();
        self.layout.model.forget_sessions(&sessions);
        self.layout
            .held
            .retain(|session, _| !sessions.contains(session));
    }

    pub(crate) fn layout_held_sessions(&self) -> std::collections::HashSet<SessionId> {
        self.layout.held.keys().cloned().collect()
    }

    pub(crate) fn release_layout_holds(&mut self) -> Vec<StopTarget> {
        let now = self
            .layout
            .ms_at(Instant::now())
            .saturating_add(crate::CLOSED_SESSION_GRACE_MS.saturating_mul(2));
        let _ = self.layout.model.expire_holds(now);
        self.layout
            .held
            .drain()
            .map(|(_, held)| held.target)
            .collect()
    }
}
