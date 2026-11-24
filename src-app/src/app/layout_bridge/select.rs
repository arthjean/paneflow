use gpui::Context;
use paneflow_config::schema::TabTitleSource;
use paneflow_layout::Op;

use crate::PaneFlowApp;

impl PaneFlowApp {
    fn remember_shown_space(&mut self, target: usize) {
        let Some(target) = self.workspaces.get(target) else {
            return;
        };
        if target.space == self.active_space {
            return;
        }
        if let Some(shown) = self.shown_workspace_idx() {
            let id = self.workspaces[shown].id;
            self.space_last_shown.insert(self.active_space.clone(), id);
        }
    }

    pub(crate) fn select_workspace_through_model(
        &mut self,
        idx: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        let shown_before = self.shown_workspace_idx();
        self.remember_shown_space(idx);
        self.adopt_layout(cx);
        let Some(workspace) = self
            .workspaces
            .get(idx)
            .and_then(|ws| self.layout.workspace_id(ws.id))
        else {
            return false;
        };
        let pending = self.pending_pane_focus.clone();
        if let Err(error) = self.apply_layout_op(
            Op::SelectWorkspace {
                workspace,
                focus: false,
            },
            cx,
        ) {
            log::debug!("layout: workspace selection refused: {error}");
        }
        self.pending_pane_focus = pending;
        shown_before != Some(idx)
    }

    pub(crate) fn select_tab_through_model(
        &mut self,
        ws_idx: usize,
        tab_idx: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        self.remember_shown_space(ws_idx);
        self.adopt_layout(cx);
        let Some(tab) = self
            .workspaces
            .get(ws_idx)
            .and_then(|ws| ws.tabs().get(tab_idx))
            .and_then(|tab| self.layout.tab_id(tab.id))
        else {
            return false;
        };
        self.apply_layout_op(Op::SelectTab { tab }, cx).is_ok()
    }

    pub(crate) fn retitle_tab_through_model(
        &mut self,
        ws_idx: usize,
        tab_idx: usize,
        title: &str,
        source: TabTitleSource,
        cx: &mut Context<Self>,
    ) -> bool {
        let title = title.trim();
        let Some(current) = self
            .workspaces
            .get(ws_idx)
            .and_then(|ws| ws.tabs().get(tab_idx))
        else {
            return false;
        };
        if title.is_empty()
            || !current.title_source().yields_to(source)
            || (current.title() == title && current.title_source() == source)
        {
            return false;
        }
        let app_tab = current.id;
        self.adopt_layout(cx);
        let Some(tab) = self.layout.tab_id(app_tab) else {
            return false;
        };
        self.apply_layout_op(
            Op::RenameTab {
                tab,
                title: title.to_string(),
                source,
            },
            cx,
        )
        .is_ok()
    }

    pub(crate) fn unlock_tab_title_through_model(
        &mut self,
        ws_idx: usize,
        tab_idx: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(current) = self
            .workspaces
            .get(ws_idx)
            .and_then(|ws| ws.tabs().get(tab_idx))
        else {
            return false;
        };
        if current.title_source() == TabTitleSource::Preset {
            return false;
        }
        let app_tab = current.id;
        self.adopt_layout(cx);
        let Some(tab) = self.layout.tab_id(app_tab) else {
            return false;
        };
        self.apply_layout_op(Op::UnlockTabTitle { tab }, cx).is_ok()
    }

    pub(crate) fn select_surface_through_model(
        &mut self,
        pane: &gpui::Entity<crate::pane::Pane>,
        cx: &mut Context<Self>,
    ) {
        let Some(active) = pane
            .read(cx)
            .surfaces()
            .get(pane.read(cx).active_surface_idx())
            .map(super::surface_entity)
        else {
            return;
        };
        let Some(surface) = self.layout.surface_id_of(active) else {
            return;
        };
        let state = self.layout.model.state();
        let Some(location) = state.find_surface(surface) else {
            return;
        };
        if state
            .pane(location.pane)
            .is_some_and(|model| model.active == location.index)
        {
            return;
        }
        if let Err(error) = self.apply_layout_op(Op::SelectSurface { surface }, cx) {
            log::debug!("layout: the selected surface was refused: {error}");
        }
    }

    pub(crate) fn rename_surface_through_model(
        &mut self,
        surface: gpui::EntityId,
        name: Option<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        self.adopt_layout(cx);
        let Some(surface) = self.layout.surface_id_of(surface) else {
            return false;
        };
        self.apply_layout_op(Op::RenameSurface { surface, name }, cx)
            .is_ok()
    }
}
