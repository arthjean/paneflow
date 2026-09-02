use gpui::{App, AppContext, Context, Entity};
use notify::Watcher;
use paneflow_config::schema::TerminalSurfaceProfile;

use crate::layout::{LayoutTree, MAX_PANES};
use crate::pane::{self, Pane};
use crate::pane_drag::DropEdge;
use crate::terminal::{self, TerminalView};
use crate::window_chrome::title_bar;
use crate::{PaneFlowApp, ai_types};

fn pid_is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        if pid > i32::MAX as u32 {
            return false;
        }
        let ret = unsafe { libc::kill(pid as i32, 0) };
        if ret == -1 {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            return errno != libc::ESRCH;
        }
        true
    }

    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::OpenProcess;
        const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
        if pid == 0 {
            return false;
        }
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let _ = CloseHandle(handle);
            true
        }
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        true
    }
}

fn split_pane_at_edge(
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

#[cfg(any(target_os = "linux", test))]
fn parse_proc_stat_starttime(stat: &str) -> Option<u64> {
    let after = stat.rsplit_once(')')?.1;
    after.split_whitespace().nth(19)?.parse::<u64>().ok()
}

#[cfg(target_os = "linux")]
pub(crate) fn pid_start_time(pid: u32) -> Option<u64> {
    let content = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_proc_stat_starttime(&content)
}

#[cfg(target_os = "macos")]
pub(crate) fn pid_start_time(pid: u32) -> Option<u64> {
    use libproc::libproc::bsd_info::BSDInfo;
    use libproc::libproc::proc_pid::pidinfo;
    let info = pidinfo::<BSDInfo>(pid as i32, 0).ok()?;
    Some(
        info.pbi_start_tvsec
            .wrapping_mul(1_000_000)
            .wrapping_add(info.pbi_start_tvusec),
    )
}

#[cfg(windows)]
pub(crate) fn pid_start_time(pid: u32) -> Option<u64> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{GetProcessTimes, OpenProcess};
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    if pid == 0 {
        return None;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut creation: FILETIME = std::mem::zeroed();
        let mut exit: FILETIME = std::mem::zeroed();
        let mut kernel: FILETIME = std::mem::zeroed();
        let mut user: FILETIME = std::mem::zeroed();
        let ok = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
        let _ = CloseHandle(handle);
        if ok == 0 {
            return None;
        }
        Some(((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub(crate) fn pid_start_time(_pid: u32) -> Option<u64> {
    None
}

fn pid_matches(pid: u32, pinned_start: Option<u64>) -> bool {
    if !pid_is_alive(pid) {
        return false;
    }
    match (pinned_start, pid_start_time(pid)) {
        (Some(pinned), Some(current)) => pinned == current,
        _ => true,
    }
}

fn keep_session_after_surface_purge(
    dying_surface_id: u64,
    pid: u32,
    session: &ai_types::AgentSession,
) -> bool {
    if session.surface_id == Some(dying_surface_id) {
        return false;
    }
    session.surface_id.is_some() || pid > i32::MAX as u32 || pid_matches(pid, session.proc_start)
}

fn keep_session_at_shell_prompt(
    prompt_surface_id: u64,
    surface_child_pid: u32,
    pid: u32,
    session: &ai_types::AgentSession,
) -> bool {
    if session.surface_id != Some(prompt_surface_id) {
        return true;
    }
    session.state == ai_types::AgentState::Errored
        || (pid != surface_child_pid
            && pid <= i32::MAX as u32
            && pid_matches(pid, session.proc_start))
}

fn keep_session_without_agent_in_pane(
    surface_id: u64,
    surface_child_pid: u32,
    pid: u32,
    session: &ai_types::AgentSession,
) -> bool {
    if session.surface_id != Some(surface_id) {
        return true;
    }
    session.state == ai_types::AgentState::Errored
        || (pid != surface_child_pid
            && pid <= i32::MAX as u32
            && pid_matches(pid, session.proc_start))
}

fn stale_sweep_keeps_without_pid_probe(
    pid: u32,
    session: &ai_types::AgentSession,
    live_surfaces: &std::collections::HashSet<u64>,
) -> bool {
    pid > i32::MAX as u32
        || (session.state == ai_types::AgentState::Errored
            && session
                .surface_id
                .is_some_and(|sid| live_surfaces.contains(&sid)))
}

fn merge_service_label(
    labels: &mut std::collections::HashMap<u16, crate::terminal::ServiceInfo>,
    info: crate::terminal::ServiceInfo,
) -> bool {
    if let Some(existing) = labels.get(&info.port)
        && existing.is_frontend
        && !info.is_frontend
    {
        return false;
    }
    if labels.get(&info.port) == Some(&info) {
        return false;
    }
    labels.insert(info.port, info);
    true
}

fn scan_workspace_ports(
    scan: &std::collections::HashMap<u64, crate::workspace::PaneScan>,
) -> Vec<u16> {
    let mut ports: Vec<u16> = scan
        .values()
        .flat_map(|s| s.ports.iter().map(|e| e.port))
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

fn declaration_survives_scan(
    scanned: Option<crate::agent_launcher::TerminalAgent>,
    declared_until: Option<std::time::Instant>,
    now: std::time::Instant,
) -> bool {
    scanned.is_none() && declared_until.is_some_and(|until| now < until)
}

fn scan_detected_agents(
    scan: &std::collections::HashMap<u64, crate::workspace::PaneScan>,
) -> std::collections::HashSet<String> {
    scan.values()
        .flat_map(|s| s.agents.iter().cloned())
        .collect()
}

fn merge_frontend_scan_labels(
    labels: &mut std::collections::HashMap<u16, crate::terminal::ServiceInfo>,
    scan: &std::collections::HashMap<u64, crate::workspace::PaneScan>,
) -> bool {
    let mut changed = false;
    for entry in scan.values().flat_map(|s| s.ports.iter()) {
        let Some(label) = entry.frontend else {
            continue;
        };
        let fallback_url = || format!("http://localhost:{}", entry.port);
        match labels.entry(entry.port) {
            std::collections::hash_map::Entry::Occupied(mut e) => {
                let info = e.get_mut();
                if !info.is_frontend {
                    info.is_frontend = true;
                    info.label = Some(label.to_string());
                    if info.url.is_none() {
                        info.url = Some(fallback_url());
                    }
                    changed = true;
                    continue;
                }
                if info.label.is_none() {
                    info.label = Some(label.to_string());
                    changed = true;
                }
                if info.url.is_none() {
                    info.url = Some(fallback_url());
                    changed = true;
                }
            }
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(crate::terminal::ServiceInfo {
                    port: entry.port,
                    url: Some(fallback_url()),
                    label: Some(label.to_string()),
                    is_frontend: true,
                });
                changed = true;
            }
        }
    }
    changed
}

fn merge_scan_workspace_state(
    active_ports: &mut Vec<u16>,
    service_labels: &mut std::collections::HashMap<u16, crate::terminal::ServiceInfo>,
    detected_agents: &mut std::collections::HashSet<String>,
    scan: &std::collections::HashMap<u64, crate::workspace::PaneScan>,
) -> bool {
    let ports = scan_workspace_ports(scan);
    let next_agents = scan_detected_agents(scan);
    let mut changed = false;

    if *active_ports != ports {
        *active_ports = ports;
        changed = true;
    }
    let before = service_labels.len();
    service_labels.retain(|port, _| active_ports.contains(port));
    if service_labels.len() != before {
        changed = true;
    }
    let frontend_ports: std::collections::HashSet<u16> = scan
        .values()
        .flat_map(|s| s.ports.iter())
        .filter(|entry| entry.frontend.is_some())
        .map(|entry| entry.port)
        .collect();
    for info in service_labels.values_mut() {
        if info.is_frontend && !frontend_ports.contains(&info.port) {
            info.is_frontend = false;
            changed = true;
        }
    }
    if *detected_agents != next_agents {
        *detected_agents = next_agents;
        changed = true;
    }
    merge_frontend_scan_labels(service_labels, scan) || changed
}

fn port_ownership(
    scan: &std::collections::HashMap<u64, crate::workspace::PaneScan>,
) -> (
    std::collections::HashMap<u16, u64>,
    std::collections::HashSet<u16>,
) {
    let mut owner = std::collections::HashMap::new();
    let mut shared = std::collections::HashSet::new();
    for (tid, s) in scan {
        for e in &s.ports {
            match owner.entry(e.port) {
                std::collections::hash_map::Entry::Occupied(o) => {
                    if *o.get() != *tid {
                        shared.insert(e.port);
                    }
                }
                std::collections::hash_map::Entry::Vacant(v) => {
                    v.insert(*tid);
                }
            }
        }
    }
    (owner, shared)
}

fn announced_port_conflicts(
    announced_ports: &[u16],
    tid: u64,
    owner: &std::collections::HashMap<u16, u64>,
    shared: &std::collections::HashSet<u16>,
    display_names: &std::collections::HashMap<u64, String>,
) -> Vec<(u16, String)> {
    announced_ports
        .iter()
        .filter_map(|p| match owner.get(p) {
            Some(&o) if o != tid && !shared.contains(p) => {
                Some((*p, display_names.get(&o).cloned().unwrap_or_default()))
            }
            _ => None,
        })
        .collect()
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
                self.save_session_blocking(cx);
                self.emit_app_exited_and_flush();
                cx.quit();
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

    pub(crate) fn handle_pane_event(
        &mut self,
        pane: Entity<Pane>,
        event: &pane::PaneEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            pane::PaneEvent::Remove => {
                let Some((ws_idx, tab_idx)) =
                    self.workspaces.iter().enumerate().find_map(|(idx, ws)| {
                        ws.tab_index_containing_pane(&pane).map(|t| (idx, t))
                    })
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
                    term.update(cx, |view, _cx| view.declare_agent(agent.terminal_agent()));
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
                cx.background_executor()
                    .spawn(async move {
                        crate::editor::open_at_location(&path, line, col);
                    })
                    .detach();
            }
            terminal::TerminalEvent::AgentProgressChanged { busy } => {
                self.apply_terminal_agent_observation(
                    &terminal,
                    crate::app::agent_status::progress_lifecycle_event(*busy),
                    cx,
                );
            }
            terminal::TerminalEvent::ProgramNotification { title, body } => {
                let surface_id = terminal.entity_id().as_u64();
                let seen = self
                    .workspace_id_for_surface(surface_id, cx)
                    .and_then(|ws_id| self.surfaces_under_user_eye(ws_id, cx))
                    .is_some_and(|visible| visible.contains(&surface_id));
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
                if let Some(event) =
                    crate::app::agent_status::notification_lifecycle_event(title, body)
                {
                    self.apply_terminal_agent_observation(&terminal, event, cx);
                }
            }
            terminal::TerminalEvent::ShellPromptReady => {
                let child_pid = terminal.read(cx).terminal.child_pid;
                self.reap_sessions_at_shell_prompt(terminal.entity_id().as_u64(), child_pid, cx);
            }
            terminal::TerminalEvent::ChildExited => {
                self.purge_sessions_for_surface(terminal.entity_id().as_u64(), cx);
            }
            _ => {}
        }
    }

    fn apply_terminal_agent_observation(
        &mut self,
        terminal: &Entity<TerminalView>,
        event: crate::ai_types::AgentLifecycleEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(tool) = terminal.read(cx).terminal.detected_agent else {
            return;
        };
        let surface_id = terminal.entity_id().as_u64();
        self.apply_observed_agent_state(
            surface_id,
            tool,
            None,
            event,
            crate::ai_types::AgentStateSource::Terminal,
            cx,
        );
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

    fn workspace_idx_for_terminal(
        &self,
        terminal: &Entity<TerminalView>,
        cx: &App,
    ) -> Option<usize> {
        self.workspaces
            .iter()
            .position(|ws| ws.any_pane(|pane| pane.read(cx).contains_terminal(terminal)))
    }

    pub(crate) fn reap_sessions_at_shell_prompt(
        &mut self,
        surface_id: u64,
        surface_child_pid: u32,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        for ws in &mut self.workspaces {
            if ws.agent_sessions.is_empty() {
                continue;
            }
            let before = ws.agent_sessions.len();
            ws.agent_sessions.retain(|&pid, session| {
                keep_session_at_shell_prompt(surface_id, surface_child_pid, pid, session)
            });
            if ws.agent_sessions.len() < before {
                changed = true;
            }
        }
        if changed {
            self.sync_attention(cx);
            self.agent_sessions_changed(cx);
            cx.notify();
        }
    }

    pub(crate) fn reap_sessions_without_agent(
        &mut self,
        agentless: &[(u64, u32)],
        cx: &mut Context<Self>,
    ) {
        if agentless.is_empty() {
            return;
        }
        let mut changed = false;
        for ws in &mut self.workspaces {
            if ws.agent_sessions.is_empty() {
                continue;
            }
            let before = ws.agent_sessions.len();
            ws.agent_sessions.retain(|&pid, session| {
                agentless.iter().all(|&(surface_id, child_pid)| {
                    keep_session_without_agent_in_pane(surface_id, child_pid, pid, session)
                })
            });
            if ws.agent_sessions.len() < before {
                changed = true;
            }
        }
        if changed {
            self.sync_attention(cx);
            self.agent_sessions_changed(cx);
            cx.notify();
        }
    }

    pub(crate) fn purge_sessions_for_surface(&mut self, surface_id: u64, cx: &mut Context<Self>) {
        let mut changed = false;
        for ws in &mut self.workspaces {
            if ws.agent_sessions.is_empty() {
                continue;
            }
            let before = ws.agent_sessions.len();
            ws.agent_sessions
                .retain(|&pid, session| keep_session_after_surface_purge(surface_id, pid, session));
            if ws.agent_sessions.len() < before {
                changed = true;
            }
        }
        if changed {
            self.sync_attention(cx);
            self.agent_sessions_changed(cx);
            cx.notify();
        }
    }

    pub(crate) fn sweep_stale_pids(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        let live_surfaces: std::collections::HashSet<u64> = self
            .workspaces
            .iter()
            .flat_map(|ws| ws.collect_panes())
            .flat_map(|pane| {
                pane.read(cx)
                    .terminals()
                    .map(|t| t.entity_id().as_u64())
                    .collect::<Vec<_>>()
            })
            .collect();
        let stall_enabled = self.cached_config.agent_stall_detection_enabled();
        let stall_threshold = std::time::Duration::from_secs(
            self.cached_config.resolved_agent_stall_threshold_secs(),
        );
        let mut stalled_notifs: Vec<(
            crate::agent_launcher::TerminalAgent,
            String,
            u64,
            u64,
            Option<u64>,
        )> = Vec::new();
        for ws in &mut self.workspaces {
            if ws.agent_sessions.is_empty() {
                continue;
            }
            let before = ws.agent_sessions.len();
            ws.agent_sessions.retain(|&pid, session| {
                stale_sweep_keeps_without_pid_probe(pid, session, &live_surfaces)
                    || pid_matches(pid, session.proc_start)
            });
            if ws.agent_sessions.len() < before {
                changed = true;
            }
            if stall_enabled {
                for session in ws.agent_sessions.values_mut() {
                    if session
                        .state
                        .stalls_after(session.last_activity.elapsed(), stall_threshold)
                    {
                        session.state = ai_types::AgentState::Stalled;
                        session.waiting_since = None;
                        stalled_notifs.push((
                            session.tool,
                            ws.title.clone(),
                            session.last_activity.elapsed().as_secs(),
                            ws.id,
                            session.surface_id,
                        ));
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.sync_attention(cx);
            self.agent_sessions_changed(cx);
            cx.notify();
        }
        for (agent, title, silent_secs, ws_id, surface_id) in stalled_notifs {
            let seen = crate::app::agent_status::completion_was_seen(
                self.surfaces_under_user_eye(ws_id, cx).as_ref(),
                surface_id,
            );
            super::ipc_handler::fire_stalled_notification(
                agent,
                &title,
                silent_secs,
                &self.cached_config,
                seen,
                cx.background_executor().clone(),
            );
        }
    }

    fn has_unscanned_surface(&self, ws_idx: usize, cx: &Context<Self>) -> bool {
        self.workspaces.get(ws_idx).is_some_and(|ws| {
            ws.collect_panes().iter().any(|pane| {
                pane.read(cx).terminals().any(|tv| {
                    let t = &tv.read(cx).terminal;
                    t.child_pid > 0 && !t.agent_confirmed
                })
            })
        })
    }

    fn schedule_port_scan(&mut self, ws_idx: usize, cx: &mut Context<Self>) {
        let unscanned = self.has_unscanned_surface(ws_idx, cx);
        let ws = &mut self.workspaces[ws_idx];
        if ws.port_scan_pending {
            return;
        }
        ws.port_scan_pending = true;
        ws.port_scan_generation += 1;
        let generation = ws.port_scan_generation;
        let ws_id = ws.id;

        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                if !unscanned {
                    smol::Timer::after(std::time::Duration::from_millis(500)).await;
                }

                for delay_ms in [0u64, 2000, 6000] {
                    if delay_ms > 0 {
                        smol::Timer::after(std::time::Duration::from_millis(delay_ms)).await;
                    }
                    let should_continue = cx.update(|cx| {
                        this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                            app.run_port_scan(ws_id, generation, cx)
                        })
                    });
                    match should_continue {
                        Ok(true) => {}
                        _ => break,
                    }
                }

                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        let Some(ws_idx) = app.workspaces.iter().position(|ws| ws.id == ws_id)
                        else {
                            return;
                        };
                        app.workspaces[ws_idx].port_scan_pending = false;
                        if app.has_unscanned_surface(ws_idx, cx) {
                            app.schedule_port_scan(ws_idx, cx);
                        }
                    })
                });
            },
        )
        .detach();
    }

    pub(crate) fn schedule_active_port_rescans(&mut self, cx: &mut Context<Self>) {
        let workspace_ids: Vec<u64> = self
            .workspaces
            .iter()
            .filter(|ws| !ws.active_ports.is_empty() && !ws.port_scan_pending)
            .map(|ws| ws.id)
            .collect();

        for ws_id in workspace_ids {
            if let Some(ws_idx) = self.workspaces.iter().position(|ws| ws.id == ws_id) {
                self.schedule_port_scan(ws_idx, cx);
            }
        }
    }

    fn run_port_scan(&mut self, ws_id: u64, generation: u64, cx: &mut Context<Self>) -> bool {
        let ws = match self.workspaces.iter().find(|ws| ws.id == ws_id) {
            Some(ws) if ws.port_scan_generation == generation => ws,
            _ => return false,
        };

        let roots: Vec<(u64, u32)> = ws
            .collect_panes()
            .iter()
            .flat_map(|pane| {
                pane.read(cx)
                    .terminals()
                    .filter_map(|tv| {
                        let child_pid = tv.read(cx).terminal.child_pid;
                        (child_pid > 0).then_some((tv.entity_id().as_u64(), child_pid))
                    })
                    .collect::<Vec<_>>()
            })
            .collect();

        if roots.is_empty() {
            return true;
        }

        let submitted: Vec<u64> = roots.iter().map(|(key, _)| *key).collect();

        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut scan = smol::unblock(move || {
                    let agent_binaries: Vec<&'static str> =
                        crate::agent_launcher::TerminalAgent::ALL
                            .iter()
                            .map(|a| a.binary())
                            .collect();
                    crate::workspace::scan_panes(&roots, &agent_binaries)
                })
                .await;
                for key in submitted {
                    scan.entry(key).or_default();
                }
                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        app.apply_pane_scan(ws_id, generation, scan, cx);
                    })
                });
            },
        )
        .detach();
        true
    }

    fn apply_pane_scan(
        &mut self,
        ws_id: u64,
        generation: u64,
        scan: std::collections::HashMap<u64, crate::workspace::PaneScan>,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self
            .workspaces
            .iter_mut()
            .find(|ws| ws.id == ws_id && ws.port_scan_generation == generation)
        else {
            return;
        };

        let mut changed = merge_scan_workspace_state(
            &mut ws.active_ports,
            &mut ws.service_labels,
            &mut ws.detected_agents,
            &scan,
        );

        let live_ports: Vec<u16> = ws.active_ports.clone();

        let frontend_urls: std::collections::HashMap<u16, String> = ws
            .service_labels
            .iter()
            .filter(|(_, info)| info.is_frontend)
            .filter_map(|(port, info)| info.url.clone().map(|u| (*port, u)))
            .collect();

        let leaves: Vec<gpui::Entity<crate::pane::Pane>> = ws.collect_panes();

        let (owner, shared) = port_ownership(&scan);

        let mut display_names: std::collections::HashMap<u64, String> =
            std::collections::HashMap::new();
        for pane in &leaves {
            for tv in pane.read(cx).terminals() {
                let tid = tv.entity_id().as_u64();
                let r = tv.read(cx);
                let name = r
                    .terminal
                    .custom_name
                    .clone()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| {
                        if r.terminal.title.is_empty() {
                            format!("surface {tid}")
                        } else {
                            r.terminal.title.clone()
                        }
                    });
                let name = crate::markdown::strip_bidi_zero_width(name.chars().take(64).collect());
                display_names.insert(tid, name);
            }
        }

        let mut agentless: Vec<(u64, u32)> = Vec::new();

        for pane in &leaves {
            let terminals: Vec<gpui::Entity<crate::terminal::TerminalView>> =
                pane.read(cx).terminals().cloned().collect();
            let mut pane_changed = false;
            for tv in terminals {
                let tid = tv.entity_id().as_u64();
                let Some(s) = scan.get(&tid) else {
                    continue;
                };
                let agent = s
                    .agents
                    .first()
                    .and_then(|b| crate::agent_launcher::TerminalAgent::from_binary(b));
                tv.update(cx, |view, _cx| {
                    let t = &mut view.terminal;
                    t.retain_reported_ports(&live_ports);
                    let in_grace = declaration_survives_scan(
                        agent,
                        t.agent_declared_until,
                        std::time::Instant::now(),
                    );
                    if !in_grace {
                        t.agent_declared_until = None;
                        if t.detected_agent != agent || !t.agent_confirmed {
                            if agent.is_none() && t.detected_agent.is_some() {
                                agentless.push((tid, t.child_pid));
                            }
                            t.detected_agent = agent;
                            t.agent_confirmed = true;
                            pane_changed = true;
                        }
                    }
                    let ports_with_links: Vec<(u16, Option<String>)> = s
                        .ports
                        .iter()
                        .map(|e| (e.port, frontend_urls.get(&e.port).cloned()))
                        .collect();
                    if t.detected_ports != ports_with_links {
                        t.detected_ports = ports_with_links;
                        pane_changed = true;
                    }
                    if t.cached_foreground_command != s.foreground_command {
                        t.cached_foreground_command = s.foreground_command.clone();
                        pane_changed = true;
                    }
                    let conflicts = announced_port_conflicts(
                        &t.announced_ports,
                        tid,
                        &owner,
                        &shared,
                        &display_names,
                    );
                    if t.port_conflicts != conflicts {
                        t.port_conflicts = conflicts;
                        pane_changed = true;
                    }
                });
            }
            if pane_changed {
                pane.update(cx, |_, cx| cx.notify());
                changed = true;
            }
        }

        self.reap_sessions_without_agent(&agentless, cx);

        if changed {
            cx.notify();
        }
    }

    fn handle_cwd_change(
        &mut self,
        terminal: &Entity<TerminalView>,
        new_cwd: &str,
        cx: &mut Context<Self>,
    ) {
        let located = self.workspaces.iter().enumerate().find_map(|(ws_idx, ws)| {
            ws.tabs()
                .iter()
                .position(|tab| {
                    tab.root.as_ref().is_some_and(|root| {
                        root.any_leaf(&mut |pane| {
                            pane.read(cx)
                                .active_terminal_opt()
                                .is_some_and(|t| *t == *terminal)
                        })
                    })
                })
                .map(|tab_idx| (ws_idx, tab_idx))
        });
        let Some((ws_idx, tab_idx)) = located else {
            return;
        };
        let is_active_tab = self.workspaces[ws_idx].active_tab_idx() == tab_idx;

        if self.workspaces[ws_idx].cwd == new_cwd {
            return;
        }

        let ws_id = self.workspaces[ws_idx].id;
        let tab_id = self.workspaces[ws_idx].tabs()[tab_idx].id;
        let ws_repo_root = self.workspaces[ws_idx].repo_root.clone();
        let ws_worktree_root = self.workspaces[ws_idx].worktree_root.clone();

        let new_cwd_owned = new_cwd.to_string();

        cx.spawn({
            let new_cwd = new_cwd_owned.clone();
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (git_dir, branch, is_repo, stats, checkout) = smol::unblock({
                    let cwd = new_cwd.clone();
                    move || {
                        let git_dir = crate::workspace::find_git_dir(&cwd);
                        let (branch, is_repo) = crate::workspace::detect_branch(&cwd);
                        let stats = crate::workspace::GitDiffStats::from_cwd(&cwd);
                        let checkout = git_dir.as_deref().map(|dir| {
                            let (repo_root, is_worktree) = crate::workspace::resolve_repo_root(dir);
                            let root = crate::workspace::resolve_worktree_root(
                                &cwd,
                                Some(dir),
                                repo_root.as_deref(),
                                is_worktree,
                            );
                            (repo_root, root)
                        });
                        (git_dir, branch, is_repo, stats, checkout)
                    }
                })
                .await;

                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        let Some(ws_idx) = app.workspaces.iter().position(|ws| ws.id == ws_id)
                        else {
                            return;
                        };
                        if let Some((repo_root, checkout)) = checkout
                            && repo_root.is_some()
                            && repo_root == ws_repo_root
                            && checkout != ws_worktree_root
                        {
                            if let Some((ws_idx, tab_idx)) = app.tab_position(ws_id, tab_id) {
                                app.set_tab_worktree(ws_idx, tab_idx, Some(checkout.clone()), cx);
                            }
                            let key = checkout.to_string_lossy().into_owned();
                            if app.worktree_states.set_checkout(
                                &key,
                                crate::app::tab_worktree::CheckoutGit {
                                    branch,
                                    is_repo,
                                    stats,
                                },
                            ) {
                                cx.notify();
                            }
                            return;
                        }
                        if !is_active_tab {
                            return;
                        }
                        let old_git_dir = app.workspaces[ws_idx].git_dir.clone();
                        if let Some(ref dir) = old_git_dir {
                            app.unwatch_git_dir(dir);
                        }
                        let tracked_cwd = {
                            let ws = &mut app.workspaces[ws_idx];
                            ws.git_dir = git_dir.clone();
                            ws.cwd.clone()
                        };
                        if let Some(ref dir) = git_dir {
                            let count = app.git_watch_counts.entry(dir.clone()).or_insert(0);
                            *count += 1;
                            if *count == 1
                                && let Some(ref mut watcher) = app.git_watcher
                                && let Err(e) =
                                    watcher.watch(dir, notify::RecursiveMode::NonRecursive)
                            {
                                log::warn!("git watcher: failed to watch {}: {e}", dir.display());
                            }
                        }
                        let changed =
                            app.apply_git_state_for_cwd(&tracked_cwd, branch, is_repo, stats);
                        let refreshed_diff =
                            changed && app.refresh_diff_dock_if_open_for_cwd(&tracked_cwd, cx);
                        log::debug!("workspace CWD changed to: {new_cwd}");
                        if changed && !refreshed_diff {
                            cx.notify();
                        }
                    })
                });
            }
        })
        .detach();
    }

    pub(crate) fn spawn_initial_git_stats(ws_id: u64, cwd: String, cx: &mut Context<Self>) {
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let cwd_for_apply = cwd.clone();
                let (branch, is_repo, stats) = smol::unblock(move || {
                    let (branch, is_repo) = crate::workspace::detect_branch(&cwd);
                    let stats = crate::workspace::GitDiffStats::from_cwd(&cwd);
                    (branch, is_repo, stats)
                })
                .await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        if app.workspaces.iter().any(|ws| ws.id == ws_id) {
                            let changed =
                                app.apply_git_state_for_cwd(&cwd_for_apply, branch, is_repo, stats);
                            let refreshed_diff = changed
                                && app.refresh_diff_dock_if_open_for_cwd(&cwd_for_apply, cx);
                            if changed && !refreshed_diff {
                                cx.notify();
                            }
                        }
                    })
                });
            },
        )
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        announced_port_conflicts, declaration_survives_scan, keep_session_after_surface_purge,
        keep_session_at_shell_prompt, keep_session_without_agent_in_pane,
        merge_scan_workspace_state, merge_service_label, parse_proc_stat_starttime, port_ownership,
        stale_sweep_keeps_without_pid_probe,
    };
    use crate::agent_launcher::TerminalAgent;
    use crate::ai_types::{AgentSession, AgentState};
    use crate::terminal::ServiceInfo;
    use crate::workspace::{PaneScan, PortEntry};
    use std::collections::{HashMap, HashSet};

    #[test]
    fn proc_stat_starttime_survives_hostile_comm_names() {
        let plain = "1234 (zsh) S 1 1234 1234 0 -1 4194304 0 0 0 0 5 3 0 0 20 0 11 0 9876543 123 456 18446744073709551615";
        assert_eq!(parse_proc_stat_starttime(plain), Some(9876543));
        let hostile = "1234 (next-server (v15)) S 1 1234 1234 0 -1 4194304 0 0 0 0 5 3 0 0 20 0 11 0 424242 123 456";
        assert_eq!(parse_proc_stat_starttime(hostile), Some(424242));
        assert_eq!(parse_proc_stat_starttime("1234 (zsh) S 1 1234"), None);
        assert_eq!(parse_proc_stat_starttime(""), None);
    }

    #[test]
    fn surface_purge_drops_sessions_bound_to_dying_surface() {
        let mut session = AgentSession::new(TerminalAgent::ClaudeCode, AgentState::Errored);
        session.surface_id = Some(7);

        assert!(!keep_session_after_surface_purge(7, u32::MAX, &session));
        assert!(keep_session_after_surface_purge(8, u32::MAX, &session));
    }

    #[test]
    fn shell_prompt_reaps_the_surface_it_fired_on() {
        const SHELL: u32 = 4242;
        let mut thinking = AgentSession::new(TerminalAgent::ClaudeCode, AgentState::Thinking);
        thinking.surface_id = Some(7);
        assert!(!keep_session_at_shell_prompt(7, SHELL, u32::MAX, &thinking));
        assert!(keep_session_at_shell_prompt(8, SHELL, u32::MAX, &thinking));

        assert!(!keep_session_at_shell_prompt(7, SHELL, SHELL, &thinking));

        let mut errored = AgentSession::new(TerminalAgent::ClaudeCode, AgentState::Errored);
        errored.surface_id = Some(7);
        assert!(keep_session_at_shell_prompt(7, SHELL, u32::MAX, &errored));

        let mut backgrounded = AgentSession::new(TerminalAgent::Codex, AgentState::Thinking);
        backgrounded.surface_id = Some(7);
        let own_pid = std::process::id();
        backgrounded.proc_start = super::pid_start_time(own_pid);
        assert!(keep_session_at_shell_prompt(
            7,
            SHELL,
            own_pid,
            &backgrounded
        ));
    }

    #[test]
    fn a_pane_that_lost_its_agent_drops_the_row_keyed_on_its_own_shell() {
        const SHELL: u32 = 4242;
        let mut waiting = AgentSession::new(TerminalAgent::ClaudeCode, AgentState::WaitingForInput);
        waiting.surface_id = Some(7);
        assert!(!keep_session_without_agent_in_pane(
            7, SHELL, SHELL, &waiting
        ));
        assert!(keep_session_without_agent_in_pane(
            8, SHELL, SHELL, &waiting
        ));

        assert!(!keep_session_without_agent_in_pane(
            7,
            SHELL,
            u32::MAX,
            &waiting
        ));

        let mut errored = AgentSession::new(TerminalAgent::ClaudeCode, AgentState::Errored);
        errored.surface_id = Some(7);
        assert!(keep_session_without_agent_in_pane(
            7, SHELL, SHELL, &errored
        ));

        let mut backgrounded = AgentSession::new(TerminalAgent::Codex, AgentState::Thinking);
        backgrounded.surface_id = Some(7);
        let own_pid = std::process::id();
        backgrounded.proc_start = super::pid_start_time(own_pid);
        assert!(keep_session_without_agent_in_pane(
            7,
            SHELL,
            own_pid,
            &backgrounded
        ));
    }

    #[test]
    fn stale_sweep_keeps_synthetic_pid_without_os_probe() {
        let session = AgentSession::new(TerminalAgent::ClaudeCode, AgentState::Thinking);
        let live_surfaces = HashSet::new();

        assert!(stale_sweep_keeps_without_pid_probe(
            u32::MAX,
            &session,
            &live_surfaces
        ));
    }

    #[test]
    fn stale_sweep_keeps_errored_session_while_surface_is_live() {
        let mut session = AgentSession::new(TerminalAgent::Codex, AgentState::Errored);
        session.surface_id = Some(42);
        let live_surfaces = HashSet::from([42]);

        assert!(stale_sweep_keeps_without_pid_probe(
            1234,
            &session,
            &live_surfaces
        ));

        let live_surfaces = HashSet::new();
        assert!(!stale_sweep_keeps_without_pid_probe(
            1234,
            &session,
            &live_surfaces
        ));
    }

    #[test]
    fn merge_service_label_keeps_frontend_when_backend_mentions_same_port() {
        let mut labels = HashMap::new();
        assert!(merge_service_label(
            &mut labels,
            ServiceInfo {
                port: 3000,
                url: Some("http://localhost:3000/app".to_string()),
                label: Some("Next.js".to_string()),
                is_frontend: true,
            },
        ));

        assert!(!merge_service_label(
            &mut labels,
            ServiceInfo {
                port: 3000,
                url: Some("http://localhost:3000".to_string()),
                label: Some("Fastify".to_string()),
                is_frontend: false,
            },
        ));

        let info = labels.get(&3000).unwrap();
        assert_eq!(info.label.as_deref(), Some("Next.js"));
        assert_eq!(info.url.as_deref(), Some("http://localhost:3000/app"));
        assert!(info.is_frontend);
    }

    #[test]
    fn declaration_survives_only_absent_evidence_before_its_deadline() {
        use crate::agent_launcher::TerminalAgent;
        let now = std::time::Instant::now();
        let future = now.checked_add(std::time::Duration::from_secs(5));
        let past = now.checked_sub(std::time::Duration::from_secs(5));

        assert!(declaration_survives_scan(None, future, now));
        assert!(!declaration_survives_scan(None, past, now));
        assert!(!declaration_survives_scan(None, None, now));
        assert!(!declaration_survives_scan(
            Some(TerminalAgent::ClaudeCode),
            future,
            now
        ));
        assert!(!declaration_survives_scan(
            Some(TerminalAgent::Codex),
            future,
            now
        ));
    }

    #[test]
    fn merge_scan_workspace_state_adds_frontend_fallback_and_prunes_stale_labels() {
        let mut active_ports = vec![9999];
        let mut service_labels = HashMap::from([(
            9999,
            ServiceInfo {
                port: 9999,
                url: Some("http://localhost:9999".to_string()),
                label: Some("Vite".to_string()),
                is_frontend: true,
            },
        )]);
        let mut detected_agents = HashSet::new();
        let scan = HashMap::from([(
            7,
            PaneScan {
                ports: vec![PortEntry {
                    port: 5173,
                    frontend: Some("Vite"),
                }],
                agents: vec!["codex".to_string()],
                foreground_command: None,
            },
        )]);

        assert!(merge_scan_workspace_state(
            &mut active_ports,
            &mut service_labels,
            &mut detected_agents,
            &scan,
        ));

        assert_eq!(active_ports, vec![5173]);
        assert!(!service_labels.contains_key(&9999));
        let info = service_labels.get(&5173).unwrap();
        assert_eq!(info.url.as_deref(), Some("http://localhost:5173"));
        assert_eq!(info.label.as_deref(), Some("Vite"));
        assert!(info.is_frontend);
        assert!(detected_agents.contains("codex"));
    }

    #[test]
    fn merge_scan_workspace_state_preserves_exact_frontend_url() {
        let mut active_ports = vec![5173];
        let mut service_labels = HashMap::from([(
            5173,
            ServiceInfo {
                port: 5173,
                url: Some("http://localhost:5173/app".to_string()),
                label: Some("Vite".to_string()),
                is_frontend: true,
            },
        )]);
        let mut detected_agents = HashSet::new();
        let scan = HashMap::from([(
            7,
            PaneScan {
                ports: vec![PortEntry {
                    port: 5173,
                    frontend: Some("Vite"),
                }],
                agents: Vec::new(),
                foreground_command: None,
            },
        )]);

        assert!(!merge_scan_workspace_state(
            &mut active_ports,
            &mut service_labels,
            &mut detected_agents,
            &scan,
        ));
        assert_eq!(
            service_labels.get(&5173).unwrap().url.as_deref(),
            Some("http://localhost:5173/app")
        );
    }

    #[test]
    fn merge_scan_workspace_state_downgrades_unconfirmed_frontend_label() {
        let mut active_ports = vec![5173];
        let mut service_labels = HashMap::from([(
            5173,
            ServiceInfo {
                port: 5173,
                url: Some("http://localhost:5173/app".to_string()),
                label: Some("Vite".to_string()),
                is_frontend: true,
            },
        )]);
        let mut detected_agents = HashSet::new();
        let scan = HashMap::from([(
            7,
            PaneScan {
                ports: vec![PortEntry {
                    port: 5173,
                    frontend: None,
                }],
                agents: Vec::new(),
                foreground_command: None,
            },
        )]);

        assert!(merge_scan_workspace_state(
            &mut active_ports,
            &mut service_labels,
            &mut detected_agents,
            &scan,
        ));
        let info = service_labels.get(&5173).unwrap();
        assert!(!info.is_frontend);
        assert_eq!(info.label.as_deref(), Some("Vite"));
        assert_eq!(info.url.as_deref(), Some("http://localhost:5173/app"));
    }

    #[test]
    fn merge_scan_workspace_state_upgrades_terminal_label_from_frontend_scan() {
        let mut active_ports = vec![5173];
        let mut service_labels = HashMap::from([(
            5173,
            ServiceInfo {
                port: 5173,
                url: Some("http://localhost:5173/app".to_string()),
                label: Some("Vite".to_string()),
                is_frontend: false,
            },
        )]);
        let mut detected_agents = HashSet::new();
        let scan = HashMap::from([(
            7,
            PaneScan {
                ports: vec![PortEntry {
                    port: 5173,
                    frontend: Some("Vite"),
                }],
                agents: Vec::new(),
                foreground_command: None,
            },
        )]);

        assert!(merge_scan_workspace_state(
            &mut active_ports,
            &mut service_labels,
            &mut detected_agents,
            &scan,
        ));
        let info = service_labels.get(&5173).unwrap();
        assert!(info.is_frontend);
        assert_eq!(info.url.as_deref(), Some("http://localhost:5173/app"));
    }

    #[test]
    fn announced_port_conflicts_ignore_shared_ports() {
        let shared_scan = HashMap::from([
            (
                1,
                PaneScan {
                    ports: vec![PortEntry {
                        port: 3000,
                        frontend: None,
                    }],
                    agents: Vec::new(),
                    foreground_command: None,
                },
            ),
            (
                2,
                PaneScan {
                    ports: vec![PortEntry {
                        port: 3000,
                        frontend: None,
                    }],
                    agents: Vec::new(),
                    foreground_command: None,
                },
            ),
        ]);
        let (owner, shared) = port_ownership(&shared_scan);
        let display_names = HashMap::from([(1, "frontend".to_string())]);

        assert!(announced_port_conflicts(&[3000], 2, &owner, &shared, &display_names).is_empty());

        let single_owner_scan = HashMap::from([(
            1,
            PaneScan {
                ports: vec![PortEntry {
                    port: 5173,
                    frontend: Some("Vite"),
                }],
                agents: Vec::new(),
                foreground_command: None,
            },
        )]);
        let (owner, shared) = port_ownership(&single_owner_scan);
        let display_names = HashMap::from([(1, "vite pane".to_string())]);

        assert_eq!(
            announced_port_conflicts(&[5173], 2, &owner, &shared, &display_names),
            vec![(5173, "vite pane".to_string())]
        );
    }

    #[gpui::test]
    fn edgeless_drop_opens_a_new_workspace_tab(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;

        let cx = cx.add_empty_window();
        let new_pane = |cx: &mut gpui::VisualTestContext| {
            let terminal = cx.new(|cx| crate::terminal::TerminalView::display_only_for_test(1, cx));
            cx.new(|cx| crate::pane::Pane::new(terminal, 1, cx))
        };

        let target = new_pane(cx);
        let target_surface = cx.update(|_, cx| target.read(cx).surface.as_terminal().cloned());
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
            cx.update(|_, cx| target.read(cx).surface.as_terminal().cloned()),
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
