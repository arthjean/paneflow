use std::collections::HashSet;

use gpui::{AppContext, Context, Entity, EntityId, Window};
use paneflow_browser_proto::TabId;

use super::diff_dock::DiffDockTab;
use crate::app::menu_icons as icon;
use crate::ui_primitives::action_menu::{ActionMenu, MenuItem};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LinkMenu {
    pub(crate) url: String,
    pub(crate) position: gpui::Point<gpui::Pixels>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinkAction {
    OpenExternal,
    OpenInBrowser,
    Copy,
}
use crate::PaneFlowApp;
use crate::browser::{AgentChoice, BrowserView, BrowserViewEvent, DevServer};
use crate::pane::{Pane, PaneSurface};

impl PaneFlowApp {
    pub(crate) fn open_local_link(&mut self, url: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| {
            let installed = smol::unblock(crate::browser_control::installed).await;
            let _ = this.update(cx, |app, cx| {
                if installed {
                    app.open_diff_browser_url(url, cx);
                } else {
                    app.open_external_url(url, cx);
                }
            });
        })
        .detach();
    }

    pub(crate) fn render_terminal_link_menu(
        &self,
        menu: LinkMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let actions = ActionMenu::new()
            .item(MenuItem::new("Open link", LinkAction::OpenExternal).icon(icon::LINK))
            .item(
                MenuItem::new("Open in Paneflow browser", LinkAction::OpenInBrowser)
                    .icon(icon::GLOBE),
            )
            .item(MenuItem::new("Copy link", LinkAction::Copy).icon(icon::COPY));
        let url = menu.url.clone();
        let handlers = self.menu_handlers(
            cx,
            move |this, action: &LinkAction, _, cx| {
                this.terminal_link_menu = None;
                match action {
                    LinkAction::OpenExternal => this.open_external_url(url.clone(), cx),
                    LinkAction::OpenInBrowser => this.open_diff_browser_url(url.clone(), cx),
                    LinkAction::Copy => {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(url.clone()));
                        this.show_toast("Copied link", cx);
                    }
                }
            },
            |this| this.terminal_link_menu = None,
        );
        crate::app::sidebar::context_menu::render_context_menu(
            "terminal-link-menu",
            actions,
            menu.position,
            handlers,
            window,
            cx,
        )
    }

    pub(crate) fn new_browser_view(
        &mut self,
        build: impl FnOnce(&mut Context<BrowserView>) -> BrowserView,
        cx: &mut Context<Self>,
    ) -> Entity<BrowserView> {
        let view = cx.new(build);
        cx.subscribe(&view, Self::handle_browser_view_event)
            .detach();
        crate::browser::track(&view, cx);
        view
    }

    fn handle_browser_view_event(
        &mut self,
        view: Entity<BrowserView>,
        event: &BrowserViewEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            BrowserViewEvent::Changed => cx.notify(),
            BrowserViewEvent::Persist => self.save_session(cx),
            BrowserViewEvent::Opened(tab) => self.open_browser_beside(&view, *tab, cx),
            BrowserViewEvent::Show(tab) => self.show_browser_tab(&view, *tab, cx),
            BrowserViewEvent::WantsAgents => {
                let workspace_id = view.read(cx).workspace_id;
                let agents = self.browser_agents(workspace_id, cx);
                let targets = self
                    .workspaces
                    .iter()
                    .position(|ws| ws.id == workspace_id)
                    .map(|index| {
                        crate::app::agent_send::workspace_agent_targets(&self.workspaces, index, cx)
                    })
                    .unwrap_or_default();
                view.update(cx, |view, cx| {
                    view.set_agents(agents, cx);
                    view.set_targets(targets, cx);
                });
            }
            BrowserViewEvent::SendAnnotations {
                surface_id,
                text,
                window,
            } => {
                let (surface_id, text, window) = (*surface_id, text.clone(), *window);
                let host = cx.weak_entity();
                cx.defer(move |cx| {
                    let _ = window.update(cx, |_, window, cx| {
                        let _ = host.update(cx, |app, cx| {
                            app.prefill_agent(surface_id, &text, None, window, cx);
                        });
                    });
                });
            }
            BrowserViewEvent::OpenSettings(window) => {
                let window = *window;
                let host = cx.weak_entity();
                cx.defer(move |cx| {
                    let _ = window.update(cx, |_, window, cx| {
                        let _ = host.update(cx, |app, cx| {
                            app.open_settings_at(crate::SettingsSection::Browser, window, cx);
                        });
                    });
                });
            }
            BrowserViewEvent::WantsDevServers => {
                let workspace_id = view.read(cx).workspace_id;
                let servers = self.browser_dev_servers(workspace_id);
                view.update(cx, |view, cx| view.set_dev_servers(servers, cx));
            }
        }
    }

    fn browser_dev_servers(&self, workspace_id: u64) -> Vec<DevServer> {
        let mut ordered: Vec<&crate::workspace::Workspace> = self.workspaces.iter().collect();
        ordered.sort_by_key(|ws| ws.id != workspace_id);
        let mut servers: Vec<DevServer> = Vec::new();
        for ws in ordered {
            for port in &ws.active_ports {
                if servers.iter().any(|server| server.port == *port) {
                    continue;
                }
                let label = ws
                    .service_labels
                    .get(port)
                    .and_then(|info| info.label.clone())
                    .filter(|label| !label.trim().is_empty());
                servers.push(DevServer { port: *port, label });
            }
        }
        servers
    }

    pub(crate) fn persist_browser(
        &mut self,
        edit: impl FnOnce(&mut paneflow_config::schema::BrowserConfig),
        cx: &mut Context<Self>,
    ) {
        let mut browser = self.cached_config.browser.clone();
        edit(&mut browser);
        let Ok(value) = serde_json::to_value(browser) else {
            return;
        };
        self.persist_setting_or_revert(false, "browser", value, cx);
    }

    fn browser_agents(&self, workspace_id: u64, cx: &gpui::App) -> Vec<AgentChoice> {
        let Some(index) = self.workspaces.iter().position(|ws| ws.id == workspace_id) else {
            return Vec::new();
        };
        crate::app::ipc_handler::workspace_agent_sessions(&self.workspaces, index, cx)
            .into_iter()
            .map(|(session, label)| AgentChoice { session, label })
            .collect()
    }

    pub(crate) fn displayed_browser_views(&self, cx: &gpui::App) -> HashSet<EntityId> {
        let mut shown = HashSet::new();
        if self.settings_section.is_some() {
            return shown;
        }
        if let Some(root) = self
            .active_workspace()
            .and_then(|ws| ws.active_tab().root.as_ref())
        {
            for pane in root.collect_leaves() {
                if let PaneSurface::Browser(view) = pane.read(cx).surface() {
                    shown.insert(view.entity_id());
                }
            }
        }
        if self.diff_dock_visible()
            && !self.diff_dock.picker
            && let Some(DiffDockTab::Browser(view)) =
                self.diff_dock.diff_tabs.get(self.diff_dock.diff_active_tab)
        {
            shown.insert(view.entity_id());
        }
        shown
    }

    pub(crate) fn sync_browser_display(&self, cx: &mut gpui::App) {
        crate::browser::sync_displayed(self.displayed_browser_views(cx), cx);
    }

    pub(crate) fn open_diff_browser_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace_id) = self.active_workspace().map(|ws| ws.id) else {
            return;
        };
        let view = self.new_browser_view(|cx| BrowserView::new(workspace_id, cx), cx);
        self.diff_dock
            .diff_tabs
            .push(DiffDockTab::Browser(view.clone()));
        self.select_diff_tab(self.diff_dock.diff_tabs.len() - 1, cx);
        let focus = view.read(cx).address_focus(cx);
        window.focus(&focus, cx);
    }

    pub(crate) fn open_diff_browser_url(&mut self, url: String, cx: &mut Context<Self>) {
        let Some(workspace) = self.active_workspace() else {
            return;
        };
        let workspace_id = workspace.id;
        let cwd = self
            .active_checkout()
            .unwrap_or_else(|| workspace.cwd.clone());
        let view = self.new_browser_view(|cx| BrowserView::new(workspace_id, cx), cx);
        view.update(cx, |view, cx| view.open(url, cx));
        self.diff_dock.diff_tabs.push(DiffDockTab::Browser(view));
        self.show_diff_tab_with_cwd(self.diff_dock.diff_tabs.len() - 1, cwd, cx);
    }

    fn show_diff_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(workspace) = self.active_workspace() else {
            self.select_diff_tab(index, cx);
            return;
        };
        let cwd = self
            .active_checkout()
            .unwrap_or_else(|| workspace.cwd.clone());
        self.show_diff_tab_with_cwd(index, cwd, cx);
    }

    fn show_diff_tab_with_cwd(&mut self, index: usize, cwd: String, cx: &mut Context<Self>) {
        self.select_diff_tab(index, cx);
        if !self.diff_dock.open {
            self.open_diff_dock_panel(cwd, cx);
        }
    }

    pub(crate) fn browser_pane_surface(
        &mut self,
        workspace_id: u64,
        cx: &mut Context<Self>,
    ) -> PaneSurface {
        PaneSurface::Browser(self.new_browser_view(|cx| BrowserView::new(workspace_id, cx), cx))
    }

    pub(crate) fn restored_browser_surface(
        workspace_id: u64,
        definition: &paneflow_config::schema::SurfaceDefinition,
        cx: &mut Context<Self>,
    ) -> Option<PaneSurface> {
        let url = match definition.path.clone() {
            Some(url) if crate::browser::restorable(&url) => Some(url),
            Some(_) => return None,
            None => None,
        };
        let title = definition.name.clone();
        let view = cx.new(|cx| match url {
            Some(url) => BrowserView::restored(workspace_id, url, title, cx),
            None => BrowserView::new(workspace_id, cx),
        });
        cx.subscribe(&view, Self::handle_browser_view_event)
            .detach();
        crate::browser::track(&view, cx);
        Some(PaneSurface::Browser(view))
    }

    pub(crate) fn add_browser_surface_to_pane(
        &mut self,
        pane: &Entity<Pane>,
        cx: &mut Context<Self>,
    ) {
        let added = self.add_surface_through_model_as(
            pane,
            crate::browser::empty_surface_definition(),
            Box::new(|app, ws_id, cx| app.browser_pane_surface(ws_id, cx)),
            cx,
        );
        if !matches!(added, Some(PaneSurface::Browser(_))) {
            self.show_toast(
                format!("Maximum tab count reached ({})", crate::pane::MAX_PANE_TABS),
                cx,
            );
        }
    }

    pub(crate) fn open_tab_with_browser(
        &mut self,
        ws_idx: usize,
        split: Option<(Entity<Pane>, crate::layout::SplitDirection)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let Some(ws) = self.workspaces.get(ws_idx) else {
            return Err("This project is no longer open".to_string());
        };
        let ws_id = ws.id;
        let fills_active_tab =
            ws.active_tab().root.is_none() && ws.active_tab().saved_layout.is_none();
        let surface = self.browser_pane_surface(ws_id, cx);
        let PaneSurface::Browser(view) = surface.clone() else {
            return Err("The browser surface could not be created".to_string());
        };
        let pane = self.create_pane_with_existing_surface(surface, ws_id, cx);
        match split {
            Some((target, direction)) => {
                let edge = match direction {
                    crate::layout::SplitDirection::Horizontal => crate::pane_drag::DropEdge::Down,
                    crate::layout::SplitDirection::Vertical => crate::pane_drag::DropEdge::Right,
                };
                if !self.split_with_pane_through_model(&target, edge, &pane, cx) {
                    return Err("That pane cannot be split".to_string());
                }
            }
            None => {
                let root = crate::layout::LayoutTree::Leaf(pane);
                if self
                    .open_tab_through_model(
                        ws_idx,
                        "Browser".to_string(),
                        Some(root),
                        None,
                        fills_active_tab,
                        cx,
                    )
                    .is_err()
                {
                    return Err("Tab limit reached for this workspace".to_string());
                }
                let tab_idx = self.workspaces[ws_idx].active_tab_idx();
                self.focus_workspace_tab(ws_idx, tab_idx, window, cx);
            }
        }
        let focus = view.read(cx).address_focus(cx);
        window.focus(&focus, cx);
        cx.notify();
        Ok(())
    }

    fn open_browser_beside(
        &mut self,
        opener: &Entity<BrowserView>,
        tab: TabId,
        cx: &mut Context<Self>,
    ) {
        let workspace_id = opener.read(cx).workspace_id;
        let attached = self.new_browser_view(|cx| BrowserView::attached(workspace_id, tab, cx), cx);
        let in_dock = self
            .diff_dock
            .diff_tabs
            .iter()
            .any(|entry| matches!(entry, DiffDockTab::Browser(view) if view == opener));
        if !in_dock && let Some(pane) = self.pane_holding_browser(opener, cx) {
            let surface = PaneSurface::Browser(attached.clone());
            let added = self.add_surface_through_model_as(
                &pane,
                crate::browser::surface_definition(attached.read(cx)),
                Box::new(move |_, _, _| surface),
                cx,
            );
            if added.is_some() {
                return;
            }
        }
        self.diff_dock
            .diff_tabs
            .push(DiffDockTab::Browser(attached));
        self.show_diff_tab(self.diff_dock.diff_tabs.len() - 1, cx);
    }

    fn pane_holding_browser(
        &self,
        view: &Entity<BrowserView>,
        cx: &gpui::App,
    ) -> Option<Entity<Pane>> {
        self.workspaces
            .iter()
            .flat_map(|ws| ws.tabs().iter())
            .filter_map(|tab| tab.root.as_ref())
            .flat_map(crate::layout::LayoutTree::collect_leaves)
            .find(|pane| {
                pane.read(cx)
                    .surfaces()
                    .iter()
                    .any(|surface| matches!(surface, PaneSurface::Browser(held) if held == view))
            })
    }

    fn show_browser_tab(&mut self, from: &Entity<BrowserView>, tab: TabId, cx: &mut Context<Self>) {
        if let Some(index) = self.diff_dock.diff_tabs.iter().position(
            |entry| matches!(entry, DiffDockTab::Browser(view) if view.read(cx).tab == Some(tab)),
        ) {
            self.show_diff_tab(index, cx);
            return;
        }
        if let Some(existing) = crate::browser::view_for(tab, cx)
            && let Some(pane) = self.pane_holding_browser(&existing, cx)
        {
            let index = pane.read(cx).surfaces().iter().position(
                |surface| matches!(surface, PaneSurface::Browser(held) if *held == existing),
            );
            if let Some(index) = index {
                pane.update(cx, |pane, cx| pane.activate_surface(index, cx));
                return;
            }
        }
        let workspace_id = from.read(cx).workspace_id;
        let attached = self.new_browser_view(|cx| BrowserView::attached(workspace_id, tab, cx), cx);
        self.diff_dock
            .diff_tabs
            .push(DiffDockTab::Browser(attached));
        self.show_diff_tab(self.diff_dock.diff_tabs.len() - 1, cx);
    }
}
