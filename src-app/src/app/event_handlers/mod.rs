use gpui::{App, AppContext, Context, Entity};
use notify::Watcher;
use paneflow_config::schema::TerminalSurfaceProfile;

use crate::app::close_policy::{CloseIntent, CloseTarget};
use crate::layout::{LayoutTree, MAX_PANES};
use crate::pane::{self, Pane};
use crate::pane_drag::DropEdge;
use crate::terminal::{self, TerminalView};
use crate::window_chrome::title_bar;
use crate::{PaneFlowApp, ai_types};

mod cwd_tracking;
mod pane_scan;
mod session_reaper;

use pane_scan::*;

pub(crate) fn split_pane_at_edge(
    root: &mut LayoutTree,
    target: &Entity<Pane>,
    edge: DropEdge,
    new_pane: Entity<Pane>,
) -> bool {
    let (direction, swap) = edge.to_split();
    if !root.split_at_pane(target, direction, new_pane.clone()) {
        return false;
    }
    if swap {
        root.swap_panes(target, &new_pane);
    }
    true
}

fn open_pane_in_new_workspace_tab(
    workspaces: &mut [crate::workspace::Workspace],
    ws_idx: usize,
    pane: Entity<Pane>,
) -> bool {
    workspaces.get_mut(ws_idx).is_some_and(|ws| {
        ws.open_tab(crate::workspace::Tab::new(
            String::new(),
            Some(crate::layout::LayoutTree::Leaf(pane)),
        ))
    })
}

