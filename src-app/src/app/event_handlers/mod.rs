use gpui::{App, AppContext, Context, Entity};
use paneflow_config::schema::TerminalSurfaceProfile;

use crate::app::close_policy::CloseTarget;
use crate::layout::MAX_PANES;
use crate::pane::{self, Pane};
use crate::pane_drag::DropEdge;
use crate::terminal::{self, TerminalView};
use crate::window_chrome::title_bar;
use crate::{PaneFlowApp, ai_types};

mod cwd_tracking;
mod pane_scan;
mod session_reaper;

pub(crate) use cwd_tracking::CwdProbeSequence;
use pane_scan::*;

impl PaneFlowApp {
    pub(crate) fn open_pane_in_new_workspace_tab(
        &mut self,
        ws_idx: usize,
        pane: Entity<Pane>,
        cx: &mut Context<Self>,
    ) -> bool {
        let opened = self
            .open_tab_through_model(
                ws_idx,
                String::new(),
                Some(crate::layout::LayoutTree::Leaf(pane)),
                None,
                false,
                cx,
            )
            .is_ok();
        if !opened {
            self.show_toast("Tab limit reached for this workspace", cx);
        }
        opened
    }

    pub(crate) fn handle_title_bar_event(
        &mut self,
        _title_bar: Entity<title_bar::TitleBar>,
        event: &title_bar::TitleBarEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            title_bar::TitleBarEvent::CloseRequested => {
                self.request_quit(cx);
            }
            title_bar::TitleBarEvent::ToggleSidebar => {
                self.toggle_primary_sidebar(cx);
                if !self.primary_sidebar_visible {
                    self.dismiss_transient_surfaces();
                } else {
                    self.title_bar_files_menu_open = None;
                    self.title_bar_help_menu_open = None;
                }
            }
            title_bar::TitleBarEvent::ToggleDiffDock => {
                self.toggle_diff_dock(cx);
            }
            title_bar::TitleBarEvent::ToggleFilesMenu { anchor, was_open } => {
                self.dismiss_transient_surfaces();
                self.title_bar_files_menu_open = (!was_open).then_some(*anchor);
                cx.notify();
            }
            title_bar::TitleBarEvent::ToggleHelpMenu { anchor, was_open } => {
                self.dismiss_transient_surfaces();
                self.title_bar_help_menu_open = (!was_open).then_some(*anchor);
                cx.notify();
            }
        }
    }

    fn new_surface_cwd(
        &self,
        pane: &Entity<Pane>,
        ws_id: u64,
        cx: &Context<Self>,
    ) -> Option<std::path::PathBuf> {
        pane.read(cx)
            .active_terminal_opt()
            .and_then(|terminal| {
                let terminal = terminal.read(cx);
                terminal
                    .terminal
                    .current_cwd
                    .as_deref()
                    .filter(|cwd| !cwd.is_empty())
                    .map(std::path::PathBuf::from)
                    .or_else(|| terminal.terminal.cwd_now())
            })
            .or_else(|| {
                self.workspaces
                    .iter()
                    .find(|ws| ws.id == ws_id)
                    .map(|ws| std::path::PathBuf::from(&ws.cwd))
            })
    }

    pub(crate) fn handle_pane_event(
        &mut self,
        pane: Entity<Pane>,
        event: &pane::PaneEvent,
        cx: &mut Context<Self>,
    ) {
        if let pane::PaneEvent::DropSurfaceMove {
            source_pane_id,
            surface_id,
            edge,
        } = event
        {
            self.move_surface_to_pane(pane, *source_pane_id, *surface_id, *edge, cx);
            return;
        }
        if let pane::PaneEvent::ToggleDetached { window } = event {
            let owner = cx.weak_entity();
            let handle = *window;
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, cx| {
                    let _ =
                        owner.update(cx, |owner, cx| owner.toggle_detached_pane(pane, window, cx));
                });
            });
            return;
        }
        match event {
            pane::PaneEvent::DropSurfaceMove { .. } => {}
            pane::PaneEvent::ToggleDetached { .. } => {}
            pane::PaneEvent::SurfacesChanged => cx.notify(),
            pane::PaneEvent::ActiveSurfaceChanged => {
                self.select_surface_through_model(&pane, cx);
                cx.notify();
            }
            pane::PaneEvent::CloseViewRequested(surface) => {
                self.close_surface_entity_through_model(*surface, cx);
            }
            pane::PaneEvent::CloseRequested => {
                self.request_close(CloseTarget::Pane(pane), None, cx);
            }
            pane::PaneEvent::CloseSurfaceRequested(terminal) => {
                self.request_close(
                    CloseTarget::Surface {
                        pane: pane.clone(),
                        terminal: terminal.clone(),
                    },
                    None,
                    cx,
                );
            }
            pane::PaneEvent::NewTab => {
                let ws_id = pane.read(cx).workspace_id;
                if !pane.read(cx).can_add_surface() {
                    self.show_toast(
                        format!("Maximum tab count reached ({})", pane::MAX_PANE_TABS),
                        cx,
                    );
                    return;
                }
                let cwd = self.new_surface_cwd(&pane, ws_id, cx);
                let added = self.add_surface_through_model(
                    &pane,
                    Box::new(move |_, ws_id, cx| {
                        crate::app::layout_bridge::subscribed_terminal(
                            cx.new(|cx| TerminalView::with_cwd(ws_id, cwd, None, cx)),
                            cx,
                        )
                    }),
                    cx,
                );
                if added.is_none() {
                    self.show_toast(
                        format!("Maximum tab count reached ({})", pane::MAX_PANE_TABS),
                        cx,
                    );
                }
            }
            pane::PaneEvent::OpenNewTabMenu => {
                let ws_id = pane.read(cx).workspace_id;
                let Some(ws_idx) = self.workspaces.iter().position(|ws| ws.id == ws_id) else {
                    return;
                };
                let presets = self.pane_palette_presets(ws_idx);
                pane.update(cx, |pane, cx| pane.open_new_tab_menu(presets, cx));
            }
            pane::PaneEvent::NewTabPreset(preset) => {
                let ws_id = pane.read(cx).workspace_id;
                if !pane.read(cx).can_add_surface() {
                    self.show_toast(
                        format!("Maximum tab count reached ({})", pane::MAX_PANE_TABS),
                        cx,
                    );
                    return;
                }
                if let Err(message) = preset.ensure_launchable() {
                    self.show_toast(message, cx);
                    return;
                }
                if preset.is_browser() {
                    self.add_browser_surface_to_pane(&pane, cx);
                    return;
                }
                let env = match preset.env(&self.cached_config) {
                    Ok(env) => env,
                    Err(message) => {
                        self.show_toast(message, cx);
                        return;
                    }
                };
                let cwd = self.new_surface_cwd(&pane, ws_id, cx);
                let command = preset.command(&self.cached_config);
                let profile = preset.profile();
                let added = self.add_surface_through_model(
                    &pane,
                    Box::new(move |_, ws_id, cx| {
                        let view = cx.new(|cx| {
                            TerminalView::with_cwd_env_and_profile(
                                ws_id, cwd, None, env, profile, cx,
                            )
                        });
                        crate::app::layout_bridge::subscribed_terminal(view, cx)
                    }),
                    cx,
                );
                let Some(pane::PaneSurface::Terminal(terminal)) = added else {
                    self.show_toast(
                        format!("Maximum tab count reached ({})", pane::MAX_PANE_TABS),
                        cx,
                    );
                    return;
                };
                if let Some(command) = command.as_deref() {
                    terminal.read(cx).send_command(command);
                    terminal.update(cx, |view, _cx| view.declare_agent_from_command(command));
                }
            }
            pane::PaneEvent::ToggleAgentSessions => {
                if self.agent_sessions.sessions_sidebar_open {
                    self.close_sessions_sidebar(cx);
                    return;
                }
                self.open_sessions_sidebar_for_pane(&pane, None, cx);
            }
            pane::PaneEvent::OpenPaneMenu { position } => {
                self.dismiss_transient_surfaces();
                self.pane_menu_open = Some(crate::PaneContextMenu {
                    pane: pane.downgrade(),
                    position: *position,
                });
                cx.notify();
            }
            pane::PaneEvent::DropSessionSplit {
                edge,
                agent,
                session_id,
                cwd,
            } => {
                let edge = *edge;
                let agent = *agent;
                let session_id = session_id.clone();
                let cwd = cwd.clone();
                let target = pane.clone();

                let Some((ws_idx, tab_idx)) =
                    self.workspaces.iter().enumerate().find_map(|(idx, ws)| {
                        ws.tab_index_containing_pane(&target).map(|t| (idx, t))
                    })
                else {
                    return;
                };

                if edge.is_some()
                    && self.workspaces[ws_idx]
                        .tabs()
                        .get(tab_idx)
                        .is_some_and(|tab| tab.is_zoomed())
                {
                    self.show_toast("Unzoom before splitting panes", cx);
                    return;
                }
                if edge.is_some()
                    && !self.workspaces[ws_idx]
                        .tabs()
                        .get(tab_idx)
                        .is_some_and(|tab| tab.can_add_pane())
                {
                    return;
                }

                let ws_id = self.workspaces[ws_idx].id;
                let cwd_path = (!cwd.is_empty()).then(|| std::path::PathBuf::from(&cwd));
                let term = cx.new(|cx| {
                    TerminalView::with_cwd_and_profile(
                        ws_id,
                        cwd_path,
                        None,
                        TerminalSurfaceProfile::Agent,
                        cx,
                    )
                });
                if let Some(resume) = crate::app::sessions_sidebar::resume_command(
                    agent,
                    &session_id,
                    &self.cached_config,
                ) {
                    term.read(cx).send_command(&resume);
                    term.update(cx, |view, _cx| {
                        view.declare_launched_agent(agent.terminal_agent())
                    });
                }

                match edge {
                    Some(edge) => {
                        let new_pane = self.create_pane(term, ws_id, cx);
                        if !self.split_with_pane_through_model(&target, edge, &new_pane, cx) {
                            return;
                        }
                    }
                    None => {
                        let new_pane = self.create_pane(term, ws_id, cx);
                        if !self.open_pane_in_new_workspace_tab(ws_idx, new_pane.clone(), cx) {
                            return;
                        }
                        self.pending_pane_focus = Some(new_pane);
                    }
                }
                cx.notify();
            }
            pane::PaneEvent::DropPaneMove {
                source_pane_id,
                edge,
            } => {
                let source_pane_id = *source_pane_id;
                let edge = *edge;
                let target = pane.clone();
                if target.entity_id().as_u64() == source_pane_id {
                    return;
                }
                let Some(source) = crate::workspace::panes_across(&self.workspaces)
                    .into_iter()
                    .find(|p| p.entity_id().as_u64() == source_pane_id)
                else {
                    return;
                };
                self.move_pane_through_model(&source, &target, edge, cx);
            }
            pane::PaneEvent::Split(direction) => {
                let direction = *direction;
                let Some((ws_idx, tab_idx)) =
                    self.workspaces.iter().enumerate().find_map(|(idx, ws)| {
                        ws.tabs()
                            .iter()
                            .position(|tab| {
                                tab.root
                                    .as_ref()
                                    .is_some_and(|root| root.contains_leaf(&pane))
                            })
                            .map(|t| (idx, t))
                    })
                else {
                    return;
                };
                if self.workspaces[ws_idx]
                    .tabs()
                    .get(tab_idx)
                    .is_some_and(|tab| tab.is_zoomed())
                {
                    self.show_toast("Unzoom before splitting panes", cx);
                    return;
                }
                if self.workspaces[ws_idx]
                    .tabs()
                    .get(tab_idx)
                    .is_none_or(|tab| tab.root.is_none() || !tab.can_add_pane())
                {
                    self.show_toast(format!("Maximum pane count reached ({MAX_PANES})"), cx);
                    return;
                }
                self.open_split_palette(pane, direction, cx);
            }
        }
    }

    fn move_surface_to_pane(
        &mut self,
        target: Entity<Pane>,
        source_id: u64,
        surface_id: u64,
        edge: Option<DropEdge>,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = crate::workspace::panes_across(&self.workspaces)
            .into_iter()
            .find(|p| p.entity_id().as_u64() == source_id)
        else {
            return;
        };
        let Some(surface) = source
            .read(cx)
            .surfaces()
            .iter()
            .find(|s| s.entity_id() == surface_id)
            .cloned()
        else {
            return;
        };
        let last = source.read(cx).surfaces().len() == 1;
        if source == target && (edge.is_none() || last) {
            return;
        }
        self.move_surface_through_model(&surface, &target, edge, cx);
    }

    pub(crate) fn remove_pane_from_layout(&mut self, pane: Entity<Pane>, cx: &mut Context<Self>) {
        self.close_pane_through_model(&pane, false, cx);
    }

    pub(crate) fn handle_terminal_event(
        &mut self,
        terminal: Entity<TerminalView>,
        event: &terminal::TerminalEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            terminal::TerminalEvent::ActivityBurst => {
                if let Some(ws_idx) = self.workspace_idx_for_terminal(&terminal, cx) {
                    self.schedule_port_scan(ws_idx, cx);
                }
            }
            terminal::TerminalEvent::CwdChanged(new_cwd) => {
                self.handle_cwd_change(&terminal, new_cwd, cx);
            }
            terminal::TerminalEvent::TitleChanged => {
                self.apply_process_title(&terminal, cx);
            }
            terminal::TerminalEvent::ServiceDetected(info) => {
                terminal.update(cx, |view, _| view.terminal.note_announced_port(info.port));
                if let Some(ws_idx) = self.workspace_idx_for_terminal(&terminal, cx) {
                    let ws = &mut self.workspaces[ws_idx];
                    let mut terminal_info = info.clone();
                    terminal_info.is_frontend = false;
                    if merge_service_label(&mut ws.service_labels, terminal_info)
                        && self.settings_section.is_none()
                    {
                        cx.notify();
                    }
                }
            }
            terminal::TerminalEvent::SelectionCopied => {
                self.show_toast("Copied", cx);
            }
            terminal::TerminalEvent::Notice(message) => {
                self.show_toast(message.clone(), cx);
            }
            terminal::TerminalEvent::OpenMarkdownPath(path) => {
                self.open_markdown_in_pane(&terminal, path.clone(), cx);
            }
            terminal::TerminalEvent::OpenInBrowser { url } => {
                self.open_local_link(url.clone(), cx);
            }
            terminal::TerminalEvent::LinkMenu { url, position } => {
                self.dismiss_transient_surfaces();
                self.terminal_link_menu = Some(crate::app::browser_surface::LinkMenu {
                    url: url.clone(),
                    position: *position,
                });
                cx.notify();
            }
            terminal::TerminalEvent::FontZoomChanged => {
                self.save_session(cx);
            }
            terminal::TerminalEvent::FleetSearchRequested { query, regex } => {
                self.start_fleet_search(query.clone(), *regex, cx);
            }
            terminal::TerminalEvent::OpenCodePath {
                path,
                line,
                col,
                alternate,
                window,
            } => {
                self.follow_code_link(
                    &terminal,
                    crate::app::terminal_links::CodeLink {
                        path: path.clone(),
                        line: *line,
                        col: *col,
                        alternate: *alternate,
                        window: *window,
                    },
                    cx,
                );
            }
            terminal::TerminalEvent::PickCodePath { query, window } => {
                self.pick_code_link(query.clone(), *window, cx);
            }
            terminal::TerminalEvent::TreeFileDropped { path, root, window } => {
                self.tree_file_dropped(terminal, path.clone(), root.clone(), *window, cx);
            }
            terminal::TerminalEvent::ProgramNotification { title, body } => {
                let surface_id = terminal.read(cx).surface_id();
                let ws_id = self.workspace_id_for_surface(surface_id, cx);
                let seen = ws_id
                    .and_then(|ws_id| self.surfaces_under_user_eye(ws_id, cx))
                    .is_some_and(|visible| visible.contains(&surface_id))
                    || ws_id.is_some_and(|ws_id| self.workspace_is_muted(ws_id));
                let pane_title = terminal.read(cx).terminal.title.clone();
                crate::agents::notifications::fire_desktop_notification_for_session(
                    crate::agents::notifications::program_notification(
                        title.clone(),
                        body.clone(),
                        &pane_title,
                    ),
                    &self.cached_config,
                    seen,
                    Some(surface_id),
                    cx.background_executor().clone(),
                );
            }
            terminal::TerminalEvent::ShellPromptReady => {
                let child_pid = terminal.read(cx).terminal.child_pid;
                let reaped = self.reap_sessions_at_shell_prompt(
                    terminal.read(cx).surface_id(),
                    child_pid,
                    cx,
                );
                self.conversation_returned_to_shell(&terminal, reaped, cx);
            }
            terminal::TerminalEvent::AgentSessionChanged => {
                self.save_session(cx);
            }
            terminal::TerminalEvent::AgentDeclared => {
                cx.notify();
            }
            terminal::TerminalEvent::ConversationReady => {
                self.conversation_ready(terminal.read(cx).surface_id(), cx);
            }
            terminal::TerminalEvent::ChildExited => {
                self.purge_sessions_for_surface(terminal.read(cx).surface_id(), cx);
            }
            terminal::TerminalEvent::HostLinkResolved => {
                self.note_host_link_resolved(&terminal, cx);
                self.refresh_owned_sessions(cx);
            }
        }
    }

    fn open_markdown_in_pane(
        &mut self,
        source_terminal: &Entity<TerminalView>,
        path: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        let Some(ws_idx) = self.workspace_idx_for_terminal(source_terminal, cx) else {
            return;
        };
        let ws_id = self.workspaces[ws_idx].id;
        let markdown = cx.new(|cx: &mut Context<crate::markdown::MarkdownView>| {
            crate::markdown::MarkdownView::open(path, cx)
        });
        let new_pane = self.create_pane_with_existing_surface(
            crate::pane::PaneSurface::Markdown(markdown),
            ws_id,
            cx,
        );
        if !self.open_pane_in_new_workspace_tab(ws_idx, new_pane.clone(), cx) {
            return;
        }
        self.pending_pane_focus = Some(new_pane);
        self.save_session(cx);
        cx.notify();
    }

    pub(crate) fn workspace_idx_for_terminal(
        &self,
        terminal: &Entity<TerminalView>,
        cx: &App,
    ) -> Option<usize> {
        self.workspaces
            .iter()
            .position(|ws| ws.any_pane(|pane| pane.read(cx).contains_terminal(terminal)))
    }
}