impl PaneFlowApp {
    pub(crate) fn open_pane_in_new_workspace_tab(
        &mut self,
        ws_idx: usize,
        pane: Entity<Pane>,
        cx: &mut Context<Self>,
    ) -> bool {
        let opened = open_pane_in_new_workspace_tab(&mut self.workspaces, ws_idx, pane);
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
            title_bar::TitleBarEvent::ToggleFilesMenu(anchor) => {
                let open = self.title_bar_files_menu_open.is_none();
                self.dismiss_transient_surfaces();
                self.title_bar_files_menu_open = open.then_some(*anchor);
                cx.notify();
            }
            title_bar::TitleBarEvent::ToggleHelpMenu(anchor) => {
                let open = self.title_bar_help_menu_open.is_none();
                self.dismiss_transient_surfaces();
                self.title_bar_help_menu_open = open.then_some(*anchor);
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
            pane::PaneEvent::SurfacesChanged => {
                self.save_session(cx);
                cx.notify();
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
                let terminal = cx.new(|cx| TerminalView::with_cwd(ws_id, cwd, None, cx));
                cx.subscribe(&terminal, Self::handle_terminal_event)
                    .detach();
                pane.update(cx, |pane, cx| {
                    pane.push_surface(pane::PaneSurface::Terminal(terminal), cx);
                });
                self.pending_pane_focus = Some(pane);
                self.save_session(cx);
                cx.notify();
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
                let cwd = self.new_surface_cwd(&pane, ws_id, cx);
                let command = preset.command(&self.cached_config);
                let env = preset.env();
                let profile = preset.profile();
                let terminal = cx.new(|cx| {
                    TerminalView::with_cwd_env_and_profile(ws_id, cwd, None, env, profile, cx)
                });
                cx.subscribe(&terminal, Self::handle_terminal_event)
                    .detach();
                pane.update(cx, |pane, cx| {
                    pane.push_surface(pane::PaneSurface::Terminal(terminal.clone()), cx);
                });
                if let Some(command) = command.as_deref() {
                    terminal.read(cx).send_command(command);
                    terminal.update(cx, |view, _cx| view.declare_agent_from_command(command));
                }
                self.pending_pane_focus = Some(pane);
                self.save_session(cx);
                cx.notify();
            }
            pane::PaneEvent::Remove => {
                self.perform_close(CloseTarget::Pane(pane), CloseIntent::Stop, None, cx);
            }
            pane::PaneEvent::ToggleAgentSessions => {
                if self.agent_sessions.sessions_sidebar_open {
                    self.close_sessions_sidebar(cx);
                    return;
                }
                self.open_sessions_sidebar_for_pane(&pane, None, cx);
            }
            pane::PaneEvent::ToggleDiffDock => {
                let owner_id = pane.read(cx).workspace_id;
                let Some(cwd) = self.checkout_for_pane(&pane).or_else(|| {
                    self.workspaces
                        .iter()
                        .find(|ws| ws.id == owner_id)
                        .map(|ws| ws.cwd.clone())
                }) else {
                    return;
                };
                self.toggle_cli_diff_dock(cwd, cx);
            }
            pane::PaneEvent::OpenPaneMenu { position } => {
                self.dismiss_transient_surfaces();
                self.pane_menu_open = Some(crate::PaneContextMenu {
                    pane: pane.clone(),
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
                        let inserted = if let Some(root) = self.workspaces[ws_idx]
                            .tab_mut(tab_idx)
                            .and_then(|tab| tab.root.as_mut())
                        {
                            split_pane_at_edge(root, &target, edge, new_pane.clone())
                        } else {
                            false
                        };
                        if !inserted {
                            return;
                        }
                        self.pending_pane_focus = Some(new_pane);
                    }
                    None => {
                        let new_pane = self.create_pane(term, ws_id, cx);
                        if !self.open_pane_in_new_workspace_tab(ws_idx, new_pane.clone(), cx) {
                            return;
                        }
                        self.pending_pane_focus = Some(new_pane);
                    }
                }
                self.save_session(cx);
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
                let Some((ws_idx, tab_idx)) =
                    self.workspaces.iter().enumerate().find_map(|(idx, ws)| {
                        ws.tab_index_containing_pane(&target).map(|t| (idx, t))
                    })
                else {
                    return;
                };
                let Some(root) = self.workspaces[ws_idx]
                    .tab_mut(tab_idx)
                    .and_then(|tab| tab.root.as_mut())
                else {
                    return;
                };
                let Some(source) = root
                    .collect_leaves()
                    .into_iter()
                    .find(|p| p.entity_id().as_u64() == source_pane_id)
                else {
                    return;
                };

                let moved = match edge {
                    None => root.swap_panes(&source, &target),
                    Some(edge) => {
                        let Some(mut tree) = self.workspaces[ws_idx]
                            .tab_mut(tab_idx)
                            .and_then(|tab| tab.root.take())
                        else {
                            return;
                        };
                        let (pruned, removed) = tree.remove_pane(&source);
                        let mut moved = false;
                        tree = pruned.unwrap_or_else(|| LayoutTree::Leaf(source.clone()));
                        if removed && tree.contains_leaf(&target) {
                            moved = split_pane_at_edge(&mut tree, &target, edge, source.clone());
                            if !moved {
                                moved = tree.first_leaf().is_some_and(|anchor| {
                                    tree.split_at_pane(
                                        &anchor,
                                        crate::layout::SplitDirection::Vertical,
                                        source.clone(),
                                    )
                                });
                            }
                        }
                        if let Some(tab) = self.workspaces[ws_idx].tab_mut(tab_idx) {
                            tab.root = Some(tree);
                        }
                        moved
                    }
                };
                if !moved {
                    return;
                }
                self.pending_pane_focus = Some(source);
                self.save_session(cx);
                cx.notify();
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
        let Some((w, t)) = self
            .workspaces
            .iter()
            .enumerate()
            .find_map(|(w, ws)| ws.tab_index_containing_pane(&target).map(|t| (w, t)))
        else {
            return;
        };
        let Some(tab) = self.workspaces[w].tabs().get(t) else {
            return;
        };
        if tab.is_zoomed() {
            return;
        }
        let Some(root) = tab.root.as_ref() else {
            return;
        };
        let leaves = root.collect_leaves();
        let Some(source) = leaves
            .iter()
            .find(|p| p.entity_id().as_u64() == source_id)
            .cloned()
        else {
            return;
        };
        if source.read(cx).is_detached() || target.read(cx).is_detached() {
            return;
        }
        let Some(index) = source
            .read(cx)
            .surfaces()
            .iter()
            .position(|s| s.entity_id() == surface_id)
        else {
            return;
        };
        let last = source.read(cx).surfaces().len() == 1;
        if source == target && (edge.is_none() || last) {
            return;
        }
        if edge.is_none() && !target.read(cx).can_add_surface() {
            return;
        }
        if edge.is_some() && !last && leaves.len() >= MAX_PANES {
            self.show_toast(format!("Maximum pane count reached ({MAX_PANES})"), cx);
            return;
        }
        let surface = source.read(cx).surfaces()[index].clone();
        let destination = if edge.is_some() {
            let workspace_id = target.read(cx).workspace_id;
            self.create_pane_with_existing_surface(surface, workspace_id, cx)
        } else {
            target.update(cx, |pane, cx| pane.push_surface(surface, cx));
            target.clone()
        };
        let root = &mut self.workspaces[w].tab_mut(t).expect("validated tab").root;
        if let Some(edge) = edge {
            let Some(tree) = root.as_mut() else {
                return;
            };
            if !split_pane_at_edge(tree, &target, edge, destination.clone()) {
                return;
            }
        }
        if last {
            if let Some(tree) = root.take() {
                *root = tree.remove_pane(&source).0;
            }
        } else {
            source.update(cx, |pane, cx| pane.remove_surface_at(index, cx));
        }
        self.pending_pane_focus = Some(destination);
        self.save_session(cx);
        cx.notify();
    }

    pub(crate) fn remove_pane_from_layout(&mut self, pane: Entity<Pane>, cx: &mut Context<Self>) {
        let Some((ws_idx, tab_idx)) = self
            .workspaces
            .iter()
            .enumerate()
            .find_map(|(idx, ws)| ws.tab_index_containing_pane(&pane).map(|t| (idx, t)))
        else {
            return;
        };

        let Some(tab) = self.workspaces[ws_idx].tabs().get(tab_idx) else {
            return;
        };
        let root_contains = tab
            .root
            .as_ref()
            .is_some_and(|root| root.contains_leaf(&pane));
        let saved_contains = tab
            .saved_layout
            .as_ref()
            .is_some_and(|saved| saved.contains_leaf(&pane));

        if let Some(tab) = self.workspaces[ws_idx].tab_mut(tab_idx) {
            if saved_contains {
                if let Some(saved) = tab.saved_layout.take() {
                    let (new_saved, _) = saved.remove_pane(&pane);
                    if root_contains {
                        tab.root = new_saved;
                    } else {
                        tab.saved_layout = new_saved;
                    }
                }
            } else if let Some(root) = tab.root.take() {
                let (new_root, _) = root.remove_pane(&pane);
                tab.root = new_root;
            }
        }

        let tab_is_empty = self.workspaces[ws_idx]
            .tabs()
            .get(tab_idx)
            .is_none_or(|tab| tab.root.is_none());
        if tab_is_empty {
            let ws_id = self.workspaces[ws_idx].id;
            let cwd = std::path::PathBuf::from(&self.workspaces[ws_idx].cwd);
            let terminal = cx.new(|cx| TerminalView::with_cwd(ws_id, Some(cwd), None, cx));
            let new_pane = self.create_pane(terminal, ws_id, cx);
            if let Some(tab) = self.workspaces[ws_idx].tab_mut(tab_idx) {
                tab.root = Some(LayoutTree::Leaf(new_pane));
            }
        }
        self.save_session(cx);
        cx.notify();
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
            terminal::TerminalEvent::CancelSwapMode => {
                self.cancel_swap_mode(cx);
            }
            terminal::TerminalEvent::SelectionCopied => {
                self.show_toast("Copied", cx);
            }
            terminal::TerminalEvent::OpenMarkdownPath(path) => {
                self.open_markdown_in_pane(&terminal, path.clone(), cx);
            }
            terminal::TerminalEvent::FontZoomChanged => {
                self.save_session(cx);
            }
            terminal::TerminalEvent::FleetSearchRequested { query, regex } => {
                self.start_fleet_search(query.clone(), *regex, cx);
            }
            terminal::TerminalEvent::OpenCodePath { path, line, col } => {
                let path = path.clone();
                let line = *line;
                let col = *col;
                let preference = self.cached_config.external_editor.clone();
                cx.background_executor()
                    .spawn(async move {
                        crate::editor::open_at_location(&path, line, col, preference.as_deref());
                    })
                    .detach();
            }
            terminal::TerminalEvent::ProgramNotification { title, body } => {
                let surface_id = terminal.entity_id().as_u64();
                let ws_id = self.workspace_id_for_surface(surface_id, cx);
                let seen = ws_id
                    .and_then(|ws_id| self.surfaces_under_user_eye(ws_id, cx))
                    .is_some_and(|visible| visible.contains(&surface_id))
                    || ws_id.is_some_and(|ws_id| self.workspace_is_muted(ws_id));
                let pane_title = terminal.read(cx).terminal.title.clone();
                crate::agents::notifications::fire_program_notification(
                    crate::agents::notifications::program_notification(
                        title.clone(),
                        body.clone(),
                        &pane_title,
                    ),
                    seen,
                    cx.background_executor().clone(),
                );
            }
            terminal::TerminalEvent::ShellPromptReady => {
                let child_pid = terminal.read(cx).terminal.child_pid;
                self.reap_sessions_at_shell_prompt(terminal.entity_id().as_u64(), child_pid, cx);
            }
            terminal::TerminalEvent::ChildExited => {
                self.purge_sessions_for_surface(terminal.entity_id().as_u64(), cx);
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

#[cfg(test)]
mod tests {

    #[gpui::test]
    fn edgeless_drop_opens_a_new_workspace_tab(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;

        let cx = cx.add_empty_window();
        let new_pane = |cx: &mut gpui::VisualTestContext| {
            let terminal = cx.new(|cx| crate::terminal::TerminalView::display_only_for_test(1, cx));
            cx.new(|cx| crate::pane::Pane::new(terminal, 1, cx))
        };

        let target = new_pane(cx);
        let target_surface = cx.update(|_, cx| target.read(cx).active_terminal_opt().cloned());
        let mut workspaces = vec![crate::workspace::Workspace::with_layout_and_id(
            1,
            "ws",
            std::path::PathBuf::new(),
            crate::layout::LayoutTree::Leaf(target.clone()),
        )];

        assert_eq!(
            crate::pane_drag::compute_drop_edge(
                100.0,
                100.0,
                50.0,
                50.0,
                crate::pane_drag::SPLIT_EDGE_BAND
            ),
            None
        );

        let dropped = new_pane(cx);
        assert!(super::open_pane_in_new_workspace_tab(
            &mut workspaces,
            0,
            dropped.clone()
        ));

        assert_eq!(workspaces[0].tab_count(), 2, "the drop opened a new tab");
        assert_eq!(
            workspaces[0].active_tab_idx(),
            1,
            "the new tab is the active one"
        );
        assert_eq!(
            workspaces[0].tabs()[1]
                .root
                .as_ref()
                .map(|root| root.collect_leaves()),
            Some(vec![dropped]),
            "the new tab holds the dropped surface alone"
        );
        assert_eq!(
            workspaces[0].tabs()[0]
                .root
                .as_ref()
                .map(|root| root.collect_leaves()),
            Some(vec![target.clone()])
        );
        assert_eq!(
            cx.update(|_, cx| target.read(cx).active_terminal_opt().cloned()),
            target_surface,
            "the pane dropped onto keeps its own surface"
        );

        while workspaces[0].tab_count() < crate::workspace::MAX_TABS_PER_WORKSPACE {
            assert!(workspaces[0].open_tab(crate::workspace::Tab::empty()));
        }
        let refused = new_pane(cx);
        assert!(!super::open_pane_in_new_workspace_tab(
            &mut workspaces,
            0,
            refused
        ));
        assert_eq!(
            workspaces[0].tab_count(),
            crate::workspace::MAX_TABS_PER_WORKSPACE
        );
    }
}
